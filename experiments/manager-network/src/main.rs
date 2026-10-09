//! One-shot command entry point for the bounded network laboratory.

use std::{env, io::Write as _, process};

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
        return Err("usage: podmesh-manager-network-lab serve-once CONFIG | serve-once-ready CONFIG | serve-once-drop-reply-ready CONFIG | sync CONFIG PEER OPERATION_ID NONCE".into());
    }
    let configuration = load_configuration(std::path::Path::new(&arguments[2]))?;
    let mut node = configuration.open()?;
    match arguments[1].as_str() {
        "serve-once" if arguments.len() == 3 => node.serve_once().map_err(Into::into),
        "serve-once-ready" if arguments.len() == 3 => node
            .serve_once_reporting_address(|address| {
                println!("{address}");
                std::io::stdout()
                    .flush()
                    .map_err(podmesh_manager_network_lab::Error::from_io)?;
                Ok(())
            })
            .map_err(Into::into),
        "serve-once-drop-reply-ready" if arguments.len() == 3 => {
            if env::var_os("PODMESH_MANAGER_NETWORK_LAB_ENABLE_REPLY_LOSS").as_deref()
                != Some(std::ffi::OsStr::new("1"))
            {
                return Err("reply-loss mode requires the explicit laboratory fault gate".into());
            }
            node.serve_once_drop_reply_after_decision(|address| {
                println!("{address}");
                std::io::stdout()
                    .flush()
                    .map_err(podmesh_manager_network_lab::Error::from_io)?;
                Ok(())
            })
            .map_err(Into::into)
        }
        "sync" if arguments.len() == 6 => {
            let result = node.sync_to(&arguments[3], &arguments[4], &arguments[5])?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        _ => Err("invalid arguments".into()),
    }
}
