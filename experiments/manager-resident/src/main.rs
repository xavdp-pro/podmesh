use std::{io::Read, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("resident refused: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.len() != 2 {
        return Err("usage: podmesh-manager-resident-lab CONFIG.json".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(Path::new(&arguments[1]))?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        return Err("configuration exceeds 1 MiB".into());
    }
    podmesh_manager_resident_lab::run(serde_json::from_slice(&bytes)?)
}
