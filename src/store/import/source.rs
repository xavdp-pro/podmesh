//! Version10 source inspection on an already-private stable snapshot, using raw SQLite classes.
use super::{canonical, caps::Caps, refusal};
use crate::store::{migrations, DurableStore, Engine, Result, SqliteStore, Value};
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;

pub const SOURCE_VERSION: usize = 10;

pub fn quoted(name: &str) -> Result<String> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(refusal("unexpected_identifier"));
    }
    Ok(format!("`{name}`"))
}

fn sql_error(_: rusqlite::Error) -> crate::store::StoreError { refusal("source_sqlite_read_failed") }

fn meta_rows(db: &Connection, sql: &str) -> Result<Json> {
    let mut statement = db.prepare(sql).map_err(sql_error)?;
    let count = statement.column_count();
    let mut rows = statement.query([]).map_err(sql_error)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let mut values = Vec::new();
        for column in 0..count {
            values.push(match row.get_ref(column).map_err(sql_error)? {
                ValueRef::Null => Json::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Text(bytes) => json!(std::str::from_utf8(bytes).map_err(|_| refusal("invalid_utf8_text"))?),
                _ => return Err(refusal("unexpected_schema_scalar_class")),
            });
        }
        out.push(json!(values));
    }
    Ok(json!(out))
}

pub fn shape(db: &Connection) -> Result<Json> {
    let objects = meta_rows(db, "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")?;
    let names = meta_rows(db, "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let mut tables = serde_json::Map::new();
    for row in names.as_array().ok_or_else(|| refusal("source_shape_failed"))? {
        let name = row[0].as_str().ok_or_else(|| refusal("source_shape_failed"))?;
        let indexes = meta_rows(db, &format!("PRAGMA index_list({})", quoted(name)?))?;
        let mut indexed = Vec::new();
        for index in indexes.as_array().ok_or_else(|| refusal("source_shape_failed"))? {
            let index_name = index[1].as_str().ok_or_else(|| refusal("source_shape_failed"))?;
            // sqlite_autoindex names are recognized by the exact expected shape, not ignored.
            indexed.push(json!({"definition": index, "columns": meta_rows(db, &format!("PRAGMA index_xinfo({})", quoted(index_name)?))?}));
        }
        tables.insert(name.to_owned(), json!({
            "columns": meta_rows(db, &format!("PRAGMA table_xinfo({})", quoted(name)?))?,
            "indexes": indexed,
            "foreign_keys": meta_rows(db, &format!("PRAGMA foreign_key_list({})", quoted(name)?))?,
        }));
    }
    Ok(json!({"objects":objects,"tables":tables}))
}

pub fn expected_shape() -> Result<Json> {
    let mut db = SqliteStore::open_in_memory()?;
    migrations::apply_set(&mut db, migrations::NODE, &migrations::NODE_MIGRATIONS[..SOURCE_VERSION])?;
    shape(db.connection())
}

#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub hash: String,
}

