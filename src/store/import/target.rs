//! MariaDB synthetic importer. Private checked context; normal paths always reject its marker.
use super::{
    canonical,
    contract::{self, Contract},
    copy, refusal, snapshot,
    source::{self, Plan, Table},
};
use crate::store::{
    migrations, DurableStore, Engine, MariadbConfig, MariadbStore, Result, Row, Value,
};
use serde_json::{json, Value as Json};
use std::{path::Path, time::Duration};

const CONTROL: &str = include_str!("../migrations/import/0001-control.mariadb.sql");

fn encode_rows(rows: Vec<Row>) -> Result<Json> {
    rows.into_iter()
        .map(|row| {
            row.into_values()
                .into_iter()
                .map(|value| match value {
                    Value::Null => Ok(Json::Null),
                    Value::Integer(n) => Ok(json!(n)),
                    Value::Text(s) => Ok(json!(s)),
                    _ => Err(refusal("target_schema_raw_class_refused")),
                })
                .collect::<Result<Vec<Json>>>()
                .map(|row| json!(row))
        })
        .collect::<Result<Vec<Json>>>()
        .map(|rows| json!(rows))
}
fn metadata(db: &mut dyn DurableStore, sql: &str) -> Result<Json> {
    encode_rows(db.query(sql, &[])?)
}

/// Stable schema metadata: excludes counters/row estimates; includes full index, generated
/// columns, all constraints and node version. Data/control records are validated separately.
pub fn shape(db: &mut dyn DurableStore) -> Result<Json> {
    let objects=metadata(db,"SELECT TABLE_NAME,TABLE_TYPE,ENGINE,ROW_FORMAT,TABLE_COLLATION FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME")?;
    let columns=metadata(db,"SELECT TABLE_NAME,COLUMN_NAME,ORDINAL_POSITION,COLUMN_TYPE,DATA_TYPE,IS_NULLABLE,COLUMN_DEFAULT,COLUMN_KEY,EXTRA,CHARACTER_MAXIMUM_LENGTH,COLLATION_NAME,CHARACTER_SET_NAME,GENERATION_EXPRESSION FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME,ORDINAL_POSITION")?;
    let indexes=metadata(db,"SELECT TABLE_NAME,INDEX_NAME,NON_UNIQUE,SEQ_IN_INDEX,COLUMN_NAME,SUB_PART,INDEX_TYPE,NULLABLE FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME,INDEX_NAME,SEQ_IN_INDEX")?;
    let constraints=metadata(db,"SELECT TABLE_NAME,CONSTRAINT_NAME,CONSTRAINT_TYPE FROM information_schema.TABLE_CONSTRAINTS WHERE CONSTRAINT_SCHEMA=DATABASE() ORDER BY TABLE_NAME,CONSTRAINT_NAME")?;
    let key_columns=metadata(db,"SELECT TABLE_NAME,CONSTRAINT_NAME,ORDINAL_POSITION,COLUMN_NAME,REFERENCED_TABLE_NAME,REFERENCED_COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE CONSTRAINT_SCHEMA=DATABASE() ORDER BY TABLE_NAME,CONSTRAINT_NAME,ORDINAL_POSITION")?;
    let checks=metadata(db,"SELECT TABLE_NAME,CONSTRAINT_NAME,CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS WHERE CONSTRAINT_SCHEMA=DATABASE() ORDER BY TABLE_NAME,CONSTRAINT_NAME")?;
    let triggers=metadata(db,"SELECT TRIGGER_NAME,EVENT_MANIPULATION,EVENT_OBJECT_TABLE,ACTION_STATEMENT,ACTION_TIMING FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=DATABASE() ORDER BY TRIGGER_NAME")?;
    let routines=metadata(db,"SELECT ROUTINE_NAME,ROUTINE_TYPE FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=DATABASE() ORDER BY ROUTINE_NAME")?;
    let events=metadata(db,"SELECT EVENT_NAME FROM information_schema.EVENTS WHERE EVENT_SCHEMA=DATABASE() ORDER BY EVENT_NAME")?;
    let has_schema = objects
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row[0] == "store_schema"));
    let schema_record = if has_schema {
        metadata(
            db,
            "SELECT name,version,applied_at FROM store_schema ORDER BY name",
        )?
    } else {
        json!([])
    };
    Ok(
        json!({"objects":objects,"columns":columns,"indexes":indexes,"constraints":constraints,"key_columns":key_columns,"checks":checks,"triggers":triggers,"routines":routines,"events":events,"schema_record":schema_record}),
    )
}
fn digest(value: &Json) -> Result<String> {
    Ok(canonical::hash(
        &serde_json::to_vec(value).map_err(|_| refusal("canonical_contract_json_failed"))?,
    ))
}

