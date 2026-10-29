//! PodMesh Unified Orchestrator CLI (`podmesh-ctl`) in Pure Rust

use clap::{Args, Parser, Subcommand};
use podmesh_cli::admin::get_cluster_status;
use podmesh_cli::ha::HALedger;
use podmesh_cli::move_cmd::execute_move;
use podmesh_cli::replication::ReplicationManager;
use podmesh_cli::system::{run_boot_restore, run_self_fence};
use podmesh_cli::{run_decision_follow, run_publisher_follow};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "podmesh-ctl",
    author = "Xavier DP <xavier@xavdp.pro>",
    version = "2.4.1",
    about = "PodMesh Unified Orchestration, Replication & HA Failover CLI in Pure Rust",
    long_about = "Replaces legacy Python tools (ha-standby, replicate-universe, move-universe) with native, ultra-fast Rust binaries."
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// High-Availability & Warm-Standby Failover operations
    Ha(HaArgs),

    /// Universe Replication management
    Replicate(ReplicateArgs),

    /// Transparently migrate an universe between two hosts
    Move(MoveArgs),

    /// Cluster & Manager administration
    Admin(AdminArgs),

    /// Systemd mandate execution (replaces shell scripts)
    System(SystemArgs),
}

#[derive(Args, Debug)]
pub struct HaArgs {
    #[command(subcommand)]
    pub action: HaSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum HaSubcommand {
    /// Declare an authority policy for a universe
    Declare {
        universe: String,
        #[arg(short, long, default_value = "authority-default")]
        authority: String,
        #[arg(short, long, default_value_t = 30)]
        lease: u64,
        #[arg(short, long, default_value_t = 20)]
        margin: u64,
    },
    /// Rotate epoch to a new holder and output signed Ed25519 takeover proof
    Rotate {
        universe: String,
        #[arg(short, long)]
        host: String,
        #[arg(short, long)]
        barrier: Option<i64>,
    },
    /// Inspect HA policy and gate state
    Inspect {
        universe: String,
    },
}

#[derive(Args, Debug)]
pub struct ReplicateArgs {
    #[command(subcommand)]
    pub action: ReplicateSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ReplicateSubcommand {
    /// Configure replication target and schedule
    Configure {
        universe: String,
        #[arg(short, long)]
        active: String,
        #[arg(short, long, value_delimiter = ',')]
        standbys: Vec<String>,
        #[arg(short, long, default_value_t = 900)]
        interval: u64,
        #[arg(long, default_value_t = false)]
        live: bool,
    },
    /// Run one replication cycle now
    Run {
        universe: String,
    },
    /// Show replication status and last run reports
    Status {
        universe: String,
    },
}

#[derive(Args, Debug)]
pub struct MoveArgs {
    /// Name of the universe to relocate
    pub universe: String,
    /// Source host
    pub source: String,
    /// Destination host
    pub destination: String,
}

#[derive(Args, Debug)]
pub struct AdminArgs {
    #[command(subcommand)]
    pub action: AdminSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum AdminSubcommand {
    /// Query cluster manager status, quorum and replicas
    Status,
}

#[derive(Args, Debug)]
pub struct SystemArgs {
    #[command(subcommand)]
    pub action: SystemSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum SystemSubcommand {
    /// Execute self-fence under /etc/podmesh/fence-mandate
    Fence {
        #[arg(short, long, default_value = "/etc/podmesh/fence-mandate")]
        mandate: PathBuf,
    },
    /// Execute boot restore under /etc/podmesh/restore-mandate
    Restore {
        #[arg(short, long, default_value = "/etc/podmesh/restore-mandate")]
        mandate: PathBuf,
    },
    /// Execute decision follow under /etc/podmesh/decision-follow-mandate
    FollowDecision {
        #[arg(short, long)]
        mandate: Option<PathBuf>,
    },
    /// Execute publisher follow under /etc/podmesh/publisher-follow-mandate
    FollowPublisher {
        #[arg(short, long)]
        mandate: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    // Multi-call binary inspection (drop-in compatibility with symlinks)
    if let Some(arg0) = std::env::args().next() {
        let file_name = std::path::Path::new(&arg0)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        if file_name == "podmesh-decision-follow" {
            let code = run_decision_follow(None).await;
            return ExitCode::from(code as u8);
        } else if file_name == "podmesh-publisher-follow" {
            let code = run_publisher_follow(None).await;
            return ExitCode::from(code as u8);
        } else if file_name == "podmesh-fence" {
            let mandate = PathBuf::from(
                std::env::var("PODMESH_FENCE_MANDATE").unwrap_or_else(|_| "/etc/podmesh/fence-mandate".to_string()),
            );
            return match run_self_fence(&mandate) {
                Ok(outcome) => {
                    println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("podmesh-fence: {}", e);
                    ExitCode::from(3)
                }
            };
        } else if file_name == "podmesh-restore" {
            let mandate = PathBuf::from(
                std::env::var("PODMESH_RESTORE_MANDATE").unwrap_or_else(|_| "/etc/podmesh/restore-mandate".to_string()),
            );
            return match run_boot_restore(&mandate) {
                Ok(outcome) => {
                    println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("podmesh-restore: {}", e);
                    ExitCode::from(3)
                }
            };
        }
    }

    let args = Cli::parse();

    let default_level = if args.debug { "debug" } else { "info" };
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("podmesh_cli={}", default_level)));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .init();

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let ledger_dir = PathBuf::from(home).join(".podmesh-ha");

