//! One-shot command entry point for the bounded network laboratory.

use std::{env, process};

use podmesh_manager_network_lab::load_configuration;

fn main() {
    let arguments: Vec<_> = env::args().collect();
    let result = run(&arguments);
    if let Err(error) = result {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 3 {
        return Err("usage: podmesh-manager-network-lab serve-once CONFIG | sync CONFIG PEER OPERATION_ID NONCE".into());
    }
    let configuration = load_configuration(std::path::Path::new(&arguments[2]))?;
    let mut node = configuration.open()?;
    match arguments[1].as_str() {
        "serve-once" if arguments.len() == 3 => node.serve_once().map_err(Into::into),
        "sync" if arguments.len() == 6 => {
            let result = node.sync_to(&arguments[3], &arguments[4], &arguments[5])?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        _ => Err("invalid arguments".into()),
    }
}