fn select_schema_stage(stages: &[Json], actual_hash: &str, final_step: usize) -> Result<usize> {
    let expected_count = final_step
        .checked_add(1)
        .ok_or_else(|| refusal("capability_stage_invalid"))?;
    if stages.len() != expected_count {
        return Err(refusal("capability_stage_invalid"));
    }
    for (position, stage) in stages.iter().enumerate() {
        let step = stage["step"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok());
        let hash = stage["shape_sha256"].as_str();
        if step != Some(position)
            || hash.is_none_or(|value| {
                value.len() != 64
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(refusal("capability_stage_invalid"));
        }
    }

    let mut matching = stages
        .iter()
        .enumerate()
        .filter(|(_, stage)| stage["shape_sha256"] == actual_hash);
    let (position, stage) = matching
        .next()
        .ok_or_else(|| refusal("target_unknown_or_ambiguous_partial_schema"))?;
    if matching.next().is_some() {
        return Err(refusal("target_unknown_or_ambiguous_partial_schema"));
    }
    let step = stage["step"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| refusal("capability_stage_invalid"))?;
    if step > final_step {
        return Err(refusal("capability_stage_invalid"));
    }
    if step != position {
        return Err(refusal("capability_stage_invalid"));
    }
    Ok(step)
}

fn validate_next_schema_stage(
    stages: &[Json],
    completed_step: usize,
    actual_hash: &str,
) -> Result<()> {
    if stages.get(completed_step).is_none_or(|stage| {
        stage["step"] != json!(completed_step) || stage["shape_sha256"] != actual_hash
    }) {
        return Err(refusal("target_ddl_stage_drift"));
    }
    Ok(())
}
fn prerequisite_provenance(receipt: &Json, contract: &Json, plan_sha: &str) -> Result<()> {
    for key in [
        "source_commit",
        "binary_sha256",
        "source_manifest_sha256",
        "SQL_hashes",
        "migration_id",
        "resource_caps",
        "canonical_contract_sha256",
        "resource_caps_contract_sha256",
    ] {
        if receipt[key] != contract[key] || receipt[key].is_null() {
            return Err(refusal(
                "prerequisite_exact_source_binary_or_input_mismatch",
            ));
        }
    }
    if receipt["plan_sha256"] != plan_sha {
        return Err(refusal("prerequisite_exact_plan_mismatch"));
    }
    Ok(())
}

fn plan_binding(contract: &Contract, plan: &Plan, seal: &Json) -> Result<(String,String,String)> {
    let source_schema_hex = canonical::row(&plan.source_schema)?
        .iter().map(|b|format!("{b:02x}")).collect::<String>();
    let snapshot_sha = contract::text(seal,"snapshot_sha256")?.to_owned();
    let plan_sha=digest(&json!({"tables":plan.tables.iter().map(Table::manifest).collect::<Vec<_>>(),"shape":plan.shape_hash,"snapshot":snapshot_sha,"source_schema_hex":source_schema_hex,"source_manifest":contract.source_sha256,"caps":contract.json["resource_caps"],"canonical":contract::CANONICAL_SHA,"SQL":sql_hashes()}))?;
    Ok((source_schema_hex,snapshot_sha,plan_sha))
}

fn validate_dump_restore_link(receipt:&Json,contract:&Json,origin:&Json,source:&Json,target:&Json)->Result<()> {
    if receipt["format"]!="podmesh-A-synthetic-dump-restore/1" || receipt["status"]!="DUMP_RESTORED_TYPED_VERIFICATION_PENDING"
        || receipt["serving"]!=false || receipt["identity_rebound"]!=false || receipt["typed_verification_proven"]!=false {
        return Err(refusal("admin_dump_receipt_status_or_flags_refused"));
    }
    if receipt["origin_receipt"]!=contract["restore_origin_receipt"] || receipt["origin_receipt"].is_null()
        || receipt["helper_sha256"]!=contract["dump_restore_helper_sha256"] || receipt["helper_sha256"].is_null()
        || receipt["restored_database"]!=contract["database"] || source["database"]!=origin["database"] {
        return Err(refusal("admin_dump_origin_helper_or_database_mismatch"));
    }
    for key in ["container_id","volume_name","image_id"] {
        if source[key].is_null() || target[key].is_null() || source[key]!=origin["target_resource_identity"][key] || target[key]!=contract["target_resource_identity"][key] {
            return Err(refusal("admin_dump_resource_identity_mismatch"));
        }
    }
    if source["container_id"]==target["container_id"] || source["volume_name"]==target["volume_name"] || origin["database"]==contract["database"] {
        return Err(refusal("admin_dump_distinct_resource_required"));
    }
    Ok(())
}
fn validate_dump_hash(receipt:&Json,actual:&str)->Result<()> {
    if actual!=contract::text(receipt,"dump_sha256")?{return Err(refusal("admin_dump_actual_bytes_checksum_mismatch"));}
    Ok(())
}
fn checked_dump_restore(contract:&Contract,origin:&Json)->Result<Json> {
    let spec=&contract.json["dump_restore_receipt"];
    let receipt=contract::pinned_json(spec)?;
    let source=contract::pinned_json(&receipt["source_resource"])?;
    let target=contract::pinned_json(&receipt["target_resource"])?;
    validate_dump_restore_link(&receipt,&contract.json,origin,&source,&target)?;
    let bytes=contract::number(&receipt,"dump_bytes")?;
    let path=Path::new(contract::text(spec,"path")?).parent().ok_or_else(||refusal("admin_dump_parent_missing"))?.join("database.sql");
    let sha=contract::hash_private_stream(&path,bytes,67_108_864,&mut ||contract.clock_boundary())?;
    validate_dump_hash(&receipt,&sha)?;
    Ok(receipt)
}

fn warnings(db: &mut dyn DurableStore) -> Result<()> {
    if !db.query("SHOW WARNINGS", &[])?.is_empty() {
        return Err(refusal("target_sql_warning_refused"));
    }
    Ok(())
}

pub fn sql_hashes() -> Json {
    let mut hashes = serde_json::Map::new();
    for migration in migrations::NODE_MIGRATIONS {
        for engine in [Engine::Sqlite, Engine::Mariadb] {
            hashes.insert(
                format!("{}.{}.sql", migration.id, engine.as_str()),
                json!(canonical::hash(migration.sql(engine).as_bytes())),
            );
        }
    }
    hashes.insert(
        "import/0001-control.mariadb.sql".into(),
        json!(canonical::hash(CONTROL.as_bytes())),
    );
    hashes.insert(
        "import/store_schema.mariadb.sql".into(),
        json!(canonical::hash(
            crate::store::schema_ddl(Engine::Mariadb).as_bytes()
        )),
    );
    json!(hashes)
}

fn steps() -> Result<Vec<String>> {
    let mut steps: Vec<String> = crate::store::sql::statements(CONTROL)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    steps.push(crate::store::schema_ddl(Engine::Mariadb).to_owned());
    for (index, migration) in migrations::NODE_MIGRATIONS.iter().enumerate() {
        steps.extend(
            crate::store::sql::statements(migration.sql(Engine::Mariadb))?
                .into_iter()
                .map(str::to_owned),
        );
        steps.push(format!("INSERT INTO store_schema(name,version,applied_at) VALUES('node',{},0) ON DUPLICATE KEY UPDATE version=VALUES(version),applied_at=VALUES(applied_at)",index+1));
    }
    Ok(steps)
}

fn configure(config: &Contract) -> Result<MariadbStore> {
    let dsn = contract::text(&config.json, "dsn")?;
    let opts = mysql::Opts::from_url(dsn).map_err(|_| refusal("fixture_dsn_invalid"))?;
    if opts.get_socket().filter(|s| !s.is_empty()).is_none()
        || opts.get_pass().is_some_and(|p| !p.is_empty())
        || opts.get_db_name() != Some(contract::text(&config.json, "database")?)
        || opts.get_user().filter(|s| !s.is_empty()).is_none()
    {
        return Err(refusal(
            "fixture_requires_exact_socket_database_without_secret",
        ));
    }
    let mut profile = MariadbConfig::from_dsn(dsn);
    profile.connect_timeout =
        Duration::from_millis(contract::number(&config.json, "connect_timeout_ms")?);
    profile.lock_wait_timeout =
        Duration::from_secs(contract::number(&config.json, "lock_wait_timeout_seconds")?);
    let mut db = MariadbStore::open(&profile)
        .map_err(|_| refusal("fixture_backend_open_failed_or_durability_refused"))?;
    let mode = contract::text(&config.json, "session_sql_mode")?;
    if !mode
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b == b'_' || b == b',')
        || !mode
            .split(',')
            .any(|v| matches!(v, "STRICT_ALL_TABLES" | "STRICT_TRANS_TABLES"))
        || !mode.split(',').any(|v| v == "NO_AUTO_VALUE_ON_ZERO")
    {
        return Err(refusal("fixture_strict_zero_sql_mode_required"));
    }
    db.execute_batch(&format!("SET SESSION sql_mode='{mode}'"))?;
    warnings(&mut db)?;
    db.execute_batch("SET NAMES utf8mb4 COLLATE utf8mb4_nopad_bin")?;
    warnings(&mut db)?;
    db.execute_batch(&format!(
        "SET SESSION max_statement_time={}.{:03}",
        contract::number(&config.json, "statement_timeout_ms")? / 1000,
        contract::number(&config.json, "statement_timeout_ms")? % 1000
    ))?;
    warnings(&mut db)?;
    settings(&mut db, config)?;
    Ok(db)
}

fn settings(db: &mut dyn DurableStore, config: &Contract) -> Result<Json> {
    let rows=db.query("SELECT DATABASE(),VERSION(),CURRENT_USER(),@@GLOBAL.innodb_flush_log_at_trx_commit,@@GLOBAL.innodb_page_size,@@SESSION.sql_mode,@@SESSION.tx_isolation,@@SESSION.character_set_connection,@@SESSION.collation_connection,@@SESSION.innodb_strict_mode, (SELECT DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=DATABASE()), (SELECT DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=DATABASE()),@@GLOBAL.character_set_server,@@GLOBAL.collation_server,@@GLOBAL.innodb_default_row_format,@@SESSION.unique_checks,@@SESSION.foreign_key_checks",&[])?;
    checked_settings(rows, config)
}
fn checked_settings(rows: Vec<Row>, config: &Contract) -> Result<Json> {
    if rows.len() != 1 {
        return Err(refusal("fixture_effective_settings_unreadable"));
    }
    let row = &rows[0];
    if row.text(0)? != contract::text(&config.json, "database")?
        || row.text(1)? != contract::text(&config.json, "server_version")?
        || row.text(2)? != contract::text(&config.json, "current_user")?
        || row.integer(3)? != 1
        || row.integer(4)? != 16384
        || row.text(5)? != contract::text(&config.json, "session_sql_mode")?
        || row.text(6)? != "REPEATABLE-READ"
        || row.text(7)? != "utf8mb4"
        || row.text(8)? != "utf8mb4_nopad_bin"
        || row.integer(9)? != 1
        || row.text(10)? != contract::text(&config.json, "database_charset")?
        || row.text(11)? != contract::text(&config.json, "database_collation")?
        || !row.text(14)?.eq_ignore_ascii_case("dynamic")
        || row.integer(15)? != 1
        || row.integer(16)? != 1
    {
        return Err(refusal(
            "fixture_effective_identity_or_durability_gate_refused",
        ));
    }
    Ok(
        json!({"database":row.text(0)?,"server_version":row.text(1)?,"current_user":row.text(2)?,"flush_log_at_commit":row.integer(3)?,"innodb_page_size":row.integer(4)?,"sql_mode":row.text(5)?,"isolation":row.text(6)?,"charset":row.text(7)?,"collation":row.text(8)?,"innodb_strict_mode":row.integer(9)?,"database_charset":row.text(10)?,"database_collation":row.text(11)?,"server_charset":row.text(12)?,"server_collation":row.text(13)?,"default_row_format":row.text(14)?,"unique_checks":row.integer(15)?,"foreign_key_checks":row.integer(16)?}),
    )
}
fn commit_settings(tx: &mut dyn crate::store::Transaction, config: &Contract) -> Result<()> {
    config.boundary()?;
    let rows=tx.query("SELECT DATABASE(),VERSION(),CURRENT_USER(),@@GLOBAL.innodb_flush_log_at_trx_commit,@@GLOBAL.innodb_page_size,@@SESSION.sql_mode,@@SESSION.tx_isolation,@@SESSION.character_set_connection,@@SESSION.collation_connection,@@SESSION.innodb_strict_mode, (SELECT DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=DATABASE()), (SELECT DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=DATABASE()),@@GLOBAL.character_set_server,@@GLOBAL.collation_server,@@GLOBAL.innodb_default_row_format,@@SESSION.unique_checks,@@SESSION.foreign_key_checks",&[])?;
    checked_settings(rows, config)?;
    Ok(())
}
fn profile(settings: &Json) -> Json {
    // User/DB are fixture resource bindings, not portable capacity characteristics.
    let mut settings = settings.clone();
    if let Some(map) = settings.as_object_mut() {
        map.remove("database");
        map.remove("current_user");
    }
    settings
}

