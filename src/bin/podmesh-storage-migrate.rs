//! Explicit synthetic storage qualification CLI. No operational paths or DSN defaults.
use podmesh::store::{
    import::{canonical, fixture, snapshot, source},
    Fault, Result,
};
use serde_json::{json, Value as Json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn error(code: &str) -> podmesh::store::StoreError {
    Fault::Denied.error(code)
}

fn read_json(path: &Path) -> Result<Json> {
    let length = fs::metadata(path)
        .map_err(|_| error("contract_file_missing"))?
        .len();
    if length > 1_048_576 {
        return Err(error("contract_file_too_large"));
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| error("contract_file_read_failed"))?)
        .map_err(|_| error("contract_json_invalid"))
}

fn options(args: &[String]) -> Result<BTreeMap<String, PathBuf>> {
    if args.len() % 2 != 0 {
        return Err(error("explicit_named_paths_required"));
    }
    let mut out = BTreeMap::new();
    for pair in args.chunks(2) {
        if !matches!(
            pair[0].as_str(),
            "--output"
                | "--source-manifest"
                | "--snapshot"
                | "--seal"
                | "--contract"
                | "--plan"
                | "--receipt"
        ) || pair[1].is_empty()
            || out
                .insert(pair[0].clone(), PathBuf::from(&pair[1]))
                .is_some()
        {
            return Err(error("unknown_or_duplicate_option"));
        }
    }
    Ok(out)
}

fn required<'a>(options: &'a BTreeMap<String, PathBuf>, name: &str) -> Result<&'a Path> {
    options
        .get(name)
        .map(PathBuf::as_path)
        .ok_or_else(|| error("required_explicit_path_missing"))
}

fn inspect(options: &BTreeMap<String, PathBuf>) -> Result<Json> {
    let manifest = read_json(required(options, "--source-manifest")?)?;
    let seal = read_json(required(options, "--seal")?)?;
    if seal["source_manifest_sha256"]
        != canonical::hash(
            &serde_json::to_vec(&manifest).map_err(|_| error("contract_json_invalid"))?,
        )
    {
        return Err(error("snapshot_source_manifest_mismatch"));
    }
    let caps = fixture::development_caps()?;
    let size = seal["snapshot_bytes"]
        .as_u64()
        .ok_or_else(|| error("snapshot_size_missing"))?;
    caps.check(size, 0, 0, 0)?;
    let db = snapshot::open_sealed(required(options, "--snapshot")?, &seal)?;
    let plan = source::inspect(&db, &caps, size)?;
    let tables: Vec<Json> = plan.tables.iter().map(source::Table::manifest).collect();
    if json!(tables) != manifest["tables"] || plan.shape_hash != manifest["source_shape_sha256"] {
        return Err(error("synthetic_source_manifest_mismatch"));
    }
    let report = json!({"format":"podmesh-c02-inspection/1","source_version":10,"target_version":11,"status":"SOURCE_PREFLIGHT_ONLY_NOT_MIGRATION","tables":tables,"rows":plan.rows,"scalar_bytes":plan.scalar_bytes,"cells":plan.cells,"estimated_peak_memory":plan.estimated_peak,"source_shape_sha256":plan.shape_hash,"snapshot_sha256":seal["snapshot_sha256"],"source_schema_provenance":{"name":"node","version":10,"applied_at":plan.source_schema[2].integer()},"canonical_version":"podmesh-c02-canonical/1","reader_object":db.identity,"reader_path":db.read_path,"real_server_proof":false});
    snapshot::private_json(required(options, "--output")?, &report)?;
    Ok(
        json!({"status":report["status"],"rows":plan.rows,"tables":38,"report":required(options,"--output")?}),
    )
}

fn run() -> Result<Json> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args
        .first()
        .ok_or_else(|| error("explicit_operation_required"))?;
    let paths = options(&args[1..])?;
    let allowed: &[&str] = match mode.as_str() {
        "fixture-create" | "protocol" => &["--output"],
        "snapshot" => &["--source-manifest", "--output"],
        "inspect" => &["--source-manifest", "--snapshot", "--seal", "--output"],
        "capability" | "copy" | "resume" | "verify" | "restore-verify" => {
            &["--contract", "--snapshot", "--seal", "--output"]
        }
        "snapshot-campaign" => &["--contract", "--output"],
        _ => &[],
    };
    if !allowed.is_empty()
        && (paths.len() != allowed.len() || paths.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(error("required_explicit_paths_or_unknown_option"));
    }
    match mode.as_str() {
        "protocol" => {
            #[cfg(feature = "mariadb")]
            {
                let protocol = podmesh::store::import::target::protocol()?;
                snapshot::private_json(required(&paths, "--output")?, &protocol)?;
                Ok(json!({"status":protocol["status"],"protocol":required(&paths,"--output")?}))
            }
            #[cfg(not(feature = "mariadb"))]
            {
                Err(error("mariadb_feature_required_no_server_test_performed"))
            }
        }
        "fixture-create" => {
            let output = required(&paths, "--output")?;
            let manifest = fixture::create(output)?;
            Ok(
                json!({"status":"SYNTHETIC_SOURCE_CREATED_NOT_MIGRATED","rows":manifest["total_rows"],"tables":38,"source_manifest":output.join("source-manifest.json")}),
            )
        }
        "snapshot" => {
            let manifest = read_json(required(&paths, "--source-manifest")?)?;
            let path = manifest["source_path"]
                .as_str()
                .ok_or_else(|| error("source_path_missing"))?;
            let captured = snapshot::capture(
                Path::new(path),
                &manifest,
                required(&paths, "--output")?,
                &fixture::development_caps()?,
            )?;
            Ok(
                json!({"status":"PRIVATE_SNAPSHOT_CREATED_NOT_MIGRATED","snapshot":captured.path,"seal":required(&paths,"--output")?.join("seal.json")}),
            )
        }
        "snapshot-campaign" => {
            let contract = podmesh::store::import::contract::Contract::open(
                required(&paths, "--contract")?,
                &["copy", "copy_resume", "capability"],
            )?;
            podmesh::store::import::contract::check_binary_pin(&contract.json)?;
            let captured = snapshot::capture_controlled(
                &contract.source_path,
                &contract.source,
                required(&paths, "--output")?,
                &contract.caps,
                &mut |point| contract.event(point),
                &mut || contract.clock_boundary(),
            )?;
            contract.boundary()?;
            Ok(
                json!({"status":"PRIVATE_SNAPSHOT_CREATED_NOT_MIGRATED","snapshot":captured.path,"seal":required(&paths,"--output")?.join("seal.json"),"contract_sha256":contract.sha256}),
            )
        }
        "inspect" => inspect(&paths),
        "capability" | "copy" | "resume" | "verify" | "restore-verify" => {
            #[cfg(feature = "mariadb")]
            {
                podmesh::store::import::target::run(
                    mode,
                    &podmesh::store::import::target::Request {
                        contract_path: required(&paths, "--contract")?,
                        snapshot_path: required(&paths, "--snapshot")?,
                        seal_path: required(&paths, "--seal")?,
                        output: required(&paths, "--output")?,
                    },
                )
            }
            #[cfg(not(feature = "mariadb"))]
            {
                Err(error("mariadb_feature_required_no_server_test_performed"))
            }
        }
        _ => Err(error("operation_not_implemented")),
    }
}

fn main() {
    match run() {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!(
                "{}",
                json!({"status":"REFUSED","fault":error.fault.as_str(),"reason":error.message})
            );
            std::process::exit(2);
        }
    }
}
