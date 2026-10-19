package PVE::Storage::Custom::MeshFSPlugin;

# =============================================================================
# PodMesh MeshFS — Plugin de Stockage Officiel Proxmox VE
# Système de fichiers distribué, souverain, avec déduplication SHA-256
# et snapshots d'arbres de Merkle Copy-on-Write (Alternative légère à CephFS)
# =============================================================================

use strict;
use warnings;

use File::Path qw(mkpath);
use IO::File;
use POSIX;
use JSON;
use LWP::UserAgent;
use HTTP::Request;

use PVE::Tools qw(run_command);
use PVE::Storage::Plugin;
use PVE::JSONSchema qw(get_standard_option);

use base qw(PVE::Storage::Plugin);

# Alias pour compatibilité descendante et invocation PVE::Storage::MeshFSPlugin
{
    no strict 'refs';
    *PVE::Storage::MeshFSPlugin:: = *PVE::Storage::Custom::MeshFSPlugin::;
}

# Version API Stockage Proxmox VE
use constant APIVER => 16;
use constant APIAGE => 7;

sub api {
    my $ver = eval { PVE::Storage::APIVER() };
    return $ver if defined($ver);
    return APIVER;
}

# -----------------------------------------------------------------------------
# Configuration du Plugin Proxmox VE
# -----------------------------------------------------------------------------

sub type {
    return 'meshfs';
}

sub plugindata {
    return {
        content => [
            {
                images   => 1,
                rootdir  => 1,
                backup   => 1,
                iso      => 1,
                vztmpl   => 1,
                snippets => 1,
            },
            {
                images  => 1,
                rootdir => 1,
            },
        ],
        format => [
            {
                raw    => 1,
                qcow2  => 1,
                subvol => 1,
            },
            'raw',
        ],
    };
}

sub properties {
    return {
        meshvolume => {
            description => "Nom du volume MeshFS cible (défaut: default).",
            type        => 'string',
        },
        mountoptions => {
            description => "Options de montage FUSE / système de fichiers.",
            type        => 'string',
        },
    };
}

sub options {
    return {
        server                  => { optional => 1 },
        port                    => { optional => 1 },
        meshvolume              => { optional => 1 },
        path                    => { fixed => 1 },
        shared                  => { optional => 1 },
        content                 => { optional => 1 },
        format                  => { optional => 1 },
        nodes                   => { optional => 1 },
        disable                 => { optional => 1 },
        mountoptions            => { optional => 1 },
        'prune-backups'         => { optional => 1 },
        'max-protected-backups' => { optional => 1 },
        bwlimit                 => { optional => 1 },
        preallocation           => { optional => 1 },
    };
}

sub check_config {
    my ($class, $sectionId, $config, $create, $skipSchemaCheck) = @_;

    $config->{path} = "/mnt/pve/$sectionId" if $create && !$config->{path};

    return $class->SUPER::check_config($sectionId, $config, $create, $skipSchemaCheck);
}

# -----------------------------------------------------------------------------
# Communication REST avec le démon MeshFS
# -----------------------------------------------------------------------------