fn valid_node_shape(shape: &Json, plan: &Plan) -> Result<()> {
    let objects = shape["objects"]
        .as_array()
        .ok_or_else(|| refusal("target_shape_invalid"))?;
    let expected: std::collections::BTreeSet<String> = plan
        .tables
        .iter()
        .map(|t| t.name.clone())
        .chain([
            "store_schema".into(),
            super::MARKER.into(),
            super::CHECKPOINTS.into(),
        ])
        .collect();
    let actual: std::collections::BTreeSet<String> = objects
        .iter()
        .map(|v| v[0].as_str().unwrap_or_default().to_owned())
        .collect();
    if actual != expected
        || objects.len() != 41
        || objects
            .iter()
            .any(|v| v[1] != "BASE TABLE" || v[2] != "InnoDB" || v[3] != "Dynamic")
        || shape["triggers"] != json!([])
        || shape["routines"] != json!([])
        || shape["events"] != json!([])
        || shape["schema_record"] != json!([["node", 11, 0]])
    {
        return Err(refusal("target_inventory_engine_version_or_extra_objects"));
    }
    let expected_source = source::expected_shape()?;
    let widths: Json = serde_json::from_str(include_str!(
        "../../../fixtures/C/copy-v1/target-widths-v11.json"
    ))
    .map_err(|_| refusal("fixed_width_map_invalid"))?;
    let columns = shape["columns"]
        .as_array()
        .ok_or_else(|| refusal("target_shape_invalid"))?;
    for table in &plan.tables {
        let raw = expected_source["tables"][&table.name]["columns"]
            .as_array()
            .ok_or_else(|| refusal("source_shape_invalid"))?;
        let target: Vec<&Json> = columns.iter().filter(|v| v[0] == table.name).collect();
        let generated = if table.name == "network_allocations" {
            2
        } else {
            0
        };
        if target.len() != table.columns.len() + generated {
            return Err(refusal("target_unexpected_column"));
        }
        for (i, col) in raw.iter().enumerate() {
            let t = target[i];
            let is_text = col[2] == "TEXT";
            let pk = col[5].as_i64().unwrap_or(0) > 0;
            let nullable = col[3] == 0 && !pk;
            if t[1] != col[1]
                || t[2] != json!(i + 1)
                || t[5] != if nullable { json!("YES") } else { json!("NO") }
                || (is_text && (t[4] != "varchar" && t[4] != "longtext"))
                || (!is_text
                    && (t[4] != "bigint" || t[3].as_str().unwrap_or_default().contains("unsigned")))
                || (is_text && (t[10] != "utf8mb4_nopad_bin" || t[11] != "utf8mb4"))
            {
                return Err(refusal("target_portable_column_domain_mismatch"));
            }
            let default_matches = if col[4].is_null() {
                t[6].is_null() || (nullable && t[6] == "NULL")
            } else {
                t[6] == col[4]
            };
            let single_integer_pk = pk
                && !is_text
                && raw
                    .iter()
                    .filter(|c| c[5].as_i64().unwrap_or(0) > 0)
                    .count()
                    == 1;
            if !default_matches
                || t[8]
                    != if single_integer_pk {
                        json!("auto_increment")
                    } else {
                        json!("")
                    }
            {
                return Err(refusal("target_default_or_identity_domain_mismatch"));
            }
            let key = format!("{}.{}", table.name, col[1].as_str().unwrap_or_default());
            if let Some(width) = widths["widths"][&key].as_u64() {
                if t[4] != "varchar" || t[9] != width {
                    return Err(refusal("target_varchar_bound_mismatch"));
                }
            } else if is_text && t[4] != "longtext" {
                return Err(refusal("target_longtext_domain_mismatch"));
            }
        }
        if generated == 2 {
            for (index, name) in ["live_ip", "live_universe_uuid"].iter().enumerate() {
                let c = target[table.columns.len() + index];
                if c[1] != *name
                    || c[4] != "varchar"
                    || c[10] != "utf8mb4_nopad_bin"
                    || c[11] != "utf8mb4"
                    || !c[8]
                        .as_str()
                        .unwrap_or_default()
                        .contains("VIRTUAL GENERATED")
                {
                    return Err(refusal("target_generated_column_mismatch"));
                }
            }
        }
        let indexes = shape["indexes"]
            .as_array()
            .ok_or_else(|| refusal("target_shape_invalid"))?;
        let mut pkcols: Vec<(i64, String)> = raw
            .iter()
            .filter_map(|c| {
                c[5].as_i64()
                    .filter(|n| *n > 0)
                    .map(|n| (n, c[1].as_str().unwrap_or_default().to_owned()))
            })
            .collect();
        pkcols.sort();
        let actualpk: Vec<String> = indexes
            .iter()
            .filter(|v| v[0] == table.name && v[1] == "PRIMARY")
            .map(|v| v[4].as_str().unwrap_or_default().to_owned())
            .collect();
        if actualpk != pkcols.into_iter().map(|(_, n)| n).collect::<Vec<_>>() {
            return Err(refusal("target_primary_key_mismatch"));
        }
        let source_indexes = expected_source["tables"][&table.name]["indexes"]
            .as_array()
            .ok_or_else(|| refusal("source_shape_invalid"))?;
        let mut expected_unique = Vec::new();
        for index in source_indexes {
            let def = &index["definition"];
            if def[2] != 1 || def[3] == "pk" {
                continue;
            }
            let names = if table.name == "network_allocations" && def[4] == 1 {
                match def[1].as_str().unwrap_or_default() {
                    "network_allocations_live_ip" => vec!["live_ip".to_owned()],
                    "network_allocations_live_universe" => vec!["live_universe_uuid".to_owned()],
                    _ => return Err(refusal("source_partial_index_mapping_missing")),
                }
            } else {
                index["columns"]
                    .as_array()
                    .ok_or_else(|| refusal("source_index_shape_invalid"))?
                    .iter()
                    .filter(|v| v[5] == 1)
                    .map(|v| {
                        v[2].as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| refusal("source_expression_index_refused"))
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            expected_unique.push(names);
        }
        let mut actual_unique = std::collections::BTreeMap::<String, Vec<String>>::new();
        for index in indexes
            .iter()
            .filter(|v| v[0] == table.name && v[1] != "PRIMARY")
        {
            if index[2] != 0 {
                return Err(refusal("target_unexpected_nonunique_index"));
            }
            actual_unique
                .entry(index[1].as_str().unwrap_or_default().to_owned())
                .or_default()
                .push(index[4].as_str().unwrap_or_default().to_owned());
        }
        let mut actual_unique: Vec<Vec<String>> = actual_unique.into_values().collect();
        expected_unique.sort();
        actual_unique.sort();
        if actual_unique != expected_unique {
            return Err(refusal("target_unique_constraint_mapping_mismatch"));
        }
    }
    for index in shape["indexes"]
        .as_array()
        .ok_or_else(|| refusal("target_shape_invalid"))?
    {
        if !index[5].is_null() || index[6] != "BTREE" {
            return Err(refusal("target_prefix_or_index_type_refused"));
        }
    }
    if shape["constraints"]
        .as_array()
        .ok_or_else(|| refusal("target_shape_invalid"))?
        .iter()
        .any(|v| {
            plan.tables.iter().any(|t| v[0] == t.name)
                && !matches!(v[2].as_str(), Some("PRIMARY KEY" | "UNIQUE"))
        })
    {
        return Err(refusal("target_unexpected_node_constraint"));
    }
    if shape["key_columns"]
        .as_array()
        .ok_or_else(|| refusal("target_shape_invalid"))?
        .iter()
        .any(|v| !v[4].is_null() || !v[5].is_null())
    {
        return Err(refusal("target_unexpected_foreign_key"));
    }
    Ok(())
}

struct ImportContext<'a> {
    db: MariadbStore,
    contract: &'a Contract,
    plan: &'a Plan,
    plan_sha: String,
    snapshot_sha: String,
    source_schema_hex: String,
    identity_sha: String,
    marker_only_probe: Option<Json>,
}
impl<'a> ImportContext<'a> {
    fn new(contract: &'a Contract, plan: &'a Plan, seal: &Json) -> Result<Self> {
        contract::check_binary_pin(&contract.json)?;
        if contract.json["SQL_hashes"] != sql_hashes() {
            return Err(refusal("fixture_sql_source_pins_mismatch"));
        }
        let (source_schema_hex,snapshot_sha,plan_sha)=plan_binding(contract,plan,seal)?;
        let identity_sha = contract.target_identity()?;
        contract.boundary()?;
        let db = configure(contract)?;
        Ok(Self {
            db,
            contract,
            plan,
            plan_sha,
            snapshot_sha,
            source_schema_hex,
            identity_sha,
            marker_only_probe: None,
        })
    }
    fn gate(&mut self) -> Result<Json> {
        self.contract.boundary()?;
        settings(&mut self.db, self.contract)
    }
    fn state(&mut self) -> Result<Vec<Row>> {
        self.db.query("SELECT protocol_version,migration_id,plan_sha256,snapshot_sha256,source_manifest_sha256,source_schema_hex,target_identity_sha256,phase,serving,ddl_step,verification_root FROM podmesh_import_state ORDER BY singleton",&[])
    }
    fn initialize(&mut self) -> Result<()> {
        let rows = self.state()?;
        if rows.is_empty() {
            self.db.execute("INSERT INTO podmesh_import_state(singleton,protocol_version,migration_id,plan_sha256,snapshot_sha256,source_manifest_sha256,source_schema_hex,target_identity_sha256,phase,serving,ddl_step,verification_root) VALUES(1,1,?,?,?,?,?,?,'INCOMPLETE',0,1,NULL)",&[contract::text(&self.contract.json,"migration_id")?.into(),self.plan_sha.clone().into(),self.snapshot_sha.clone().into(),self.contract.source_sha256.clone().into(),self.source_schema_hex.clone().into(),self.identity_sha.clone().into()])?;
            warnings(&mut self.db)?;
            self.contract.event("after_incomplete_insert")?;
        } else {
            self.validate_state(false)?;
        }
        Ok(())
    }
    fn validate_state(&mut self, restored: bool) -> Result<()> {
        let rows = self.state()?;
        if rows.len() != 1 {
            return Err(refusal("import_state_empty_or_malformed"));
        }
        let row = &rows[0];
        if row.integer(0)? != 1
            || row.text(1)? != contract::text(&self.contract.json, "migration_id")?
            || row.text(2)? != self.plan_sha
            || row.text(3)? != self.snapshot_sha
            || row.text(4)? != self.contract.source_sha256
            || row.text(5)? != self.source_schema_hex
            || (!restored && row.text(6)? != self.identity_sha)
            || !matches!(row.text(7)?, "INCOMPLETE" | "COMPLETE")
            || row.integer(8)? != 0
            || row.integer(9)? < 1
        {
            return Err(refusal("import_state_binding_or_nonserving_mismatch"));
        }
        let complete = row.text(7)? == "COMPLETE";
        if (!complete && !row.value(10)?.is_null())
            || (complete
                && row
                    .value(10)?
                    .text()
                    .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .is_none())
        {
            return Err(refusal("import_verification_root_phase_mismatch"));
        }
        Ok(())
    }
    fn apply_step(&mut self, index: usize, sql: &str) -> Result<()> {
        self.gate()?;
        self.db.execute_batch(sql)?;
        warnings(&mut self.db)?;
        if index == 0 {
            if self.contract.json["role"] == "capability" {
                self.marker_only_probe = Some(self.normal_refusals(false)?);
            }
            self.contract.event("after_marker_create")?;
            self.initialize()?;
        } else {
            self.db.execute("UPDATE podmesh_import_state SET ddl_step=? WHERE singleton=1 AND phase='INCOMPLETE'",&[i64::try_from(index+1).map_err(|_|refusal("resource_counter_overflow"))?.into()])?;
            warnings(&mut self.db)?;
        }
        self.contract
            .event(&format!("after_each_node_DDL:{}", index + 1))?;
        if crate::store::sql::without_leading_comments(sql).starts_with("ALTER TABLE") {
            self.contract
                .event(&format!("after_each_successor_ALTER:{}", index + 1))?;
        }
        if sql.starts_with("INSERT INTO store_schema") {
            self.contract.event(&format!(
                "after_each_migration_version_checkpoint:{}",
                index + 1
            ))?;
        }
        if index == 1 {
            self.contract.event("after_control_tables")?;
        }
        Ok(())
    }
    fn capability_schema(&mut self) -> Result<Vec<Json>> {
        let initial = shape(&mut self.db)?;
        let mut states = vec![json!({"step":0,"shape_sha256":digest(&initial)?})];
        if initial["objects"] != json!([])
            || initial["triggers"] != json!([])
            || initial["routines"] != json!([])
            || initial["events"] != json!([])
        {
            return Err(refusal("capability_requires_fresh_empty_database"));
        }
        for (index, sql) in steps()?.iter().enumerate() {
            self.apply_step(index, sql)?;
            states.push(json!({"step":index+1,"shape_sha256":digest(&shape(&mut self.db)?)?}));
        }
        valid_node_shape(&shape(&mut self.db)?, self.plan)?;
        Ok(states)
    }
    fn resume_schema(&mut self, capability: &Json) -> Result<()> {
        let actual = shape(&mut self.db)?;
        let actual_hash = digest(&actual)?;
        let stages = capability["stages"]
            .as_array()
            .ok_or_else(|| refusal("capability_stages_missing"))?;
        let statements = steps()?;
        let start = select_schema_stage(stages, &actual_hash, statements.len())?;
        if start > 0 {
            if start == 1 {
                self.initialize()?;
            } else {
                self.validate_state(false)?;
            }
        }
        if start > 0 {
            let state = self.state()?;
            let recorded = state[0].integer(9)?;
            if recorded != start as i64 && recorded != std::cmp::max(1, start as i64 - 1) {
                return Err(refusal(
                    "import_ddl_checkpoint_not_current_or_last_unacked_step",
                ));
            }
            if state[0].text(7)? == "COMPLETE" && start != statements.len() {
                return Err(refusal("complete_marker_on_partial_schema"));
            }
        }

        for (index, sql) in statements.iter().enumerate().skip(start) {
            self.apply_step(index, sql)?;
            let actual = digest(&shape(&mut self.db)?)?;
            validate_next_schema_stage(stages, index + 1, &actual)?;
        }
        let final_shape = shape(&mut self.db)?;
        if final_shape != capability["target_shape"] {
            return Err(refusal("target_final_schema_mismatch"));
        }
        valid_node_shape(&final_shape, self.plan)?;
        Ok(())
    }
    fn bounded_table(&mut self, table: &Table) -> Result<()> {
        let quoted = source::quoted(&table.name)?;
        let count = self
            .db
            .query(&format!("SELECT COUNT(*) FROM {quoted}"), &[])?;
        if count.len() != 1
            || count[0].integer(0)? < 0
            || count[0].integer(0)? as u64 > self.contract.caps.table_rows
            || count[0].integer(0)? as usize > table.rows.len()
        {
            return Err(refusal("target_table_row_bound_exceeded"));
        }
        let declared = source::expected_shape()?;
        let columns = declared["tables"][&table.name]["columns"]
            .as_array()
            .ok_or_else(|| refusal("source_shape_invalid"))?;
        let mut total = 0u64;
        for column in columns {
            let name = source::quoted(
                column[1]
                    .as_str()
                    .ok_or_else(|| refusal("source_shape_invalid"))?,
            )?;
            let expression = if column[2] == "TEXT" {
                format!("OCTET_LENGTH({name})")
            } else {
                format!("IF({name} IS NULL,0,8)")
            };
            let rows=self.db.query(&format!("SELECT CAST(COALESCE(SUM({expression}),0) AS SIGNED),CAST(COALESCE(MAX({expression}),0) AS SIGNED) FROM {quoted}"),&[])?;
            if rows.len() != 1 || rows[0].integer(0)? < 0 || rows[0].integer(1)? < 0 {
                return Err(refusal("target_resource_counter_invalid"));
            }
            let bytes = rows[0].integer(0)? as u64;
            let max = rows[0].integer(1)? as u64;
            total = total
                .checked_add(bytes)
                .ok_or_else(|| refusal("resource_counter_overflow"))?;
            if total > self.contract.caps.table_bytes || max > self.contract.caps.value_bytes {
                return Err(refusal("target_scalar_bound_exceeded"));
            }
        }
        Ok(())
    }
    fn tables(&mut self) -> Result<Vec<Json>> {
        let mut reports = Vec::new();
        self.validate_state(false)?;
        if self.state()?[0].text(7)? == "COMPLETE" {
            for table in &self.plan.tables {
                self.bounded_table(table)?;
                copy::verify_table(&mut self.db, table)?;
            }
            return Ok(reports);
        }
        for table in &self.plan.tables {
            self.gate()?;
            self.validate_state(false)?;
            self.bounded_table(table)?;
            let contract = self.contract;
            let copied = copy::copy_table(
                &mut self.db,
                table,
                true,
                &mut |point| contract.event(point),
                &mut |tx| commit_settings(tx, contract),
            )?;
            reports.push(json!({"table":table.name,"count":table.rows.len(),"canonical_sha256":table.hash,"copied_this_attempt":copied}));
            self.contract
                .event(&format!("between_tables:{}", table.name))?;
        }
        Ok(reports)
    }
    fn normal_refusals(&mut self, require_data: bool) -> Result<Json> {
        let before = shape(&mut self.db)?;
        for result in [
            super::guard_normal(&mut self.db),
            migrations::apply(&mut self.db).map(|_| ()),
            crate::store::ensure_schema_table(&mut self.db),
            crate::store::bootstrap(&mut self.db, "node", 11),
        ] {
            let err = result
                .err()
                .ok_or_else(|| refusal("normal_marked_schema_admission_succeeded"))?;
            if !err.message.contains("import_non_serving") {
                return Err(refusal("normal_marker_not_named_refusal"));
            }
        }
        let mut mariadb_profile =
            MariadbConfig::from_dsn(contract::text(&self.contract.json, "dsn")?);
        mariadb_profile.connect_timeout =
            Duration::from_millis(contract::number(&self.contract.json, "connect_timeout_ms")?);
        mariadb_profile.lock_wait_timeout = Duration::from_secs(contract::number(
            &self.contract.json,
            "lock_wait_timeout_seconds",
        )?);
        let profile = crate::store::StoreConfig {
            engine: Engine::Mariadb,
            mariadb: mariadb_profile,
            ..crate::store::StoreConfig::default()
        };
        let err = crate::store::open(&profile)
            .err()
            .ok_or_else(|| refusal("normal_marked_store_open_succeeded"))?;
        if !err.message.contains("import_non_serving") {
            return Err(refusal("normal_marker_not_named_refusal"));
        }
        let dir = Path::new(contract::text(
            &self.contract.json,
            "admission_probe_directory",
        )?);
        if dir.exists() {
            return Err(refusal("admission_probe_directory_must_not_exist"));
        }
        let err = crate::open_node_store(dir, &profile)
            .err()
            .ok_or_else(|| refusal("normal_public_node_admission_succeeded"))?;
        if !err.to_string().contains("import_non_serving") || dir.exists() {
            return Err(refusal(
                "public_node_marker_refusal_or_fs_preservation_failed",
            ));
        }
        if shape(&mut self.db)? != before {
            return Err(refusal("normal_marker_probe_mutated_schema"));
        }
        if require_data {
            for table in &self.plan.tables {
                self.bounded_table(table)?;
                copy::verify_table(&mut self.db, table)?;
            }
        }
        let state = self.state()?;
        let phase = state
            .first()
            .map(|row| row.text(7))
            .transpose()?
            .unwrap_or("EMPTY");
        Ok(
            json!({"phase":phase,"guard":true,"migrations_apply":true,"ensure_schema_table":true,"bootstrap":true,"store_open":true,"public_open_node_store":true,"runtime_directory_created":false,"schema_unchanged":true,"all38_typed_rows_unchanged":require_data}),
        )
    }

    fn generated(&mut self) -> Result<Json> {
        let rows=self.db.query("SELECT ip,universe_uuid,released_at,live_ip,live_universe_uuid FROM network_allocations",&[])?;
        for row in &rows {
            let live = row.value(2)?.is_null();
            for (source, generated) in [(0, 3), (1, 4)] {
                if row.value(generated)?
                    != if live {
                        row.value(source)?
                    } else {
                        &Value::Null
                    }
                {
                    return Err(refusal("target_generated_value_mismatch"));
                }
            }
        }
        Ok(
            json!({"table":"network_allocations","rows":rows.len(),"generated_columns":["live_ip","live_universe_uuid"],"exact":true}),
        )
    }
    fn verify(&mut self, expected_shape: &Json, restored: bool) -> Result<Json> {
        self.gate()?;
        self.validate_state(restored)?;
        let actual = shape(&mut self.db)?;
        if actual != *expected_shape {
            return Err(refusal("target_schema_changed_before_completion"));
        }
        valid_node_shape(&actual, self.plan)?;
        let mut tables = Vec::new();
        for table in &self.plan.tables {
            self.gate()?;
            self.bounded_table(table)?;
            copy::verify_table(&mut self.db, table)?;
            self.contract
                .event(&format!("during_verification:{}", table.name))?;
            tables.push(table.manifest());
        }
        let checkpoints=self.db.query("SELECT table_name,row_count,canonical_sha256 FROM podmesh_import_tables ORDER BY table_name",&[])?;
        if checkpoints.len() != 38 {
            return Err(refusal("import_checkpoint_inventory_mismatch"));
        }
        for row in checkpoints {
            let table = self
                .plan
                .tables
                .iter()
                .find(|t| t.name == row.text(0).unwrap_or_default())
                .ok_or_else(|| refusal("unexpected_table_checkpoint"))?;
            if row.integer(1)? != table.rows.len() as i64 || row.text(2)? != table.hash {
                return Err(refusal("table_checkpoint_mismatch"));
            }
        }
        let generated = self.generated()?;
        let integrity = self.db.integrity_check()?;
        if !integrity.ok {
            return Err(refusal("target_integrity_failed"));
        }
        self.gate()?;
        Ok(
            json!({"tables":tables,"typed_full_vectors_compared":true,"schema_sha256":digest(&actual)?,"generated":generated,"integrity":integrity.to_json(),"source_schema_hex":self.source_schema_hex,"snapshot_sha256":self.snapshot_sha,"plan_sha256":self.plan_sha,"source_manifest_sha256":self.contract.source_sha256}),
        )
    }
    fn complete(&mut self, verification: &Json) -> Result<String> {
        self.gate()?;
        self.validate_state(false)?;
        let root = digest(verification)?;
        let state = self.state()?;
        if state[0].text(7)? == "COMPLETE" {
            if state[0].value(10)?.text() != Some(&root) {
                return Err(refusal("complete_verification_root_mismatch"));
            }
            return Ok(root);
        }
        self.contract.event("before_complete_commit")?;
        let mut tx = self.db.transaction()?;
        if tx.execute("UPDATE podmesh_import_state SET phase='COMPLETE',verification_root=? WHERE singleton=1 AND phase='INCOMPLETE' AND serving=0",&[root.clone().into()])?!=1{return Err(refusal("completion_state_count_mismatch"));}
        if !tx.query("SHOW WARNINGS", &[])?.is_empty() {
            return Err(refusal("target_sql_warning_refused"));
        }
        commit_settings(tx.as_mut(), self.contract)?;
        tx.commit()?;
        self.contract
            .event("after_complete_commit_before_receipt")?;
        Ok(root)
    }
}

/// Offline protocol manifest: enables A to pin contracts before creating any fixture.
pub fn protocol() -> Result<Json> {
    let statements = steps()?;
    let mut killpoints = vec![
        "after_source_bundle_clone".to_owned(),
        "after_snapshot_seal".to_owned(),
        "after_marker_create".to_owned(),
        "after_incomplete_insert".to_owned(),
        "after_control_tables".to_owned(),
        "before_complete_commit".to_owned(),
        "after_complete_commit_before_receipt".to_owned(),
    ];
    for (index, sql) in statements.iter().enumerate() {
        killpoints.push(format!("after_each_node_DDL:{}", index + 1));
        if crate::store::sql::without_leading_comments(sql).starts_with("ALTER TABLE") {
            killpoints.push(format!("after_each_successor_ALTER:{}", index + 1));
        }
        if sql.starts_with("INSERT INTO store_schema") {
            killpoints.push(format!(
                "after_each_migration_version_checkpoint:{}",
                index + 1
            ));
        }
    }
    for table in migrations::node_tables()? {
        for prefix in [
            "after_row_before_table_commit",
            "before_table_commit",
            "after_table_commit_before_ack",
            "between_tables",
            "during_verification",
        ] {
            killpoints.push(format!("{prefix}:{table}"));
        }
    }
    Ok(
        json!({"format":"podmesh-c02-protocol/1","status":"SOURCE_PROTOCOL_ONLY_NO_SERVER_PROOF","source_version":10,"target_version":11,"node_tables":migrations::node_tables()?,"SQL_hashes":sql_hashes(),"DDL_steps":statements.len(),"schema_stage_count":statements.len()+1,"killpoints":killpoints,"canonical_contract_sha256":contract::CANONICAL_SHA,"resource_caps_contract_sha256":contract::CAPS_SHA,"required_contract_fields":["schema","status","server_execution_authorized","role","dsn","database","server_version","current_user","migration_id","writer_lock","source_manifest_path","source_manifest_sha256","resource_caps","resource_caps_contract_sha256","canonical_contract_sha256","SQL_hashes","source_commit","binary_sha256","target_resource_identity","connect_timeout_ms","lock_wait_timeout_seconds","statement_timeout_ms","maximum_campaign_seconds","session_sql_mode","database_charset","database_collation"],"target_resource_identity_required_fields":["container_id","volume_name","image_id"],"migration_id_scope":"campaign-global capability/copy/seeded-restore/postcopy-restore; distinct resource identity per target; never identity rebind/adoption","control_file_modes":["0600","0400"],"conditional_contract_fields":{"capability":["admission_probe_directory"],"copy":["capability_receipt","seeded_restore_receipt"],"copy_resume":["capability_receipt","seeded_restore_receipt"],"seeded_restore":["capability_receipt","restore_origin_receipt","dump_restore_receipt","dump_restore_helper_sha256"],"restored_copy":["capability_receipt","restore_origin_receipt","dump_restore_receipt","dump_restore_helper_sha256"],"killpoint":["killpoint","kill_event_path"]},"roles":{"capability":"fresh empty target; marker-first candidate schema capacity, all38typed roundtrip and fullindex probes","seeded_restore":"read-only owning-store synthetic restoration before copy; capability receipt and restore_origin_receipt required","copy":"fresh empty DB; capability+seeded_restore receipts required before DDL","copy_resume":"known partial/full schema bound to same plan/source/resource; COMPLETE stays non-serving","restored_copy":"read-only postcopy restore; new A resource identity; capability+copy origin receipt; no identity rebind","negative_gate":"A isolated durability-negative fixture only; C never changes GLOBAL"},"limits":"Synthetic bounded domains only. Negative IDs copied but auto-ID continuation, unknown/deleted highwater and exhaustion remain C03 limitations; real keys/authority remain outside portable store.","serving":false}),
    )
}

pub struct Request<'a> {
    pub contract_path: &'a Path,
    pub snapshot_path: &'a Path,
    pub seal_path: &'a Path,
    pub output: &'a Path,
}

