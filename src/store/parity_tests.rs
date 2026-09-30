//! Synthetic scalar insert/read parity. These tests do not copy a journal or operate a node.
use super::{migrations, DurableStore, Result, Row, SqliteStore, Value};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn fixture() -> Json {
    serde_json::from_str(include_str!("../../fixtures/C/node-parity-v1.json")).unwrap()
}

fn quoted(name: &str) -> String {
    assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    format!("`{name}`")
}

fn columns(table: &Json) -> Vec<String> {
    table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

fn values(table: &Json, table_index: usize, row: usize) -> Vec<Value> {
    table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(column_index, c)| {
            let nullable = c["nullable"].as_bool().unwrap();
            if nullable && row == 0 {
                return Value::Null;
            }
            match c["type"].as_str().unwrap() {
                "INTEGER" => {
                    let n = if c["name"] == "id" {
                        row as i64 + 1
                    } else {
                        [i64::MIN, 0, i64::MAX][row]
                    };
                    Value::Integer(n)
                }
                "TEXT" if nullable && row == 1 => Value::Text(String::new()),
                "TEXT" => Value::Text(format!(
                    "fx-t{table_index}-r{row}-c{column_index}-é中🙂e\u{301};'"
                )),
                other => panic!("fixture must explicitly handle declared node kind {other}"),
            }
        })
        .collect()
}

fn insert_sql(table: &str, names: &[String]) -> String {
    format!(
        "INSERT INTO {}({}) VALUES({})",
        quoted(table),
        names
            .iter()
            .map(|c| quoted(c))
            .collect::<Vec<_>>()
            .join(","),
        vec!["?"; names.len()].join(",")
    )
}

fn select_sql(table: &str, names: &[String]) -> String {
    format!(
        "SELECT {} FROM {}",
        names
            .iter()
            .map(|c| quoted(c))
            .collect::<Vec<_>>()
            .join(","),
        quoted(table)
    )
}

// Framed bytes retain kind, width, empty/NULL distinction, Unicode bytes and IEEE bits.
// Whole rows are sorted in memory: engine collation must never choose hash ordering.
fn frame(bytes: &[u8], output: &mut Vec<u8>) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}

fn canonical_row(values: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(values.len() as u64).to_be_bytes());
    for value in values {
        match value {
            Value::Null => out.push(b'n'),
            Value::Integer(n) => {
                out.push(b'i');
                out.extend_from_slice(&n.to_be_bytes());
            }
            Value::Real(n) => {
                out.push(b'r');
                out.extend_from_slice(&n.to_bits().to_be_bytes());
            }
            Value::Text(text) => {
                out.push(b't');
                frame(text.as_bytes(), &mut out);
            }
            Value::Blob(blob) => {
                out.push(b'b');
                frame(blob, &mut out);
            }
        }
    }
    out
}

fn snapshot(table: &str, names: &[String], rows: &[Row]) -> Json {
    let mut encoded: Vec<Vec<u8>> = rows.iter().map(|r| canonical_row(r.values())).collect();
    encoded.sort();
    let mut all = b"podmesh-c02a-canonical/1".to_vec();
    frame(table.as_bytes(), &mut all);
    for name in names {
        frame(name.as_bytes(), &mut all);
    }
    all.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
    for row in encoded {
        frame(&row, &mut all);
    }
    let mut kinds = BTreeMap::<String, usize>::new();
    for value in rows.iter().flat_map(Row::values) {
        *kinds.entry(value.kind().to_string()).or_default() += 1;
    }
    json!({"rows": rows.len(), "sha256": format!("{:x}", Sha256::digest(all)), "kind_counts": kinds, "columns": names})
}

fn expected(table: &Json, table_index: usize) -> Vec<Row> {
    let names = std::sync::Arc::new(columns(table));
    (0..3)
        .map(|r| Row::new(names.clone(), values(table, table_index, r)))
        .collect()
}

fn populate(store: &mut dyn DurableStore, table: &Json, index: usize) -> Result<()> {
    let names = columns(table);
    let sql = insert_sql(table["name"].as_str().unwrap(), &names);
    for row in 0..3 {
        store.execute(&sql, &values(table, index, row))?;
    }
    Ok(())
}