sub meshfs_api_request {
    my ($scfg, $method, $endpoint, $payload) = @_;

    my $server = $scfg->{server} // '127.0.0.1';
    my $port   = $scfg->{port} // 7443;
    my $url    = "http://${server}:${port}${endpoint}";

    my $ua = LWP::UserAgent->new(timeout => 5);
    $ua->agent("PVE-Storage-MeshFS/2.0");

    my $req = HTTP::Request->new($method => $url);
    $req->header('Content-Type' => 'application/json');

    if (defined $payload) {
        $req->content(encode_json($payload));
    }

    my $res = $ua->request($req);
    if (!$res->is_success) {
        die "MeshFS API error [$method $url]: " . $res->status_line . " - " . ($res->decoded_content // '') . "\n";
    }

    my $content = $res->decoded_content;
    return {} if !$content || $content =~ /^\s*$/;

    my $data;
    eval {
        $data = decode_json($content);
    };
    if ($@) {
        die "MeshFS invalid JSON returned from $url: $@\n";
    }

    return $data;
}

# -----------------------------------------------------------------------------
# Gestion du cycle de vie du stockage (Activation, Statut, Désactivation)
# -----------------------------------------------------------------------------

sub status {
    my ($class, $storeid, $scfg, $cache) = @_;

    my $path = $scfg->{path};
    return undef if !$path;

    my $active = 0;
    my ($total, $free, $used) = (0, 0, 0);

    # 1. Vérification santé via /api/v1/meshfs/status
    my $status;
    eval {
        $status = meshfs_api_request($scfg, 'GET', '/api/v1/meshfs/status');
        if ($status && $status->{ok}) {
            $active = 1;
        }
    };
    if ($@ || !$active) {
        return (0, 0, 0, 0);
    }

    # 2. Métriques d'espace disque
    # Si le point de montage local existe et est accessible
    if (-d $path) {
        my $df = eval { PVE::Tools::df($path, 1000); };
        if ($df && defined $df->{total} && $df->{total} > 0) {
            $total = $df->{total};
            $free  = $df->{avail} // $df->{free};
            $used  = $df->{used};
            return ($total, $free, $used, $active);
        }
    }

    # 3. Fallback sur les statistiques renvoyées par le démon MeshFS
    if ($status && defined $status->{diskTotal} && $status->{diskTotal} > 0) {
        $total = $status->{diskTotal};
        $used  = $status->{diskUsed} // 0;
        $free  = $status->{diskFree} // ($total - $used);
    } elsif ($status && defined $status->{totalLogicalBytes}) {
        # Si espace logique uniquement
        $used  = $status->{totalLogicalBytes};
        $total = $used + (1024 * 1024 * 1024 * 50); # Réserve affichée 50GB
        $free  = $total - $used;
    }

    return ($total, $free, $used, $active);
}

sub activate_storage {
    my ($class, $storeid, $scfg, $cache) = @_;

    my $path = $scfg->{path};
    die "MeshFS storage '$storeid': missing storage path\n" if !$path;

    my $server = $scfg->{server} // '127.0.0.1';
    my $port   = $scfg->{port} // 7443;
    my $volume = $scfg->{volume} // 'default';

    # 1. Vérification connectivité HTTP vers MeshFS
    my $status;
    eval {
        $status = meshfs_api_request($scfg, 'GET', '/api/v1/meshfs/status');
    };
    if ($@ || !$status || !$status->{ok}) {
        die "MeshFS storage '$storeid' is inactive or unreachable at http://${server}:${port}: " . ($@ // 'unknown error') . "\n";
    }

    # 2. Création du répertoire de montage si nécessaire
    if (! -d $path) {
        mkpath($path, 0, 0755);
    }

    # 3. Vérification point de montage FUSE
    my $is_mounted = 0;
    eval {
        run_command(['mountpoint', '-q', $path], noerr => 1, quiet => 1);
        $is_mounted = 1 if $? == 0;
    };

    if (!$is_mounted && -x '/usr/bin/meshfs-mount') {
        run_command([
            '/usr/bin/meshfs-mount',
            '--server', $server,
            '--port', $port,
            '--volume', $volume,
            $path
        ]);
    }

    # 4. Initialisation des répertoires de contenu standards PVE (images, dump, etc.)
    $class->SUPER::activate_storage($storeid, $scfg, $cache);

    return 1;
}

sub deactivate_storage {
    my ($class, $storeid, $scfg, $cache) = @_;

    my $path = $scfg->{path};
    return 1 if !$path;

    my $is_mounted = 0;
    eval {
        run_command(['mountpoint', '-q', $path], noerr => 1, quiet => 1);
        $is_mounted = 1 if $? == 0;
    };

    if ($is_mounted) {
        eval {
            run_command(['umount', '-l', $path]);
        };
        if ($@) {
            warn "MeshFS storage '$storeid': unmount failed at $path: $@\n";
        }
    }

    return 1;
}

# -----------------------------------------------------------------------------
# Gestion des Images disques (Allocation, Suppression, Liste)
# -----------------------------------------------------------------------------

sub alloc_image {
    my ($class, $storeid, $scfg, $vmid, $fmt, $name, $size) = @_;

    # Assure que le stockage est bien actif
    $class->activate_storage($storeid, $scfg);

    # Allocation standard via PVE::Storage::Plugin (création du fichier qcow2/raw)
    my $volname = $class->SUPER::alloc_image($storeid, $scfg, $vmid, $fmt, $name, $size);

    # Enregistrement ou synchronisation du volume logique auprès de MeshFS
    eval {
        my $volume = $scfg->{volume} // 'default';
        meshfs_api_request($scfg, 'POST', '/api/v1/meshfs/volumes', {
            name => $volume,
        });
    };

    return $volname;
}

sub free_image {
    my ($class, $storeid, $scfg, $volname, $isBase) = @_;

    # Suppression du fichier image physique
    my $res = $class->SUPER::free_image($storeid, $scfg, $volname, $isBase);

    # Purge des snapshots Merkle associés dans MeshFS
    eval {
        my $volume = $scfg->{volume} // 'default';
        meshfs_api_request($scfg, 'POST', '/api/v1/meshfs/snapshots/delete', {
            volume => $volume,
            name   => $volname,
        });
    };

    return $res;
}

sub list_images {
    my ($class, $storeid, $scfg, $vmid, $vollist, $cache) = @_;

    # Délégation à l'implémentation robuste de base de PVE
    return $class->SUPER::list_images($storeid, $scfg, $vmid, $vollist, $cache);
}

# -----------------------------------------------------------------------------
# Gestion des Fonctionnalités Avancées & Snapshots Merkle MeshFS
# -----------------------------------------------------------------------------

sub volume_has_feature {
    my ($class, $scfg, $aspect, $volname, $snapname, $running, $opts) = @_;

    my $features = {
        snapshot => { current => 1, snap => 1 },
        clone    => { base => 1, current => 1 },
        copy     => { base => 1, current => 1 },
        template => { current => 1 },
    };

    my ($vtype, $name, $vmid, $basename, $basevmid, $isBase, $format) =
        $class->parse_volname($volname);

    if ($aspect eq 'snapshot') {
        return 1;
    }

    return $features->{$aspect}->{$isBase ? 'base' : 'current'};
}

sub volume_snapshot {
    my ($class, $scfg, $storeid, $volname, $snap, $running) = @_;

    my $volume = $scfg->{volume} // 'default';
    my $snap_identifier = "${volname}\@${snap}";

    # 1. Déclenchement du Snapshot Merkle instantané dans MeshFS
    my $payload = {
        volume => $volume,
        name   => $snap_identifier,
        meta   => {
            storeid   => $storeid,
            volname   => $volname,
            snapshot  => $snap,
            running   => $running ? 1 : 0,
            timestamp => time(),
        },
    };

    my $res = eval {
        meshfs_api_request($scfg, 'POST', '/api/v1/meshfs/snapshots', $payload);
    };
    if ($@) {
        warn "MeshFS API Merkle snapshot notification: $@\n";
    }

    # 2. Si le format est qcow2, créer le snapshot qemu-img pour cohérence interne
    my (undef, undef, undef, undef, undef, undef, $format) = $class->parse_volname($volname);
    if ($format eq 'qcow2') {
        my $path = $class->filesystem_path($scfg, $volname);
        eval {
            run_command(['/usr/bin/qemu-img', 'snapshot', '-c', $snap, $path]);
        };
        if ($@) {
            die "qemu-img snapshot failed for $volname: $@\n";
        }
    }

    return 1;
}

sub volume_snapshot_delete {
    my ($class, $scfg, $storeid, $volname, $snap, $running) = @_;

    my $volume = $scfg->{volume} // 'default';
    my $snap_identifier = "${volname}\@${snap}";

    # 1. Suppression du Snapshot Merkle dans MeshFS
    eval {
        meshfs_api_request($scfg, 'POST', '/api/v1/meshfs/snapshots/delete', {
            volume => $volume,
            name   => $snap_identifier,
        });
    };
    if ($@) {
        warn "MeshFS API Merkle snapshot delete notification: $@\n";
    }

    # 2. Si le format est qcow2, supprimer le snapshot interne qemu-img
    my (undef, undef, undef, undef, undef, undef, $format) = $class->parse_volname($volname);
    if ($format eq 'qcow2') {
        my $path = $class->filesystem_path($scfg, $volname);
        eval {
            run_command(['/usr/bin/qemu-img', 'snapshot', '-d', $snap, $path]);
        };
        if ($@) {
            die "qemu-img snapshot delete failed for $volname: $@\n";
        }
    }

    return 1;
}

sub volume_snapshot_rollback {
    my ($class, $scfg, $storeid, $volname, $snap) = @_;

    my (undef, undef, undef, undef, undef, undef, $format) = $class->parse_volname($volname);
    if ($format eq 'qcow2') {
        my $path = $class->filesystem_path($scfg, $volname);
        run_command(['/usr/bin/qemu-img', 'snapshot', '-a', $snap, $path]);
    } else {
        die "MeshFS rollback for raw image '$volname' requires Merkle tree restore from MeshFS API\n";
    }

    return 1;
}

sub volume_rollback_is_possible {
    my ($class, $scfg, $storeid, $volname, $snap, $blockers) = @_;
    return 1;
}

1;
