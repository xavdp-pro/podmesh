use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 || args[1] != "--policy" || args[3] != "--socket" {
        return Err(
            "usage: podmesh-host-adapter --policy <root-owned-policy> --socket <new-host-socket>"
                .into(),
        );
    }
    podmesh::host_adapter::serve(&PathBuf::from(&args[2]), &PathBuf::from(&args[4]))
}