fn checked_sqlite() -> SqliteStore {
    let mut store = SqliteStore::open_in_memory().unwrap();
    migrations::apply(&mut store).unwrap();
    let manifest = fixture();
    for migration in migrations::NODE_MIGRATIONS {
        for engine in [crate::store::Engine::Sqlite, crate::store::Engine::Mariadb] {
            let path = format!(
                "src/store/migrations/node/{}.{}.sql",
                migration.id,
                engine.as_str()
            );
            assert_eq!(
                manifest["released_sql_sha256"][path],
                format!("{:x}", Sha256::digest(migration.sql(engine).as_bytes())),
                "released SQL must match immutable fixture manifest"
            );
        }
    }
    let fixture_names: Vec<&str> = manifest["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let mut inventory = migrations::node_tables().unwrap();
    inventory.sort_unstable();
    assert_eq!(fixture_names, inventory);
    for table in manifest["tables"].as_array().unwrap() {
        let rows = store
            .query(
                &format!(
                    "PRAGMA table_info({})",
                    quoted(table["name"].as_str().unwrap())
                ),
                &[],
            )
            .unwrap();
        let actual: Vec<Json> = rows.iter().map(|r| json!({"name": r.text(1).unwrap(), "type": r.text(2).unwrap(), "nullable": r.integer(3).unwrap() == 0 && r.integer(5).unwrap() == 0, "primary_key_position": r.integer(5).unwrap()})).collect();
        assert_eq!(
            actual,
            *table["columns"].as_array().unwrap(),
            "fixture shape must match released schema"
        );
    }
    store
}

#[test]
fn all_38_versioned_sqlite_tables_hold_exact_typed_synthetic_rows() {
    let manifest = fixture();
    let mut store = checked_sqlite();
    let mut total = 0;
    for (index, table) in manifest["tables"].as_array().unwrap().iter().enumerate() {
        populate(&mut store, table, index).unwrap();
        let names = columns(table);
        let name = table["name"].as_str().unwrap();
        let rows = store.query(&select_sql(name, &names), &[]).unwrap();
        assert_eq!(
            snapshot(name, &names, &rows),
            snapshot(name, &names, &expected(table, index))
        );
        total += rows.len();
    }
    assert_eq!(total, 114);
}

#[test]
fn canonical_hash_distinguishes_kind_empty_null_unicode_and_row_order() {
    let names = std::sync::Arc::new(vec!["value".to_string()]);
    let hash = |v| snapshot("probe", &names, &[Row::new(names.clone(), vec![v])])["sha256"].clone();
    let distinct = [
        Value::Null,
        Value::Text(String::new()),
        Value::Blob(vec![]),
        Value::Integer(1),
        Value::Real(1.0),
        Value::Text("1".to_string()),
        Value::Text("é".to_string()),
        Value::Text("e\u{301}".to_string()),
        Value::Blob(vec![0, 255]),
    ];
    let hashes: std::collections::BTreeSet<String> = distinct
        .into_iter()
        .map(|v| hash(v).as_str().unwrap().to_string())
        .collect();
    assert_eq!(hashes.len(), 9);
    let a = Row::new(names.clone(), vec![Value::Integer(1)]);
    let b = Row::new(names.clone(), vec![Value::Integer(2)]);
    assert_eq!(
        snapshot("probe", &names, &[a.clone(), b.clone()]),
        snapshot("probe", &names, &[b, a])
    );
}

#[cfg(feature = "mariadb")]
mod server {
    use super::*;
    use crate::store::{Fault, MariadbConfig, MariadbStore};
    use mysql::Opts;
    use std::{fs, io::Write, path::Path};

    fn required_env(name: &str) -> String {
        std::env::var(name)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                panic!("{name} is required: refused, no server test success claimed")
            })
    }

    fn validate_contract(contract: &Json, dsn: &str) -> Result<MariadbConfig> {
        if contract["task_id"] != "A02R"
            || contract["status"] != "ready"
            || !contract["writer_lock"]
                .as_str()
                .is_some_and(|s| Path::new(s).is_absolute())
            || contract["network"] != "none"
            || contract["synthetic_only"] != true
            || contract["C02A_execution_authorized"] != true
            || contract["fixture_id"].as_str().is_none_or(str::is_empty)
        {
            return Err(Fault::Denied.error(
                "C02A requires A02R's explicitly authorized isolated synthetic fixture contract",
            ));
        }
        let opts =
            Opts::from_url(dsn).map_err(|_| Fault::Denied.error("C02A fixture DSN is invalid"))?;
        let socket = contract["socket"].as_str().unwrap_or_default();
        let database = contract["database"].as_str().unwrap_or_default();
        if !Path::new(socket).is_absolute()
            || opts.get_socket() != Some(socket)
            || !database.starts_with("podmesh_c02a_")
            || opts.get_db_name() != Some(database)
            || opts.get_pass().is_some_and(|s| !s.is_empty())
        {
            return Err(Fault::Denied.error(
                "C02A requires exactly the private fixture socket/database and no password",
            ));
        }
        Ok(MariadbConfig::from_dsn(dsn))
    }

    fn cell(value: &Value) -> Json {
        match value {
            Value::Null => json!({"kind":"null"}),
            Value::Integer(n) => json!({"kind":"integer","value":n}),
            Value::Real(n) => json!({"kind":"real","bits":format!("{:016x}",n.to_bits())}),
            Value::Text(s) => {
                json!({"kind":"text","value":s,"characters":s.chars().count(),"bytes":s.len()})
            }
            Value::Blob(b) => {
                json!({"kind":"blob","hex":b.iter().map(|n|format!("{n:02x}")).collect::<String>()})
            }
        }
    }

    fn cells(rows: &[Row]) -> Json {
        Json::Array(
            rows.iter()
                .map(|r| Json::Array(r.values().iter().map(cell).collect()))
                .collect(),
        )
    }

    fn exact_rows(rows: &[Row]) -> Vec<Vec<u8>> {
        let mut result: Vec<Vec<u8>> = rows.iter().map(|r| canonical_row(r.values())).collect();
        result.sort();
        result
    }

    fn attempt(
        store: &mut dyn DurableStore,
        table: &str,
        names: &[String],
        params: &[Value],
    ) -> Result<Json> {
        let select = select_sql(table, names);
        let before = store.query(&select, &[])?;
        let result = store.execute(&insert_sql(table, names), params);
        // Read warnings immediately: another statement may replace the diagnostics area.
        let warning_rows = if store.engine() == crate::store::Engine::Mariadb {
            store.query("SHOW WARNINGS", &[])?
        } else {
            Vec::new()
        };
        let warnings = cells(&warning_rows);
        let mysql_diagnostic_codes: Vec<i64> = warning_rows
            .iter()
            .filter_map(|r| r.integer(1).ok())
            .collect();
        let outcome = match result {
            Ok(n) => json!({"accepted":true,"affected_rows":n}),
            Err(e) => json!({"accepted":false,"fault":e.fault.as_str(),"message":e.message}),
        };
        let after = store.query(&select, &[])?;
        Ok(
            json!({"outcome":outcome,"warnings":warnings,"mysql_diagnostic_codes":mysql_diagnostic_codes,"before":snapshot(table,names,&before),"after":snapshot(table,names,&after),"input":params.iter().map(cell).collect::<Vec<_>>()}),
        )
    }

    fn require(condition: bool, detail: &str) -> Result<()> {
        if condition {
            Ok(())
        } else {
            Err(Fault::Integrity.error(detail))
        }
    }

    fn probe_kinds(sqlite: &mut SqliteStore, maria: &mut MariadbStore) -> Result<Json> {
        sqlite.execute_batch("CREATE TABLE c02a_value_probe(id INTEGER PRIMARY KEY, integer_value INTEGER, real_value REAL, text_value TEXT, blob_value BLOB);")?;
        maria.execute_batch("CREATE TABLE c02a_value_probe(id BIGINT PRIMARY KEY, integer_value BIGINT NULL, real_value DOUBLE NULL, text_value LONGTEXT NULL, blob_value LONGBLOB NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;")?;
        let names = [
            "id",
            "integer_value",
            "real_value",
            "text_value",
            "blob_value",
        ]
        .map(str::to_string)
        .to_vec();
        let sql = insert_sql("c02a_value_probe", &names);
        let samples = [
            vec![
                Value::Integer(1),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ],
            vec![
                Value::Integer(2),
                Value::Integer(i64::MIN),
                Value::Real(1.5),
                Value::Text("é中🙂e\u{301}\0;'\\".into()),
                Value::Blob(vec![0, 255, 10, 39, 92, 195, 40]),
            ],
            vec![
                Value::Integer(3),
                Value::Integer(i64::MAX),
                Value::Real(-2.25),
                Value::Text(String::new()),
                Value::Blob(vec![]),
            ],
        ];
        for row in &samples {
            sqlite.execute(&sql, row)?;
            maria.execute(&sql, row)?;
        }
        let select = select_sql("c02a_value_probe", &names);
        let left = sqlite.query(&select, &[])?;
        let right = maria.query(&select, &[])?;
        let expected_rows: Vec<Row> = samples
            .into_iter()
            .map(|v| Row::new(std::sync::Arc::new(names.clone()), v))
            .collect();
        let expected = snapshot("c02a_value_probe", &names, &expected_rows);
        let a = snapshot("c02a_value_probe", &names, &left);
        let b = snapshot("c02a_value_probe", &names, &right);
        require(
            a == expected && b == expected,
            "auxiliary five-kind roundtrip must preserve exact values and kind",
        )?;
        Ok(
            json!({"scope":"Auxiliary generic store roundtrip, outside 38 node tables; node schema declares no BLOB/REAL","sqlite":a,"mariadb":b,"expected":expected,"synthetic_values":cells(&expected_rows)}),
        )
    }

    fn counterexamples(
        sqlite: &mut SqliteStore,
        maria: &mut MariadbStore,
        report: &mut Json,
    ) -> Result<()> {
        let mut cases = Vec::new();
        let names = vec!["key".into(), "value".into()];
        for (kind, seed, alias) in [
            ("case", "c02a-Case-Key", "c02a-case-key"),
            ("accent", "c02a-cafe-key", "c02a-café-key"),
            (
                "unicode_composition",
                "c02a-norm-é-key",
                "c02a-norm-e\u{301}-key",
            ),
            ("trailing_space", "c02a-space-key", "c02a-space-key "),
        ] {
            for (is_alias, key) in [(false, seed), (true, alias)] {
                let label = format!(
                    "collation_{kind}_{}",
                    if is_alias { "alias" } else { "seed" }
                );
                let params = [Value::Text(key.into()), Value::Text(label.clone())];
                let a = attempt(sqlite, "metadata", &names, &params)?;
                let b = attempt(maria, "metadata", &names, &params)?;
                let left = sqlite.query(
                    "SELECT `key`,value FROM metadata WHERE `key`=?",
                    &[Value::Text(key.into())],
                )?;
                let right = maria.query(
                    "SELECT `key`,value FROM metadata WHERE `key`=?",
                    &[Value::Text(key.into())],
                )?;
                cases.push(json!({"name":label,"sqlite":a,"mariadb":b,"sqlite_equality_lookup":cells(&left),"mariadb_equality_lookup":cells(&right),"observed_insert_outcome_differs":a["outcome"]["accepted"]!=b["outcome"]["accepted"],"observed_equality_lookup_differs":cells(&left)!=cells(&right)}));
                report["F6_counterexamples"] = json!(cases);
                require(
                    a["outcome"]["accepted"] == true,
                    "SQLite binary TEXT keys must retain each distinct test key",
                )?;
                if !is_alias {
                    require(
                        b["outcome"]["accepted"] == true,
                        "collation seed must be accepted by MariaDB",
                    )?;
                } else if b["outcome"]["accepted"] == false {
                    require(
                        b["outcome"]["fault"] == "integrity",
                        "alias refusal must name integrity rather than conceal another fault",
                    )?;
                }
                // A binary collation may preserve the pair. Record that exact outcome;
                // neither collision nor preservation closes the general collation gap.
            }
        }
        for (alphabet, ch) in [("ascii", 'w'), ("unicode", 'é')] {
            for length in [190, 191, 192] {
                let label = format!("varchar191_{alphabet}_{length}");
                let key = ch.to_string().repeat(length);
                let params = [Value::Text(key.clone()), Value::Text(label.clone())];
                let a = attempt(sqlite, "metadata", &names, &params)?;
                let b = attempt(maria, "metadata", &names, &params)?;
                let left = sqlite.query(
                    "SELECT `key` FROM metadata WHERE value=?",
                    &[Value::Text(label.clone())],
                )?;
                let right = maria.query(
                    "SELECT `key` FROM metadata WHERE value=?",
                    &[Value::Text(label)],
                )?;
                let same = exact_rows(&left) == exact_rows(&right);
                cases.push(json!({"name":format!("varchar191_{alphabet}_{length}"),"sqlite":a,"mariadb":b,"sqlite_returned":cells(&left),"mariadb_returned":cells(&right),"exact_roundtrip_equal":same}));
                report["F6_counterexamples"] = json!(cases);
                require(
                    a["outcome"]["accepted"] == true,
                    "SQLite must retain the over-width synthetic text",
                )?;
                if length <= 191 {
                    require(
                        same && b["outcome"]["accepted"] == true,
                        "representable width boundary must roundtrip exactly",
                    )?;
                } else {
                    require(!same,"192 characters must expose current VARCHAR191 divergence, refusal or truncation")?;
                }
            }
        }
        let manifest = fixture();
        let table = manifest["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "network_effects")
            .unwrap();
        let names = columns(table);
        let intent = names.iter().position(|n| n == "intent").unwrap();
        let id = names.iter().position(|n| n == "id").unwrap();
        let serialized=serde_json::to_string(&json!({"operation":"synthetic_intent","universe_uuid":"fixture-only","desired":"measure storage, no effect","authority":"inert fixture"})).unwrap();
        let intent_cases = vec![
            ("intent64_63".to_string(), "i".repeat(63)),
            ("intent64_64".to_string(), "i".repeat(64)),
            ("intent64_65".to_string(), "i".repeat(65)),
            ("intent64_serialized".to_string(), serialized),
        ];
        for (case_index, (label, payload)) in intent_cases.into_iter().enumerate() {
            let length = payload.chars().count();
            let mut params = values(table, 90, 2);
            params[intent] = Value::Text(payload);
            params[id] = Value::Integer(7000 + case_index as i64);
            let a = attempt(sqlite, "network_effects", &names, &params)?;
            let b = attempt(maria, "network_effects", &names, &params)?;
            let left = sqlite.query(
                "SELECT intent FROM network_effects WHERE id=?",
                &[params[id].clone()],
            )?;
            let right = maria.query(
                "SELECT intent FROM network_effects WHERE id=?",
                &[params[id].clone()],
            )?;
            let same = exact_rows(&left) == exact_rows(&right);
            cases.push(json!({"name":label,"sqlite":a,"mariadb":b,"sqlite_returned":cells(&left),"mariadb_returned":cells(&right),"exact_roundtrip_equal":same}));
            report["F6_counterexamples"] = json!(cases);
            require(
                a["outcome"]["accepted"] == true,
                "SQLite must retain over-width synthetic intent",
            )?;
            if length <= 64 {
                require(
                    same && b["outcome"]["accepted"] == true,
                    "64 character intent boundary must roundtrip exactly",
                )?;
            } else {
                require(!same,"over-width intent must expose current VARCHAR64 divergence, refusal or truncation")?;
            }
        }
        Ok(())
    }

    fn run_pair(config: &MariadbConfig, report: &mut Json) -> Result<()> {
        let mut sqlite = checked_sqlite();
        let mut maria = MariadbStore::open(config)?;
        require(maria.tables()?.is_empty(),"C02A refuses any fixture database that already carries tables; A must provide a fresh database")?;
        let info=maria.query("SELECT DATABASE(), VERSION(), @@version_comment, @@GLOBAL.sql_mode, @@SESSION.sql_mode, @@collation_server, @@collation_database, @@character_set_database, @@character_set_connection, @@time_zone, @@system_time_zone, @@GLOBAL.innodb_flush_log_at_trx_commit, @@tx_isolation",&[])?;
        report["effective_server_settings"] =
            json!({"columns":info[0].columns(),"values":cells(&info)});
        require(
            info[0].text(0)? == report["contract_database"].as_str().unwrap(),
            "DATABASE() must equal the named A02R fixture database before DDL",
        )?;
        report["actual_database_verified_before_ddl"] = json!(info[0].text(0)?);
        migrations::apply(&mut maria)?;
        report["table_collations"]=cells(&maria.query("SELECT TABLE_NAME,TABLE_COLLATION FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() ORDER BY TABLE_NAME",&[])?);
        let manifest = fixture();
        let mut results = Vec::new();
        let mut duplicates = Vec::new();
        for (index, table) in manifest["tables"].as_array().unwrap().iter().enumerate() {
            let name = table["name"].as_str().unwrap();
            let names = columns(table);
            let shape=maria.query("SELECT COLUMN_NAME,DATA_TYPE,IS_NULLABLE,EXTRA,CHARACTER_SET_NAME,COLLATION_NAME,COLUMN_TYPE,COLUMN_DEFAULT,CHARACTER_MAXIMUM_LENGTH FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? ORDER BY ORDINAL_POSITION",&[Value::Text(name.into())])?;
            let physical: Vec<String> = shape
                .iter()
                .filter(|r| !r.text(3).unwrap().contains("VIRTUAL"))
                .map(|r| r.text(0).unwrap().to_string())
                .collect();
            require(
                physical == names,
                "MariaDB nonvirtual columns must match pinned SQLite fixture names/order",
            )?;
            let virtuals: Vec<&str> = shape
                .iter()
                .filter(|r| r.text(3).unwrap().contains("VIRTUAL"))
                .map(|r| r.text(0).unwrap())
                .collect();
            require(
                (name == "network_allocations" && virtuals == ["live_ip", "live_universe_uuid"])
                    || (name != "network_allocations" && virtuals.is_empty()),
                "only the two declared MariaDB live-allocation virtual columns may be excluded",
            )?;
            populate(&mut sqlite, table, index)?;
            populate(&mut maria, table, index)?;
            if name == "network_allocations" {
                let generated_names = vec![
                    "operation_id".to_string(),
                    "live_ip".to_string(),
                    "live_universe_uuid".to_string(),
                ];
                let generated = maria.query(&select_sql(name, &generated_names), &[])?;
                let position = |want: &str| names.iter().position(|n| n == want).unwrap();
                let generated_expected: Vec<Row> = (0..3)
                    .map(|row| {
                        let v = values(table, index, row);
                        let live = v[position("released_at")].is_null();
                        Row::new(
                            std::sync::Arc::new(generated_names.clone()),
                            vec![
                                v[position("operation_id")].clone(),
                                if live {
                                    v[position("ip")].clone()
                                } else {
                                    Value::Null
                                },
                                if live {
                                    v[position("universe_uuid")].clone()
                                } else {
                                    Value::Null
                                },
                            ],
                        )
                    })
                    .collect();
                report["mariadb_only_derived_live_columns"] = json!({"scope":"Additional derived values, not dropped source fields","actual":cells(&generated),"expected":cells(&generated_expected)});
                require(
                    exact_rows(&generated) == exact_rows(&generated_expected),
                    "MariaDB live columns must match NULL/released fixture semantics",
                )?;
            }
            let select = select_sql(name, &names);
            let left = sqlite.query(&select, &[])?;
            let right = maria.query(&select, &[])?;
            let want = snapshot(name, &names, &expected(table, index));
            let a = snapshot(name, &names, &left);
            let b = snapshot(name, &names, &right);
            let want_rows = expected(table, index);
            let exact_equal = exact_rows(&left) == exact_rows(&want_rows)
                && exact_rows(&right) == exact_rows(&want_rows);
            results.push(json!({"table":name,"sqlite":a,"mariadb":b,"expected":want,"exact_typed_multiset_equal":exact_equal,"sqlite_typed_rows":cells(&left),"mariadb_typed_rows":cells(&right),"expected_typed_rows":cells(&want_rows),"mariadb_column_fields":shape[0].columns(),"mariadb_columns":cells(&shape),"virtual_columns_excluded":virtuals}));
            report["node_scalar_insert_read_parity"] = json!(results);
            require(
                exact_equal && a == want && b == want,
                "populated table must retain count, exact hash and kind counts in both stores",
            )?;
            let params = values(table, index, 0);
            let a = attempt(&mut sqlite, name, &names, &params)?;
            let b = attempt(&mut maria, name, &names, &params)?;
            duplicates.push(json!({"table":name,"sqlite":a,"mariadb":b}));
            report["duplicate_constraint_checks"] = json!(duplicates);
            require(
                a["outcome"]["accepted"] == false
                    && b["outcome"]["accepted"] == false
                    && a["outcome"]["fault"] == "integrity"
                    && b["outcome"]["fault"] == "integrity",
                "duplicate fixture row must refuse with integrity in both engines",
            )?;
            require(
                a["before"] == a["after"] && b["before"] == b["after"],
                "duplicate refusal must leave table hashes and counts unchanged",
            )?;
        }
        let mut final_positive = Vec::new();
        for (index, table) in manifest["tables"].as_array().unwrap().iter().enumerate() {
            let name = table["name"].as_str().unwrap();
            let names = columns(table);
            let sql = select_sql(name, &names);
            let left = sqlite.query(&sql, &[])?;
            let right = maria.query(&sql, &[])?;
            let want = expected(table, index);
            require(
                exact_rows(&left) == exact_rows(&want) && exact_rows(&right) == exact_rows(&want),
                "all positive table bytes must remain unchanged before counterexamples",
            )?;
            final_positive.push(json!({"table":name,"sqlite":snapshot(name,&names,&left),"mariadb":snapshot(name,&names,&right)}));
        }
        report["all_positive_tables_rechecked_before_counterexamples"] = json!(final_positive);
        report["store_schema_control_rows"] = json!({"scope":"Control table excluded from 38 node tables","sqlite":cells(&sqlite.query("SELECT name,version,applied_at FROM store_schema ORDER BY name",&[])?),"mariadb":cells(&maria.query("SELECT name,version,applied_at FROM store_schema ORDER BY name",&[])?)});
        report["auxiliary_value_probe"] = probe_kinds(&mut sqlite, &mut maria)?;
        counterexamples(&mut sqlite, &mut maria, report)?;
        report["node_tables_measured"] = json!(results.len());
        report["node_rows_per_engine_before_counterexamples"] = json!(114);
        report["F6_status"] =
            json!("OPEN: counterexamples reproduced, equivalence not established");
        Ok(())
    }

    #[test]
    fn fixture_contract_refuses_missing_authority_wrong_socket_or_password() {
        let valid = json!({"task_id":"A02R","fixture_id":"synthetic","status":"ready","writer_lock":"/synthetic/writer.lock","network":"none","socket":"/synthetic/socket","database":"podmesh_c02a_synthetic","synthetic_only":true,"C02A_execution_authorized":true});
        let dsn = "mysql://fixture@localhost/podmesh_c02a_synthetic?socket=%2Fsynthetic%2Fsocket";
        assert!(validate_contract(&valid, dsn).is_ok());
        for bad in [
            json!({}),
            json!({"task_id":"A02R","fixture_id":"synthetic","status":"ready","writer_lock":"/synthetic/writer.lock","network":"none","socket":"/synthetic/socket","database":"podmesh_c02a_synthetic","synthetic_only":true,"C02A_execution_authorized":false}),
        ] {
            assert_eq!(
                validate_contract(&bad, dsn).unwrap_err().fault,
                Fault::Denied
            );
        }
        for bad in ["mysql://fixture@localhost/podmesh_c02a_synthetic", "mysql://fixture:forbidden@localhost/podmesh_c02a_synthetic?socket=%2Fsynthetic%2Fsocket","mysql://fixture@localhost/operational?socket=%2Fsynthetic%2Fsocket"] {assert_eq!(validate_contract(&valid,bad).unwrap_err().fault,Fault::Denied);}
    }

    #[test]
    fn cooperative_shared_lease_blocks_nonblocking_cleanup_until_release() {
        use std::os::unix::fs::OpenOptionsExt;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "fixtures/C/lease-test-{}-{nonce}",
            std::process::id()
        ));
        let shared = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let cleanup = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        shared.lock_shared().unwrap();
        assert!(
            matches!(cleanup.try_lock(), Err(fs::TryLockError::WouldBlock)),
            "cooperative cleanup must refuse an active shared test lease"
        );
        shared.unlock().unwrap();
        cleanup.try_lock().unwrap();
        cleanup.unlock().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires A02R isolated fixture contract, private DSN and fresh evidence file; never silently skips"]
    fn c02a_populated_node_scalar_parity_and_expected_f6_counterexamples() {
        let contract_path = required_env("PODMESH_C02A_CONTRACT");
        let dsn = required_env("PODMESH_C02A_DSN");
        let output = required_env("PODMESH_C02A_REPORT");
        let contract_bytes = fs::read(&contract_path).expect("read A02R fixture contract");
        let contract: Json =
            serde_json::from_slice(&contract_bytes).expect("parse A02R fixture contract");
        let config = validate_contract(&contract, &dsn)
            .expect("refuse invalid A02R contract before any connection");
        // Cooperative resource lifecycle lease. It does not defend against a forged
        // operator contract or a process that ignores this protocol.
        let lease_path = contract["writer_lock"].as_str().unwrap();
        let writer_lease =
            fs::File::open(lease_path).expect("open A-owned cooperative writer lease");
        assert!(
            writer_lease.metadata().unwrap().is_file(),
            "writer lease must be the A-owned regular file"
        );
        writer_lease
            .lock_shared()
            .expect("acquire cooperative shared writer lease");
        let lease_acquired_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_eq!(
            fs::read(&contract_path).unwrap(),
            contract_bytes,
            "ready contract changed before shared lease; refuse before connection"
        );
        let mut options = fs::OpenOptions::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut evidence = options
            .write(true)
            .create_new(true)
            .open(&output)
            .expect("C02A evidence path must be a new file");
        let mut report = json!({"task_id":"C02A","fixture_id":contract["fixture_id"],"contract_path":contract_path,"contract_database":contract["database"],"writer_lock":lease_path,"lease_acquired_unix_ns":lease_acquired_ns,"writer_lease":"shared cooperative file lock held until report sync and all DB connections closed; not a production sandbox","sequence":["read contract","acquire shared writer lease","reread unchanged ready contract under lease","connect named fixture","check actual DATABASE before DDL","measure","sync report","release lease"],"contract_sha256":format!("{:x}",Sha256::digest(fs::read(&contract_path).unwrap())),"fixture_sha256":format!("{:x}",Sha256::digest(include_bytes!("../../fixtures/C/node-parity-v1.json"))),"claim":"Independent scalar insert/read parity under this fixture, not journal copy/migration or node runtime","customer_data":false,"real_keys_or_credentials":false,"F6_status":"OPEN","C02_complete":false,"status":"running"});
        let result = run_pair(&config, &mut report);
        report["connections_closed_unix_ns"] = json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64);
        report["status"] = json!(if result.is_ok() {
            "measured_expected_limits"
        } else {
            "failed"
        });
        if let Err(e) = &result {
            report["error"] = json!({"fault":e.fault.as_str(),"message":e.message});
        }
        evidence
            .write_all(serde_json::to_string_pretty(&report).unwrap().as_bytes())
            .unwrap();
        evidence.write_all(b"\n").unwrap();
        evidence.sync_all().unwrap();
        writer_lease
            .unlock()
            .expect("release cooperative writer lease after report sync");
        assert!(result.is_ok(), "C02A failed; durable evidence: {output}");
    }
}
