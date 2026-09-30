//! Explicit finite synthetic limits; estimates are not OS/RSS enforcement.
use super::refusal;
use crate::store::Result;
use serde_json::Value as Json;

#[derive(Clone, Debug)]
pub struct Caps {
    pub bundle: u64, pub snapshot: u64, pub scalar_bytes: u64, pub rows: u64,
    pub table_rows: u64, pub table_bytes: u64, pub value_bytes: u64, pub peak: u64,
}

impl Caps {
    pub fn parse(value: &Json) -> Result<Self> {
        fn positive(value: &Json, name: &str) -> Result<u64> {
            value[name].as_u64().filter(|n| *n > 0).ok_or_else(|| refusal("invalid_resource_caps"))
        }
        let caps = Self {
            bundle: positive(value, "total_original_bundle_bytes")?, snapshot: positive(value, "total_snapshot_bytes")?,
            scalar_bytes: positive(value, "total_input_scalar_bytes")?, rows: positive(value, "total_rows")?,
            table_rows: positive(value, "per_table_rows")?, table_bytes: positive(value, "per_table_scalar_bytes")?,
            value_bytes: positive(value, "per_value_bytes")?, peak: positive(value, "estimated_peak_memory_bytes")?,
        };
        if caps.table_rows > caps.rows || caps.value_bytes > caps.table_bytes || caps.table_bytes > caps.scalar_bytes {
            return Err(refusal("inconsistent_resource_caps"));
        }
        // The accepted development ceilings may be reduced, never silently enlarged.
        if caps.bundle > 33_554_432 || caps.snapshot > 33_554_432 || caps.scalar_bytes > 16_777_216
            || caps.rows > 10_000 || caps.table_rows > 2_000 || caps.table_bytes > 2_097_152
            || caps.value_bytes > 262_144 || caps.peak > 536_870_912 {
            return Err(refusal("resource_caps_exceed_accepted_contract"));
        }
        Ok(caps)
    }

    pub fn check(&self, snapshot: u64, bytes: u64, rows: u64, cells: u64) -> Result<u64> {
        let add = |a: u64, b| a.checked_add(b).ok_or_else(|| refusal("resource_counter_overflow"));
        let mul = |a: u64, b| a.checked_mul(b).ok_or_else(|| refusal("resource_counter_overflow"));
        let peak = add(add(add(mul(add(snapshot, bytes)?, 8)?, mul(rows, 512)?)?, mul(cells, 128)?)?, 8_388_608)?;
        if snapshot > self.snapshot || bytes > self.scalar_bytes || rows > self.rows || peak > self.peak {
            return Err(refusal("source_limit_exceeded"));
        }
        Ok(peak)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Caps { Caps { bundle: 100, snapshot: 100, scalar_bytes: 100, rows: 100, table_rows: 10, table_bytes: 10, value_bytes: 5, peak: 9_000_000 } }

    #[test]
    fn exact_formula_overflow_and_limits_are_falsifiable() {
        assert_eq!(limits().check(10, 20, 2, 3).unwrap(), 8_388_608 + 240 + 1024 + 384);
        assert!(limits().check(101, 0, 0, 0).is_err());
        assert!(limits().check(0, 101, 0, 0).is_err());
        assert!(limits().check(0, 0, 101, 0).is_err());
        assert!(limits().check(u64::MAX, 1, 0, 0).is_err());
        assert!(Caps::parse(&serde_json::json!({})).is_err());
    }
}
