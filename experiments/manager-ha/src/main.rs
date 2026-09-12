//! One bounded JSON request per process; no socket, daemon or network listener.
use std::{
    io::{self, Read},
    path::Path,
    process::ExitCode,
};

use podmesh_manager_ha_lab::durable::{Configuration, Request, Store};

fn main() -> ExitCode {
    match run() {
        Ok(response) => {
            println!("{response}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            println!("{}", serde_json::json!({"error": message}));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: podmesh-manager-ha-lab DATABASE CONFIGURATION_JSON_FILE REPLICA_ID; one JSON request on stdin".into());
    }
    let configuration: Configuration =
        serde_json::from_slice(&bounded(std::fs::File::open(&args[2]).map_err(error)?)?)
            .map_err(error)?;
    let request: Request = serde_json::from_slice(&bounded(io::stdin().lock())?).map_err(error)?;
    // Validate the request before opening or creating the state file.
    let mut store = Store::open(Path::new(&args[1]), configuration, &args[3])?;
    serde_json::to_string(&store.execute(&request)?).map_err(error)
}

fn bounded(reader: impl Read) -> Result<Vec<u8>, String> {
    const MAX_BYTES: u64 = 1_048_576;
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > usize::try_from(MAX_BYTES).map_err(error)? {
        return Err("input exceeds the 1 MiB laboratory limit".into());
    }
    Ok(bytes)
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