/// Missing/closed contracts refuse before connecting; no environment DSN or silent skips.
pub fn run(mode: &str, request: &Request<'_>) -> Result<Json> {
    let roles: &[&str] = match mode {
        "capability" => &["capability"],
        "copy" => &["copy", "negative_gate"],
        "resume" => &["copy_resume"],
        "restore-verify" => &["seeded_restore", "restored_copy"],
        "verify" => &["copy_resume"],
        _ => return Err(refusal("unknown_import_operation")),
    };
    let contract = Contract::open(request.contract_path, roles)?;
    let (seal, _) = contract::read_json(request.seal_path, 1_048_576)?;
    if seal["source_manifest_sha256"] != digest(&contract.source)? {
        return Err(refusal("snapshot_source_manifest_mismatch"));
    }
    let size = seal["snapshot_bytes"]
        .as_u64()
        .ok_or_else(|| refusal("snapshot_size_missing"))?;
    contract.caps.check(size, 0, 0, 0)?;
    let reader = snapshot::open_sealed_controlled(request.snapshot_path, &seal, &mut ||contract.clock_boundary())?;
    let plan = source::inspect(&reader, &contract.caps, size)?;
    if json!(plan.tables.iter().map(Table::manifest).collect::<Vec<_>>())
        != contract.source["tables"]
        || plan.shape_hash != contract.source["source_shape_sha256"]
    {
        return Err(refusal("synthetic_source_manifest_mismatch"));
    }
    let (_,_,expected_plan_sha)=plan_binding(&contract,&plan,&seal)?;
    // A campaign-global migration ID binds capability/copy/restore; resource IDs differ.
    // Validate prerequisite receipts BEFORE opening the target or any target DDL.
    let capability = if mode == "capability" {
        None
    } else {
        Some(contract.receipt("capability_receipt", "podmesh-c02-capability/1")?)
    };
    let seeded = if matches!(mode, "copy" | "resume" | "verify") {
        Some(contract.receipt("seeded_restore_receipt", "podmesh-c02-restoration/1")?)
    } else {
        None
    };
    if let Some(cap) = &capability {
        prerequisite_provenance(cap, &contract.json, &expected_plan_sha)?;
        if cap["SQL_hashes"] != sql_hashes()
            || cap["source_manifest_sha256"] != contract.source_sha256
            || cap["tables"] != contract.source["tables"]
        {
            return Err(refusal("capability_source_or_sql_mismatch"));
        }
    }
    if let Some(restore) = &seeded {
        prerequisite_provenance(restore, &contract.json, &expected_plan_sha)?;
        if restore["tables"] != contract.source["tables"]
            || restore["SQL_hashes"] != sql_hashes()
            || restore["restoration_kind"] != "seeded_restore"
        {
            return Err(refusal("seeded_restore_prerequisite_mismatch"));
        }
    }
    let origin_receipt = if mode == "restore-verify" {
        let origin = contract.receipt(
            "restore_origin_receipt",
            if contract.json["role"] == "seeded_restore" {
                "podmesh-c02-capability/1"
            } else {
                "podmesh-c02-copy/1"
            },
        )?;
        prerequisite_provenance(&origin, &contract.json, &expected_plan_sha)?;
        if origin["migration_id"] != contract.json["migration_id"] {
            return Err(refusal("restoration_origin_migration_id_mismatch"));
        }
        Some(origin)
    } else {
        None
    };
    let dump_restore = origin_receipt.as_ref().map(|origin|checked_dump_restore(&contract,origin)).transpose()?;
    let mut report_file = snapshot::create_file(request.output)?;
    use std::io::{Seek, SeekFrom, Write};
    report_file
        .write_all(b"{\"status\":\"ATTEMPT_STARTED_NOT_PASS\"}\n")
        .map_err(|_| refusal("receipt_write_failed"))?;
    report_file
        .sync_all()
        .map_err(|_| refusal("receipt_sync_failed"))?;
    snapshot::sync_directory(
        request
            .output
            .parent()
            .ok_or_else(|| refusal("invalid_receipt_parent"))?,
    )?;
    let mut ctx = ImportContext::new(&contract, &plan, &seal)?;
    let effective = ctx.gate()?;
    if let Some(cap) = &capability {
        if cap["capacity_profile"] != profile(&effective) {
            return Err(refusal("capability_effective_profile_mismatch"));
        }
    }
    if mode == "copy" && shape(&mut ctx.db)?["objects"] != json!([]) {
        return Err(refusal(
            "copy_requires_fresh_empty_target_use_explicit_resume",
        ));
    }
    let report = if mode == "capability" {
        let stages = ctx.capability_schema()?;
        let tables = ctx.tables()?;
        let target_shape = shape(&mut ctx.db)?;
        let probes = capability_probes(&mut ctx)?;
        let incomplete_admission = ctx.normal_refusals(true)?;
        let verification = ctx.verify(&target_shape, false)?;
        let root = ctx.complete(&verification)?;
        let complete_admission = ctx.normal_refusals(true)?;
        json!({"format":"podmesh-c02-capability/1","status":"PASS","scope":"SYNTHETIC_CAPABILITY_NOT_FULL_C02","stages":stages,"target_shape":target_shape,"capacity_profile":profile(&effective),"table_copy":tables,"tables":contract.source["tables"],"probes":probes,"normal_admission_probes":[ctx.marker_only_probe,incomplete_admission,complete_admission],"verification":verification,"verification_root":root})
    } else if mode == "restore-verify" {
        let cap = capability.as_ref().unwrap();
        let origin = origin_receipt
            .as_ref()
            .ok_or_else(|| refusal("restoration_origin_receipt_missing"))?;
        let verification = ctx.verify(&cap["target_shape"], true)?;
        let state = ctx.state()?;
        let root = digest(&verification)?;
        if state[0].text(7)? != "COMPLETE"
            || state[0].value(10)?.text() != Some(&root)
            || origin["verification_root"] != root
            || origin["plan_sha256"] != ctx.plan_sha
            || origin["target_resource_identity"] == contract.json["target_resource_identity"]
            || origin["database"] == contract.json["database"]
            || state[0].text(6)? != contract::text(origin, "target_identity_sha256")?
        {
            return Err(refusal(
                "restoration_lineage_new_resource_or_complete_mismatch",
            ));
        }
        json!({"format":"podmesh-c02-restoration/1","status":"PASS","restoration_kind":contract.json["role"],"tables":contract.source["tables"],"verification":verification,"verification_root":root,"origin_receipt":contract.json["restore_origin_receipt"],"origin_target_identity_sha256":state[0].text(6)?,"serving":false,"identity_rebound":false})
    } else {
        let cap = capability.as_ref().unwrap();
        if mode == "verify" {
            ctx.validate_state(false)?;
        } else {
            ctx.resume_schema(cap)?;
        }
        let copied = if mode == "verify" {
            json!([])
        } else {
            json!(ctx.tables()?)
        };
        let verification = ctx.verify(&cap["target_shape"], false)?;
        let root = if mode == "verify" {
            let root = digest(&verification)?;
            let state = ctx.state()?;
            if state[0].text(7)? != "COMPLETE" || state[0].value(10)?.text() != Some(&root) {
                return Err(refusal("complete_verification_root_mismatch"));
            }
            root
        } else {
            ctx.complete(&verification)?
        };
        json!({"format":"podmesh-c02-copy/1","status":"PASS","operation":mode,"table_copy":copied,"tables":contract.source["tables"],"verification":verification,"verification_root":root,"serving":false,"authority_or_host_identity_changed":false})
    };
    contract.boundary()?;
    let mut report = report
        .as_object()
        .cloned()
        .ok_or_else(|| refusal("receipt_object_invalid"))?;
    for (k, v) in [
        ("SQL_hashes", sql_hashes()),
        ("migration_id", contract.json["migration_id"].clone()),
        ("resource_caps", contract.json["resource_caps"].clone()),
        ("canonical_contract_sha256", json!(contract::CANONICAL_SHA)),
        ("resource_caps_contract_sha256", json!(contract::CAPS_SHA)),
        ("source_manifest_sha256", json!(contract.source_sha256)),
        ("source_commit", contract.json["source_commit"].clone()),
        ("binary_sha256", contract.json["binary_sha256"].clone()),
        ("contract_sha256", json!(contract.sha256)),
        ("effective_settings", effective),
        (
            "target_resource_identity",
            contract.json["target_resource_identity"].clone(),
        ),
        ("target_identity_sha256", json!(ctx.identity_sha)),
        ("database", contract.json["database"].clone()),
        ("plan_sha256", json!(ctx.plan_sha)),
        ("snapshot_sha256", json!(ctx.snapshot_sha)),
        ("reader_identity", reader.identity.clone()),
        ("serving", json!(false)),
    ] {
        report.insert(k.into(), v);
    }
    if let Some(admin)=dump_restore {
        report.insert("dump_restore_receipt".into(),contract.json["dump_restore_receipt"].clone());
        report.insert("dump_restore_helper_sha256".into(),contract.json["dump_restore_helper_sha256"].clone());
        report.insert("admin_dump_sha256".into(),admin["dump_sha256"].clone());
        report.insert("admin_status".into(),admin["status"].clone());
    }
    let report = json!(report);
    report_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| refusal("receipt_seek_failed"))?;
    report_file
        .set_len(0)
        .map_err(|_| refusal("receipt_truncate_failed"))?;
    serde_json::to_writer_pretty(&mut report_file, &report)
        .map_err(|_| refusal("receipt_write_failed"))?;
    report_file
        .write_all(b"\n")
        .map_err(|_| refusal("receipt_write_failed"))?;
    report_file
        .sync_all()
        .map_err(|_| refusal("receipt_sync_failed"))?;
    snapshot::sync_directory(
        request
            .output
            .parent()
            .ok_or_else(|| refusal("invalid_receipt_parent"))?,
    )?;
    Ok(report)
}