impl Table {
    pub fn select(&self) -> Result<String> {
        Ok(format!("SELECT {} FROM {}", self.columns.iter().map(|n| quoted(n)).collect::<Result<Vec<_>>>()?.join(","), quoted(&self.name)?))
    }
    pub fn insert(&self) -> Result<String> {
        Ok(format!("INSERT INTO {}({}) VALUES({})", quoted(&self.name)?, self.columns.iter().map(|n| quoted(n)).collect::<Result<Vec<_>>>()?.join(","), vec!["?"; self.columns.len()].join(",")))
    }
    pub fn manifest(&self) -> Json {
        json!({"name":self.name,"columns":self.columns,"count":self.rows.len(),"canonical_sha256":self.hash})
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub tables: Vec<Table>,
    pub source_schema: Vec<Value>,
    pub shape_hash: String,
    pub scalar_bytes: u64,
    pub rows: u64,
    pub cells: u64,
    pub estimated_peak: u64,
}

fn secret_material(text: &str) -> bool {
    fn json_secret(value: &Json) -> bool {
        match value {
            Json::Object(map) => map.iter().any(|(key,value)| {
                let sensitive = matches!(key.to_ascii_lowercase().as_str(), "password" | "private_key" | "access_token" | "token" | "client_secret");
                (sensitive && value.as_str().is_some_and(|s| !s.is_empty())) || json_secret(value)
            }),
            Json::Array(values) => values.iter().any(json_secret),
            _ => false,
        }
    }
    (text.contains("-----BEGIN ") && text.contains("PRIVATE KEY-----"))
        || text.contains("PODMESH-GENUINE-SECRET-PROBE:")
        || serde_json::from_str(text).map(|v| json_secret(&v)).unwrap_or(false)
}

pub fn raw_scalar(value: ValueRef<'_>, declared: &str, nullable: bool, limit: u64) -> Result<Value> {
    let out = match value {
        ValueRef::Null if nullable => Value::Null,
        ValueRef::Null => return Err(refusal("null_in_nonnullable_column")),
        ValueRef::Integer(n) if declared == "INTEGER" => Value::Integer(n),
        ValueRef::Integer(_) => return Err(refusal("unexpected_storage_class_integer")),
        ValueRef::Real(_) => return Err(refusal("unsupported_storage_class_real")),
        ValueRef::Blob(_) => return Err(refusal("unsupported_storage_class_blob")),
        ValueRef::Text(bytes) => {
            let text = std::str::from_utf8(bytes).map_err(|_| refusal("invalid_utf8_text"))?;
            if declared != "TEXT" { return Err(refusal("unexpected_storage_class_text")); }
            if u64::try_from(bytes.len()).map_err(|_| refusal("resource_counter_overflow"))? > limit { return Err(refusal("source_limit_exceeded")); }
            if secret_material(text) { return Err(refusal("genuine_secret_material_refused")); }
            Value::Text(text.to_owned())
        }
    };
    if scalar_size(&out)? > limit { return Err(refusal("source_limit_exceeded")); }
    Ok(out)
}

fn scalar_size(value: &Value) -> Result<u64> {
    Ok(match value {
        Value::Null => 0,
        Value::Integer(_) | Value::Real(_) => 8,
        Value::Text(s) => u64::try_from(s.len()).map_err(|_| refusal("resource_counter_overflow"))?,
        Value::Blob(b) => u64::try_from(b.len()).map_err(|_| refusal("resource_counter_overflow"))?,
    })
}

fn widths() -> BTreeMap<(String, String), usize> {
    let mut widths = BTreeMap::new();
    for migration in &migrations::NODE_MIGRATIONS[..SOURCE_VERSION] {
        let mut table = "";
        for line in migration.sql(Engine::Mariadb).lines() {
            if let Some(name) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") { table = name.split('(').next().unwrap_or(""); }
            let words: Vec<_> = line.split_whitespace().collect();
            if words.len() > 1 {
                if let Some(n) = words[1].strip_prefix("VARCHAR(").and_then(|s| s.strip_suffix(')')).and_then(|s| s.parse().ok()) {
                    widths.insert((table.into(), words[0].trim_matches('`').into()), n);
                }
            }
        }
    }
    widths.insert(("metadata".into(), "key".into()), 768);
    widths.remove(&("network_effects".into(), "intent".into()));
    widths
}

pub fn inspect(db: &Connection, caps: &Caps, snapshot_bytes: u64) -> Result<Plan> {
    caps.check(snapshot_bytes, 0, 0, 0)?;
    let actual_shape = shape(db)?;
    let expected = expected_shape()?;
    if actual_shape != expected { return Err(refusal("unexpected_source_shape")); }
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(sql_error)?;
    if integrity != "ok" { return Err(refusal("source_integrity_failed")); }
    let control = meta_rows(db, "SELECT name,version,applied_at FROM store_schema ORDER BY name")?;
    let rows = control.as_array().ok_or_else(|| refusal("unexpected_source_version"))?;
    if rows.len() != 1 || rows[0][0] != "node" || rows[0][1] != 10 || rows[0][2].as_i64().is_none() {
        return Err(refusal("unexpected_source_version"));
    }
    let source_schema = vec![Value::Text("node".into()), Value::Integer(10), Value::Integer(rows[0][2].as_i64().unwrap())];
    let limits = widths();
    let mut plan = Plan { tables:Vec::new(), source_schema, shape_hash:canonical::hash(serde_json::to_string(&actual_shape).unwrap().as_bytes()), scalar_bytes:0, rows:0, cells:0, estimated_peak:0 };
    for (name, definition) in expected["tables"].as_object().ok_or_else(|| refusal("source_shape_failed"))? {
        if name == "store_schema" { continue; }
        let definitions = definition["columns"].as_array().ok_or_else(|| refusal("source_shape_failed"))?;
        let columns: Vec<String> = definitions.iter().map(|d| d[1].as_str().unwrap().to_string()).collect();
        let pk_count = definitions.iter().filter(|d| d[5].as_i64().unwrap_or(0) > 0).count();
        let mut table = Table { name:name.clone(), columns, rows:Vec::new(), hash:String::new() };
        let mut statement = db.prepare(&table.select()?).map_err(sql_error)?;
        let mut queried = statement.query([]).map_err(sql_error)?;
        let mut table_bytes = 0_u64;
        while let Some(row) = queried.next().map_err(sql_error)? {
            if table.rows.len() as u64 >= caps.table_rows { return Err(refusal("source_limit_exceeded")); }
            let mut values = Vec::new();
            for (i, definition) in definitions.iter().enumerate() {
                let kind = definition[2].as_str().unwrap();
                let is_pk = definition[5].as_i64().unwrap_or(0) > 0;
                let nullable = definition[3] == 0 && !is_pk;
                let value = raw_scalar(row.get_ref(i).map_err(sql_error)?, kind, nullable, caps.value_bytes)?;
                if let Value::Text(text) = &value {
                    if limits.get(&(name.clone(),table.columns[i].clone())).is_some_and(|limit| text.chars().count() > *limit) { return Err(refusal("target_text_domain_exceeded")); }
                }
                if is_pk && pk_count == 1 && kind == "INTEGER" && value == Value::Integer(i64::MAX) { return Err(refusal("rowid_exhaustion_continuation_unsupported")); }
                let bytes = scalar_size(&value)?;
                table_bytes = table_bytes.checked_add(bytes).ok_or_else(|| refusal("resource_counter_overflow"))?;
                plan.scalar_bytes = plan.scalar_bytes.checked_add(bytes).ok_or_else(|| refusal("resource_counter_overflow"))?;
                plan.cells = plan.cells.checked_add(1).ok_or_else(|| refusal("resource_counter_overflow"))?;
                if table_bytes > caps.table_bytes { return Err(refusal("source_limit_exceeded")); }
                caps.check(snapshot_bytes, plan.scalar_bytes, plan.rows, plan.cells)?;
                values.push(value);
            }
            plan.rows = plan.rows.checked_add(1).ok_or_else(|| refusal("resource_counter_overflow"))?;
            plan.estimated_peak = caps.check(snapshot_bytes,plan.scalar_bytes,plan.rows,plan.cells)?;
            table.rows.push(values);
        }
        table.hash = canonical::table_hash(name,&table.columns,&table.rows)?;
        plan.tables.push(table);
    }
    if plan.tables.len() != 38 || plan.tables.iter().any(|t| t.rows.is_empty()) { return Err(refusal("synthetic_fixture_requires_38_populated_tables")); }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_text_blob_real_and_null_have_original_class_reasons() {
        let db = Connection::open_in_memory().unwrap();
        let mut statement = db.prepare("SELECT CAST(x'80' AS TEXT),x'80',0.5,NULL").unwrap();
        let mut rows = statement.query([]).unwrap(); let row = rows.next().unwrap().unwrap();
        for (i,code) in ["invalid_utf8_text","unsupported_storage_class_blob","unsupported_storage_class_real","null_in_nonnullable_column"].iter().enumerate() {
            assert_eq!(raw_scalar(row.get_ref(i).unwrap(),"TEXT",false,100).unwrap_err().message,*code);
        }
        assert_eq!(raw_scalar(ValueRef::Text(b"a\0b"),"TEXT",false,3).unwrap(),Value::Text("a\0b".into()));
    }

    #[test]
    fn raw_secret_and_shape_refusals_are_scrubbed() {
        let payload = b"PODMESH-GENUINE-SECRET-PROBE:do-not-log";
        let error = raw_scalar(ValueRef::Text(payload),"TEXT",false,100).unwrap_err();
        assert_eq!(error.message,"genuine_secret_material_refused");
        assert!(!error.message.contains("do-not-log"));
        let mut db = SqliteStore::open_in_memory().unwrap();
        migrations::apply_set(&mut db,migrations::NODE,&migrations::NODE_MIGRATIONS[..10]).unwrap();
        let expected = shape(db.connection()).unwrap();
        db.execute_batch("CREATE VIEW unexpected AS SELECT 1;").unwrap();
        assert_ne!(shape(db.connection()).unwrap(),expected);
    }
}
