fn main() {
    if let Err(error) = podmesh_manager_resident_lab::cli::execute(std::env::args_os().skip(1)) {
        eprintln!("resident refused: {error}");
        std::process::exit(1);
    }
}