fn capability_probes(ctx: &mut ImportContext<'_>) -> Result<Json> {
    let mut tests = Vec::new();
    let declared = source::expected_shape()?;
    // Every node table actually rejects its duplicated PK, preserving exact before/after rows.
    for table in &ctx.plan.tables {
        ctx.gate()?;
        let mut tx = ctx.db.transaction()?;
        let err = tx
            .execute(&table.insert()?, &table.rows[0])
            .err()
            .ok_or_else(|| refusal("capability_duplicate_primary_key_accepted"))?;
        tx.rollback()?;
        copy::verify_table(&mut ctx.db, table)?;
        tests.push(json!({"table":table.name,"probe":"duplicate_primary_key","fault":err.fault.as_str(),"rows_unchanged":true}));
        if err.fault != crate::store::Fault::Integrity {
            return Err(refusal("capability_duplicate_wrong_fault"));
        }
        let columns = declared["tables"][&table.name]["columns"]
            .as_array()
            .ok_or_else(|| refusal("source_shape_invalid"))?;
        let pkcount = columns
            .iter()
            .filter(|c| c[5].as_i64().unwrap_or(0) > 0)
            .count();
        let nullcol = columns
            .iter()
            .position(|c| {
                (c[3] == 1 || c[5].as_i64().unwrap_or(0) > 0)
                    && !(c[2] == "INTEGER" && c[5].as_i64().unwrap_or(0) > 0 && pkcount == 1)
            })
            .ok_or_else(|| refusal("capability_nonauto_notnull_column_missing"))?;
        let mut values = columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if c[2] == "INTEGER" {
                    Value::Integer(13)
                } else {
                    Value::Text(format!("null-probe-{i}"))
                }
            })
            .collect::<Vec<_>>();
        values[nullcol] = Value::Null;
        let mut tx = ctx.db.transaction()?;
        let err = tx
            .execute(&table.insert()?, &values)
            .err()
            .ok_or_else(|| refusal("capability_null_nonnullable_accepted"))?;
        if err.fault != crate::store::Fault::Integrity {
            return Err(refusal("capability_null_wrong_fault"));
        }
        tx.rollback()?;
        copy::verify_table(&mut ctx.db, table)?;
        tests.push(json!({"table":table.name,"probe":"null_nonnullable","column":table.columns[nullcol],"fault":err.fault.as_str(),"rows_unchanged":true}));
    }
    // Exact full 3072-byte index boundary, no prefix, no truncation; probes rollback.
    for length in [768usize, 769] {
        ctx.gate()?;
        let key = "🧭".repeat(length);
        let mut tx = ctx.db.transaction()?;
        let result = tx.execute(
            "INSERT INTO metadata(`key`,value) VALUES(?,?)",
            &[key.clone().into(), "synthetic-index-capacity".into()],
        );
        let success = result.is_ok();
        let warning_rows = tx.query("SHOW WARNINGS", &[])?;
        if (length == 768 && (!success || !warning_rows.is_empty())) || (length == 769 && success) {
            return Err(refusal("capability_full_index_boundary_failed"));
        }
        let failure_fault = result.as_ref().err().map(|err| err.fault.as_str());
        if length == 769
            && result
                .as_ref()
                .err()
                .is_none_or(|err| err.fault != crate::store::Fault::Type)
        {
            return Err(refusal("capability_overwidth_wrong_fault"));
        }

        if success {
            let rows = tx.query(
                "SELECT `key` FROM metadata WHERE `key`=?",
                &[key.clone().into()],
            )?;
            if rows.len() != 1 || rows[0].text(0)? != key {
                return Err(refusal("capability_silent_truncation"));
            }
        }
        tx.rollback()?;
        tests.push(json!({"probe":"metadata_full_index_boundary","characters":length,"utf8_bytes":length*4,"accepted":success,"failure_fault":failure_fault,"warnings":encode_rows(warning_rows)?,"rollback":true}));
    }
    let metadata = ctx
        .plan
        .tables
        .iter()
        .find(|t| t.name == "metadata")
        .ok_or_else(|| refusal("metadata_missing"))?;
    copy::verify_table(&mut ctx.db, metadata)?;
    if super::guard_normal(&mut ctx.db).is_ok() {
        return Err(refusal("capability_normal_admission_succeeded"));
    }
    Ok(
        json!({"constraints_38_primary_key_and_notnull":tests,"F6_all8":"Copied typed manifest includes case/accent/NFC-NFD/trailing-space keys, ASCII192/Unicode192, intent65 and complete serialized intent; exact full values verified in all38 tables.","marker_admission_refused":true,"full_index_no_prefix":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_stage_selector_accepts_one_exact_pinned_stage() {
        let stages = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":1,"shape_sha256":"b".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(select_schema_stage(&stages, &"b".repeat(64), 2).unwrap(), 1);
    }

    #[test]
    fn schema_stage_selector_accepts_initial_and_final_pinned_stages() {
        let stages = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":1,"shape_sha256":"b".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(select_schema_stage(&stages, &"a".repeat(64), 2).unwrap(), 0);
        assert_eq!(select_schema_stage(&stages, &"c".repeat(64), 2).unwrap(), 2);
    }

    #[test]
    fn schema_stage_selector_refuses_unknown_and_ambiguous_hashes() {
        let unique = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":1,"shape_sha256":"b".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(
            select_schema_stage(&unique, &"d".repeat(64), 2)
                .unwrap_err()
                .message,
            "target_unknown_or_ambiguous_partial_schema"
        );
        let ambiguous = vec![
            json!({"step":0,"shape_sha256":"b".repeat(64)}),
            json!({"step":1,"shape_sha256":"b".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(
            select_schema_stage(&ambiguous, &"b".repeat(64), 2)
                .unwrap_err()
                .message,
            "target_unknown_or_ambiguous_partial_schema"
        );
    }

    #[test]
    fn schema_stage_selector_refuses_tampered_matching_stage() {
        let missing_step = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"shape_sha256":"b".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(
            select_schema_stage(&missing_step, &"b".repeat(64), 2)
                .unwrap_err()
                .message,
            "capability_stage_invalid"
        );
        let out_of_range = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":3,"shape_sha256":"b".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(
            select_schema_stage(&out_of_range, &"b".repeat(64), 2)
                .unwrap_err()
                .message,
            "capability_stage_invalid"
        );
        let misnumbered = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":2,"shape_sha256":"b".repeat(64)}),
        ];
        assert_eq!(
            select_schema_stage(&misnumbered, &"b".repeat(64), 2)
                .unwrap_err()
                .message,
            "capability_stage_invalid"
        );
        let malformed_hash = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":1,"shape_sha256":"B".repeat(64)}),
            json!({"step":2,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(
            select_schema_stage(&malformed_hash, &"a".repeat(64), 2)
                .unwrap_err()
                .message,
            "capability_stage_invalid"
        );
        for malformed in ["a".repeat(63), "a".repeat(65), format!("{}g", "a".repeat(63))] {
            let bad_hash = vec![
                json!({"step":0,"shape_sha256":malformed}),
                json!({"step":1,"shape_sha256":"b".repeat(64)}),
                json!({"step":2,"shape_sha256":"c".repeat(64)}),
            ];
            assert_eq!(
                select_schema_stage(&bad_hash, &"a".repeat(64), 2)
                    .unwrap_err()
                    .message,
                "capability_stage_invalid"
            );
        }
    }

    #[test]
    fn schema_stage_selector_preflights_later_entries_before_accepting_current_stage() {
        let current = json!({"step":0,"shape_sha256":"a".repeat(64)});
        let later = json!({"step":1,"shape_sha256":"b".repeat(64)});
        assert_eq!(
            select_schema_stage(&[current.clone(), later.clone()], &"a".repeat(64), 2)
                .unwrap_err()
                .message,
            "capability_stage_invalid"
        );
        let misnumbered_later = json!({"step":2,"shape_sha256":"b".repeat(64)});
        assert_eq!(
            select_schema_stage(
                &[current, misnumbered_later, json!({"step":2,"shape_sha256":"c".repeat(64)})],
                &"a".repeat(64),
                2,
            )
            .unwrap_err()
            .message,
            "capability_stage_invalid"
        );
    }

    #[test]
    fn next_schema_stage_comparison_accepts_exact_and_refuses_drift_or_missing_stage() {
        let exact = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":1,"shape_sha256":"b".repeat(64)}),
        ];
        assert!(validate_next_schema_stage(&exact, 1, &"b".repeat(64)).is_ok());

        let changed = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":1,"shape_sha256":"c".repeat(64)}),
        ];
        assert_eq!(
            validate_next_schema_stage(&changed, 1, &"b".repeat(64))
                .unwrap_err()
                .message,
            "target_ddl_stage_drift"
        );
        assert_eq!(
            validate_next_schema_stage(&exact[..1], 1, &"b".repeat(64))
                .unwrap_err()
                .message,
            "target_ddl_stage_drift"
        );
        let misnumbered = vec![
            json!({"step":0,"shape_sha256":"a".repeat(64)}),
            json!({"step":2,"shape_sha256":"b".repeat(64)}),
        ];
        assert_eq!(
            validate_next_schema_stage(&misnumbered, 1, &"b".repeat(64))
                .unwrap_err()
                .message,
            "target_ddl_stage_drift"
        );
    }

    #[test]
    fn steps_are_marker_first_versioned_and_pin_all_released_bytes() {
        let steps = steps().unwrap();
        assert!(steps[0].contains("CREATE TABLE IF NOT EXISTS podmesh_import_state"));
        assert!(steps[1].contains("CREATE TABLE IF NOT EXISTS podmesh_import_tables"));
        assert!(steps[2].contains("CREATE TABLE IF NOT EXISTS store_schema"));
        assert!(steps.last().unwrap().contains("VALUES('node',11,0)"));
        assert_eq!(sql_hashes().as_object().unwrap().len(), 24);
        let manifest = protocol().unwrap();
        let points = manifest["killpoints"].as_array().unwrap();
        assert_eq!(
            points
                .iter()
                .filter(|p| p
                    .as_str()
                    .is_some_and(|s| s.starts_with("after_each_successor_ALTER:")))
                .count(),
            38
        );
        assert_eq!(
            manifest["conditional_contract_fields"]["capability"],
            json!(["admission_probe_directory"])
        );
        assert!(!manifest["required_contract_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "admission_probe_directory_for_capability"));
        assert_eq!(
            steps
                .iter()
                .filter(|s| s.starts_with("INSERT INTO store_schema"))
                .count(),
            11
        );
    }
    #[test]
    fn protocol_fields_construct_a_valid_contract_without_any_connect() {
        let manifest=protocol().unwrap();
        let mut candidate=serde_json::Map::new();
        for key in manifest["required_contract_fields"].as_array().unwrap() {candidate.insert(key.as_str().unwrap().to_owned(),json!("synthetic-placeholder"));}
        for (key,value) in [
            ("schema",json!("podmesh-c02-fixture/1")),("status",json!("ready")),("server_execution_authorized",json!(true)),("role",json!("capability")),("migration_id",json!("campaign-global")),
            ("canonical_contract_sha256",json!(contract::CANONICAL_SHA)),("resource_caps_contract_sha256",json!(contract::CAPS_SHA)),
            ("source_manifest_sha256",json!("c".repeat(64))),("binary_sha256",json!("b".repeat(64))),("source_commit",json!("a".repeat(40))),
            ("target_resource_identity",json!({"container_id":"synthetic-container","volume_name":"synthetic-volume","image_id":"synthetic-image"})),
            ("resource_caps",serde_json::from_str(include_str!("../../../fixtures/C/copy-v1/resource-caps.json")).unwrap())
        ] {candidate.insert(key.to_owned(),value);}
        for key in ["connect_timeout_ms","lock_wait_timeout_seconds","statement_timeout_ms","maximum_campaign_seconds"] {candidate.insert(key.to_owned(),json!(1));}
        let mut candidate=json!(candidate);
        assert!(Contract::validate(&candidate,&["capability"]).is_err());
        for key in manifest["conditional_contract_fields"]["capability"].as_array().unwrap(){candidate[key.as_str().unwrap()]=json!("/synthetic-private-probe");}
        Contract::validate(&candidate,&["capability"]).unwrap();
        let mut restore=candidate.clone();restore["role"]=json!("seeded_restore");
        for key in ["dump_restore_receipt","dump_restore_helper_sha256"] {assert!(Contract::validate(&restore,&["seeded_restore"]).is_err());restore[key]=if key=="dump_restore_receipt"{json!({"path":"/private-admin","sha256":"d".repeat(64)})}else{json!("e".repeat(64))};}
        Contract::validate(&restore,&["seeded_restore"]).unwrap();
        for key in manifest["target_resource_identity_required_fields"].as_array().unwrap(){let mut missing=candidate.clone();missing["target_resource_identity"][key.as_str().unwrap()]=Json::Null;assert!(Contract::validate(&missing,&["capability"]).is_err());}
    }
    #[test]
    fn prerequisite_source_binary_input_or_sql_drift_is_refused() {
        let contract = json!({"source_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","binary_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","source_manifest_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","SQL_hashes":sql_hashes(),"migration_id":"campaign-global","resource_caps":{"snapshot":123},"canonical_contract_sha256":contract::CANONICAL_SHA,"resource_caps_contract_sha256":contract::CAPS_SHA,"plan_sha256":"expected-plan"});
        prerequisite_provenance(&contract,&contract,"expected-plan").unwrap();
        for key in ["source_commit","binary_sha256","source_manifest_sha256","SQL_hashes","migration_id","resource_caps","canonical_contract_sha256","resource_caps_contract_sha256"] {
            for replacement in [Json::Null,json!("swapped")] {
                let mut changed=contract.clone();changed[key]=replacement;
                assert!(prerequisite_provenance(&changed,&contract,"expected-plan").unwrap_err().message.contains("exact_source_binary_or_input"));
            }
        }
        for replacement in [Json::Null,json!("other-plan")] {
            let mut changed=contract.clone();changed["plan_sha256"]=replacement;
            assert_eq!(prerequisite_provenance(&changed,&contract,"expected-plan").unwrap_err().message,"prerequisite_exact_plan_mismatch");
        }
    }
    #[test]
    fn admin_dump_lineage_swaps_refuse_before_any_connect() {
        let source=json!({"database":"origin-db","container_id":"source-container","volume_name":"source-volume","image_id":"image"});
        let target=json!({"database":"default-unused","container_id":"target-container","volume_name":"target-volume","image_id":"image"});
        let spec=json!({"path":"private-origin","sha256":"pinned-origin"});
        let contract=json!({"database":"restored-db","restore_origin_receipt":spec,"dump_restore_helper_sha256":"pinned-helper","target_resource_identity":target});
        let origin=json!({"database":"origin-db","target_resource_identity":source});
        let receipt=json!({"format":"podmesh-A-synthetic-dump-restore/1","status":"DUMP_RESTORED_TYPED_VERIFICATION_PENDING","serving":false,"identity_rebound":false,"typed_verification_proven":false,"origin_receipt":spec,"helper_sha256":"pinned-helper","restored_database":"restored-db"});
        validate_dump_restore_link(&receipt,&contract,&origin,&source,&target).unwrap();
        validate_dump_hash(&json!({"dump_sha256":"actual"}),"actual").unwrap();
        assert_eq!(validate_dump_hash(&json!({"dump_sha256":"swapped"}),"actual").unwrap_err().message,"admin_dump_actual_bytes_checksum_mismatch");
        assert!(validate_dump_hash(&json!({}),"actual").is_err());
        for key in ["format","status","serving","identity_rebound","typed_verification_proven","origin_receipt","helper_sha256","restored_database"] {
            for bad in [Json::Null,json!("swapped")] {let mut changed=receipt.clone();changed[key]=bad;assert!(validate_dump_restore_link(&changed,&contract,&origin,&source,&target).is_err());}
        }
        for key in ["container_id","volume_name","image_id"] {
            let mut changed=target.clone();changed[key]=json!("swapped");assert!(validate_dump_restore_link(&receipt,&contract,&origin,&source,&changed).is_err());
            let mut changed=source.clone();changed[key]=Json::Null;assert!(validate_dump_restore_link(&receipt,&contract,&origin,&changed,&target).is_err());
        }
    }
    #[test]
    fn no_contract_is_a_named_failure_not_a_server_skip() {
        let missing = Path::new("/nonexistent-C02-private-contract");
        assert!(run(
            "copy",
            &Request {
                contract_path: missing,
                snapshot_path: missing,
                seal_path: missing,
                output: missing
            }
        )
        .is_err());
    }
}
