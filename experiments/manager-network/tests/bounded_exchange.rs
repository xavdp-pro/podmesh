//! Thousands of authenticated exchanges between two stores keep a bounded
//! per-exchange time, and every exchange stays completely accounted for.
//!
//! As in the resident, each side keeps one store open for its whole run and
//! also opens its store for every exchange, as the resident's outgoing worker
//! and incoming workers do. The sender appends a fact every hundred
//! exchanges, so most exchanges replay an existing receipt and some import new
//! facts, as in a replica's steady state. The stores live in a memory-backed
//! directory when one is available, so the timings measure work that depends
//! on the tables rather than the device's per-commit fsync latency;
//! `PODMESH_MANAGER_BENCH_DIR` selects another directory.
use std::{
    env,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use podmesh_manager_ha_lab::{
    durable::{inspect_read_only, Configuration, Request, Store},
    ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::{ConfigurationFile, Peer};

const EXCHANGES: usize = 3_000;
const WINDOW: usize = 300;
const FACT_EVERY: usize = 100;

#[test]
fn thousands_of_exchanges_keep_a_bounded_per_exchange_time() {
    let directory = bench_directory();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unused = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
    let source = configuration(
        directory.path(),
        1,
        [unused, listener.local_addr().unwrap(), unused],
    );
    let destination = configuration(directory.path(), 2, [unused; 3]);
    let receiver_configuration = destination.clone();
    let _source_resident = source.open().unwrap();
    let serving = thread::spawn(move || {
        let _destination_resident = receiver_configuration.open().unwrap();
        for _ in 0..EXCHANGES {
            let (stream, _) = listener.accept().unwrap();
            receiver_configuration
                .open()
                .unwrap()
                .serve_connection(stream)
                .unwrap();
        }
    });

    let mut elapsed = Vec::with_capacity(EXCHANGES);
    let mut operation = String::new();
    for exchange in 0..EXCHANGES {
        if exchange % FACT_EVERY == 0 {
            Store::open(&source.database_path, source.manager.clone(), "r1")
                .unwrap()
                .execute(&Request::Observe {
                    operation_id: format!("fact-{exchange}"),
                    scope: "scope1".into(),
                    subject: "exchange-loop".into(),
                    exclusive_resource: None,
                    active_claim: false,
                    value: format!("value-{exchange}"),
                })
                .unwrap();
            operation = format!("sync-{exchange}");
        }
        let started = Instant::now();
        let result = source
            .open()
            .unwrap()
            .sync_to("r2", &operation, &format!("nonce-{exchange}"))
            .unwrap();
        elapsed.push(started.elapsed());
        assert_eq!(result.replayed, exchange % FACT_EVERY != 0);
    }
    serving.join().unwrap();

    let early = median(&elapsed[WINDOW..2 * WINDOW]);
    let late = median(&elapsed[EXCHANGES - WINDOW..]);
    println!(
        "bounded-exchange {EXCHANGES} exchanges: median per exchange {early:?} for exchanges {WINDOW}-{}, {late:?} for the last {WINDOW}; slowest {:?}",
        2 * WINDOW - 1,
        elapsed.iter().max().unwrap()
    );

    let sent = inspect_read_only(&source.database_path, &source.manager, "r1").unwrap();
    let received =
        inspect_read_only(&destination.database_path, &destination.manager, "r2").unwrap();
    assert_eq!(sent.audit_event_count, 2 * EXCHANGES);
    assert_eq!(received.audit_event_count, 4 * EXCHANGES);
    assert!(sent.incomplete_attempts.is_empty());
    assert!(received.incomplete_attempts.is_empty());
    assert!(received.unaudited_import_receipt_ids.is_empty());
    assert_eq!(received.receipt_count, EXCHANGES / FACT_EVERY);
    assert_eq!(sent.history_count, EXCHANGES / FACT_EVERY);
    assert_eq!(sent.logical_history_sha256, received.logical_history_sha256);

    assert!(
        late < Duration::from_millis(100),
        "late median per exchange {late:?}"
    );
    assert!(
        late <= early * 2 + Duration::from_millis(5),
        "median per exchange grew from {early:?} to {late:?} while the receiver's audit table grew to {} rows",
        received.audit_event_count
    );
}

fn bench_directory() -> tempfile::TempDir {
    let base = env::var_os("PODMESH_MANAGER_BENCH_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let memory = Path::new("/dev/shm");
            memory.is_dir().then(|| memory.to_path_buf())
        });
    base.and_then(|base| {
        tempfile::Builder::new()
            .prefix("podmesh-manager-bench-")
            .tempdir_in(base)
            .ok()
    })
    .unwrap_or_else(|| tempfile::tempdir().unwrap())
}

fn configuration(
    directory: &Path,
    replica: usize,
    endpoints: [SocketAddr; 3],
) -> ConfigurationFile {
    let key = |left: usize, right: usize| match (left.min(right), left.max(right)) {
        (1, 2) => "11".repeat(32),
        (1, 3) => "22".repeat(32),
        _ => "33".repeat(32),
    };
    ConfigurationFile {
        replica_id: format!("r{replica}"),
        database_path: directory.join(format!("r{replica}.sqlite")),
        manager: Configuration {
            logical_manager_id: "bounded-exchange".into(),
            replicas: (1..=3)
                .map(|n| ReplicaConfig {
                    replica_id: format!("r{n}"),
                    host_id: format!("h{n}"),
                })
                .collect(),
            grants: (1..=3)
                .map(|n| ScopeGrant {
                    scope: format!("scope{n}"),
                    owner_replica_id: format!("r{n}"),
                })
                .collect(),
        },
        bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        peers: (1..=3)
            .filter(|peer| *peer != replica)
            .map(|peer| Peer {
                replica_id: format!("r{peer}"),
                endpoint: endpoints[peer - 1],
                shared_key_hex: key(replica, peer),
            })
            .collect(),
    }
}

fn median(values: &[Duration]) -> Duration {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted[sorted.len() / 2]
}
