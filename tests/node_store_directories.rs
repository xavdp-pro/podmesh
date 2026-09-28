//! Opening a state directory still prepares the directories the node's operations work in.
//!
//! One test, alone in its own binary: the scratch directory is claimed once per process, so a
//! second open in the same process keeps the first one's claim. Asserting this beside other tests
//! would assert which of them ran first.
use std::fs;

#[test]
fn opening_a_state_directory_prepares_the_directories_beside_the_journal() {
    if fs::read_to_string("/etc/machine-id").is_err() {
        eprintln!("skipped: this host has no /etc/machine-id to bind a journal to");
        return;
    }
    let dir = std::env::temp_dir().join(format!("podmesh-node-store-directories-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);

    let db = podmesh::open_state(&dir).unwrap();
    assert!(dir.join("state.sqlite").exists());
    for prepared in ["podman-tmp", "migrations", "inbox", "outbox"] {
        assert!(dir.join(prepared).is_dir(), "{prepared} was not prepared");
    }
    drop(db);
    fs::remove_dir_all(&dir).unwrap();
}
