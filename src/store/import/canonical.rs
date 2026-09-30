//! Byte-exact contract podmesh-c02-canonical/1, pinned independently from C02A.
use super::refusal;
use crate::store::{Result, Value};
use sha2::{Digest, Sha256};

fn frame(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    let count = u64::try_from(bytes.len()).map_err(|_| refusal("canonical_length_overflow"))?;
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

pub fn row(values: &[Value]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for value in values {
        match value {
            Value::Null => out.push(0),
            Value::Integer(n) => { out.push(1); out.extend_from_slice(&n.to_be_bytes()); }
            Value::Real(n) => { out.push(2); out.extend_from_slice(&n.to_bits().to_be_bytes()); }
            Value::Text(text) => { out.push(3); frame(&mut out, text.as_bytes())?; }
            Value::Blob(bytes) => { out.push(4); frame(&mut out, bytes)?; }
        }
    }
    Ok(out)
}

pub fn sorted_rows(rows: &[Vec<Value>]) -> Result<Vec<Vec<u8>>> {
    let mut out = rows.iter().map(|values| row(values)).collect::<Result<Vec<_>>>()?;
    out.sort();
    Ok(out)
}

pub fn table(name: &str, columns: &[String], rows: &[Vec<Value>]) -> Result<Vec<u8>> {
    if rows.iter().any(|row| row.len() != columns.len()) {
        return Err(refusal("canonical_column_count_mismatch"));
    }
    let mut out = b"PODMESH-C02-CANONICAL\0\x01".to_vec();
    frame(&mut out, name.as_bytes())?;
    out.extend_from_slice(&u64::try_from(columns.len()).map_err(|_| refusal("canonical_length_overflow"))?.to_be_bytes());
    for column in columns { frame(&mut out, column.as_bytes())?; }
    out.extend_from_slice(&u64::try_from(rows.len()).map_err(|_| refusal("canonical_length_overflow"))?.to_be_bytes());
    for row in sorted_rows(rows)? { frame(&mut out, &row)?; }
    Ok(out)
}

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn table_hash(name: &str, columns: &[String], rows: &[Vec<Value>]) -> Result<String> {
    Ok(hash(&table(name, columns, rows)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_vector_retains_kinds_lengths_signed_bits_and_nul() {
        let encoded = row(&[Value::Null, Value::Integer(-1), Value::Real(-0.0), Value::Text("a\0".into()), Value::Blob(vec![128])]).unwrap();
        let mut expected = vec![0, 1];
        expected.extend([255; 8]);
        expected.push(2); expected.extend(0x8000000000000000_u64.to_be_bytes());
        expected.push(3); expected.extend(2_u64.to_be_bytes()); expected.extend([97, 0]);
        expected.push(4); expected.extend(1_u64.to_be_bytes()); expected.push(128);
        assert_eq!(encoded, expected);
        assert_ne!(row(&[Value::Text("é".into())]).unwrap(), row(&[Value::Text("e\u{301}".into())]).unwrap());
        assert_ne!(row(&[Value::Real(-0.0)]).unwrap(), row(&[Value::Real(0.0)]).unwrap());
    }

    #[test]
    fn row_order_is_irrelevant_but_duplicates_and_column_identity_are_not() {
        let columns = vec!["value".into()];
        let a = vec![vec![Value::Integer(1)], vec![Value::Null], vec![Value::Integer(1)]];
        let b = vec![a[2].clone(), a[0].clone(), a[1].clone()];
        assert_eq!(table("t", &columns, &a).unwrap(), table("t", &columns, &b).unwrap());
        assert_ne!(table_hash("t", &columns, &a).unwrap(), table_hash("t", &columns, &a[..2]).unwrap());
        assert_ne!(table_hash("t", &columns, &a).unwrap(), table_hash("t", &["other".into()], &a).unwrap());
    }
}
