//! Strict offline validation and explicit network opt-in for the package candidate.
use crate::{Configuration, Result};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt},
    },
    path::{Path, PathBuf},
};

const NETWORK_MODE: &str = "PODMESH_MANAGER_NETWORK_MODE";
struct Options {
    config: PathBuf,
    state: Option<PathBuf>,
    runtime: Option<PathBuf>,
    validate: bool,
}

pub fn require_network_mode() -> Result<()> {
    match std::env::var(NETWORK_MODE).as_deref() {
        Ok("authenticated-static-peers") => Ok(()),
        Ok("disabled") | Err(std::env::VarError::NotPresent) => Err("runtime networking disabled; set PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers explicitly".into()),
        _ => Err("unsupported PODMESH_MANAGER_NETWORK_MODE".into()),
    }
}

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let args: Vec<_> = arguments.collect();
    if args == [OsString::from("--version")] {
        println!("podmesh-managerd {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = parse(&args)?;
    trusted_file(&options.config, false)?;
    let mut bytes = Vec::new();
    fs::File::open(&options.config)?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        return Err("configuration exceeds 1 MiB".into());
    }
    let config: Configuration = serde_json::from_slice(&bytes)?;
    // Legacy positional calls derive the same strict boundaries from config.
    let state = options
        .state
        .as_deref()
        .or_else(|| config.network.database_path.parent())
        .ok_or("state directory missing")?;
    let runtime = options
        .runtime
        .as_deref()
        .or_else(|| config.control_socket.parent())
        .ok_or("runtime directory missing")?;
    validate_paths(&config, state, runtime)?;
    config.validate()?;
    match std::env::var(NETWORK_MODE).as_deref() {
        Ok("disabled" | "authenticated-static-peers") | Err(std::env::VarError::NotPresent) => {}
        _ => return Err("unsupported PODMESH_MANAGER_NETWORK_MODE".into()),
    }
    if options.validate {
        println!("{{\"configuration_valid\":true,\"durable_store_checked\":false,\"network_started\":false}}");
        return Ok(());
    }
    crate::run(config)
}

fn parse(args: &[OsString]) -> Result<Options> {
    if args.len() == 1 && !args[0].as_bytes().starts_with(b"-") {
        return Ok(Options {
            config: PathBuf::from(&args[0]),
            state: None,
            runtime: None,
            validate: false,
        });
    }
    let mut config = None;
    let mut state = None;
    let mut runtime = None;
    let mut validate = false;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].to_str().ok_or("invalid flag")?;
        if name == "--validate-config" {
            if validate {
                return Err("duplicate --validate-config".into());
            }
            validate = true;
            index += 1;
            continue;
        }
        let target = match name {
            "--config" => &mut config,
            "--state-dir" => &mut state,
            "--runtime-dir" => &mut runtime,
            _ => return Err("unknown or positional argument in flag mode".into()),
        };
        if target.is_some() {
            return Err("duplicate option".into());
        }
        index += 1;
        let value = args.get(index).ok_or("missing option value")?;
        if value.as_bytes().starts_with(b"-") {
            return Err("missing option value".into());
        }
        *target = Some(PathBuf::from(value));
        index += 1;
    }
    Ok(Options {
        config: config.ok_or("--config required")?,
        state: Some(state.ok_or("--state-dir required")?),
        runtime: Some(runtime.ok_or("--runtime-dir required")?),
        validate,
    })
}

fn lexical(path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute()
        || bytes.len() > 4096
        || bytes.contains(&0)
        || bytes[1..]
            .split(|b| *b == b'/')
            .any(|part| part.is_empty() || part == b"." || part == b"..")
    {
        return Err(
            "paths must be absolute without traversal, repeated separators or trailing separators"
                .into(),
        );
    }
    Ok(())
}

fn ancestors(path: &Path) -> Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    let mut current = PathBuf::from("/");
    for part in path.as_os_str().as_bytes()[1..].split(|b| *b == b'/') {
        current.push(OsStr::from_bytes(part));
        let metadata = fs::symlink_metadata(&current)?;
        if !metadata.is_dir() || (metadata.uid() != 0 && metadata.uid() != uid) {
            return Err(
                "directory chain must be nonsymlink and owned by root or current user".into(),
            );
        }
        // Root-owned sticky /tmp-style ancestors permit owned private children.
        if metadata.mode() & 0o022 != 0 && !(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0) {
            return Err("directory chain is writable by an untrusted user".into());
        }
    }
    Ok(())
}

fn directory(path: &Path, private: bool) -> Result<()> {
    lexical(path)?;
    ancestors(path)?;
    let metadata = fs::symlink_metadata(path)?;
    let forbidden = if private { 0o077 } else { 0o022 };
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & forbidden != 0 {
        return Err(
            "declared directories require current-user ownership and safe permissions".into(),
        );
    }
    Ok(())
}

fn trusted_file(path: &Path, missing_allowed: bool) -> Result<()> {
    lexical(path)?;
    ancestors(path.parent().ok_or("file has no parent")?)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let uid = rustix::process::geteuid().as_raw();
            let owned = metadata.uid() == uid || (!missing_allowed && metadata.uid() == 0);
            if !metadata.is_file()
                || metadata.nlink() != 1
                || !owned
                || metadata.mode() & 0o022 != 0
            {
                return Err(
                    "file must be regular, singly linked, owned and not untrusted-writable".into(),
                );
            }
        }
        Err(error) if missing_allowed && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_paths(config: &Configuration, state: &Path, runtime: &Path) -> Result<()> {
    directory(state, false)?;
    directory(runtime, true)?;
    let db = &config.network.database_path;
    let socket = &config.control_socket;
    lexical(db)?;
    lexical(socket)?;
    if db.parent() != Some(state) || socket.parent() != Some(runtime) {
        return Err("database/socket must be direct children of declared directories".into());
    }
    trusted_file(db, true)?;
    trusted_file(&db.with_extension("resident-lock"), true)?;
    for suffix in ["-wal", "-shm"] {
        let mut path = db.as_os_str().to_os_string();
        path.push(suffix);
        trusted_file(Path::new(&path), true)?;
    }
    match fs::symlink_metadata(socket) {
        Ok(metadata)
            if metadata.file_type().is_socket()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o077 == 0 => {}
        Ok(_) => return Err("existing control path must be an owned private Unix socket".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