    match args.command {
        Commands::Ha(ha) => {
            let ledger = match HALedger::new(Some(&ledger_dir)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Erreur initialisation ledger HA: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            match ha.action {
                HaSubcommand::Declare {
                    universe,
                    authority,
                    lease,
                    margin,
                } => match ledger.declare_policy(&universe, &authority, lease, margin) {
                    Ok(pol) => {
                        println!("{}", serde_json::to_string_pretty(&pol).unwrap());
                    }
                    Err(e) => {
                        eprintln!("Erreur déclaration politique: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                HaSubcommand::Rotate {
                    universe,
                    host,
                    barrier,
                } => match ledger.rotate_epoch(&universe, &host, barrier) {
                    Ok(proof) => {
                        println!("{}", serde_json::to_string_pretty(&proof).unwrap());
                    }
                    Err(e) => {
                        eprintln!("Erreur rotation d'époque: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                HaSubcommand::Inspect { universe } => match ledger.load_policy(&universe) {
                    Ok(pol) => {
                        println!("{}", serde_json::to_string_pretty(&pol).unwrap());
                    }
                    Err(e) => {
                        eprintln!("Erreur inspection: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
            }
        }

        Commands::Replicate(rep) => {
            let mgr = match ReplicationManager::new(&ledger_dir) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Erreur réplication: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            match rep.action {
                ReplicateSubcommand::Configure {
                    universe,
                    active,
                    standbys,
                    interval,
                    live,
                } => match mgr.configure(&universe, &active, standbys, interval, live) {
                    Ok(cfg) => {
                        println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
                    }
                    Err(e) => {
                        eprintln!("Erreur configuration: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                ReplicateSubcommand::Run { universe } => match mgr.run_replication(&universe) {
                    Ok(report) => {
                        println!("{}", serde_json::to_string_pretty(&report).unwrap());
                    }
                    Err(e) => {
                        eprintln!("Erreur run réplication: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                ReplicateSubcommand::Status { universe } => match mgr.load_config(&universe) {
                    Ok(cfg) => {
                        println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
                    }
                    Err(e) => {
                        eprintln!("Erreur statut: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
            }
        }

        Commands::Move(m) => {
            let report = execute_move(&m.universe, &m.source, &m.destination);
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }

        Commands::Admin(a) => match a.action {
            AdminSubcommand::Status => {
                let status = get_cluster_status();
                println!("{}", serde_json::to_string_pretty(&status).unwrap());
            }
        },

        Commands::System(s) => match s.action {
            SystemSubcommand::Fence { mandate } => match run_self_fence(&mandate) {
                Ok(outcome) => {
                    println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
                }
                Err(e) => {
                    eprintln!("Erreur self-fence: {}", e);
                    return ExitCode::FAILURE;
                }
            },
            SystemSubcommand::Restore { mandate } => match run_boot_restore(&mandate) {
                Ok(outcome) => {
                    println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
                }
                Err(e) => {
                    eprintln!("Erreur boot-restore: {}", e);
                    return ExitCode::FAILURE;
                }
            },
            SystemSubcommand::FollowDecision { mandate } => {
                let code = run_decision_follow(mandate.as_deref()).await;
                return ExitCode::from(code as u8);
            }
            SystemSubcommand::FollowPublisher { mandate } => {
                let code = run_publisher_follow(mandate.as_deref()).await;
                return ExitCode::from(code as u8);
            }
        },
    }

    ExitCode::SUCCESS
}
