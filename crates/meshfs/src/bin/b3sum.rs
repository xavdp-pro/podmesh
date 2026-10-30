use std::env;
use std::fs::File;
use std::io::{self, Read};

fn hash_reader<R: Read>(mut r: R) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let no_names = args.iter().any(|a| a == "--no-names");
    let files: Vec<&String> = args.iter().filter(|a| *a != "--no-names").collect();
    if files.is_empty() || (files.len() == 1 && files[0] == "-") {
        let digest = hash_reader(io::stdin().lock())?;
        if no_names {
            println!("{}", digest);
        } else {
            println!("{}  -", digest);
        }
    } else {
        for path in files {
            let digest = if path == "-" {
                hash_reader(io::stdin().lock())?
            } else {
                hash_reader(File::open(path)?)?
            };
            if no_names {
                println!("{}", digest);
            } else {
                println!("{}  {}", digest, path);
            }
        }
    }
    Ok(())
}
