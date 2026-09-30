//! Bounded SQL batches used by the versioned store schema.
//!
//! Doubled quotes and whitespace-delimited `--` comments are supported. Backslashes in
//! quotes, `#` comments, block comments (including MySQL executable comments), ambiguous
//! `--` tokens and client DELIMITER directives are refused before any execution.
//! This is a restricted schema language, not a general SQL/client-script parser.
use super::{Fault, Result};

pub(crate) fn without_leading_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if sql.starts_with("--") {
            sql = sql.split_once('\n').map_or("", |(_, rest)| rest);
        } else {
            return sql;
        }
    }
}

fn append<'a>(sql: &'a str, statements: &mut Vec<&'a str>) -> Result<()> {
    let statement = without_leading_comments(sql).trim();
    if statement.is_empty() {
        return Ok(());
    }
    if statement.split_whitespace().next().is_some_and(|word| word.eq_ignore_ascii_case("DELIMITER")) {
        return Err(Fault::Schema.error("DELIMITER directives are not supported in store SQL batches"));
    }
    statements.push(statement);
    Ok(())
}

/// Validate the complete input before returning any statement for execution.
pub(crate) fn statements(sql: &str) -> Result<Vec<&str>> {
    let mut statements = Vec::new();
    let bytes = sql.as_bytes();
    let mut start = 0;
    let mut index = 0;
    let mut quote = None;
    let mut comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            index += 1;
            continue;
        }
        if let Some(open) = quote {
            if byte == b'\\' {
                return Err(Fault::Schema.error("backslashes in quotes are ambiguous across store SQL dialects"));
            }
            if byte == open {
                if bytes.get(index + 1) == Some(&open) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'#' => return Err(Fault::Schema.error("hash comments are not shared by store SQL dialects")),
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                if !bytes.get(index + 2).is_some_and(|next| next.is_ascii_whitespace() || next.is_ascii_control()) {
                    return Err(Fault::Schema.error("SQL dash comments require whitespace or control after --"));
                }
                comment = true;
                index += 2;
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                return Err(Fault::Schema.error("block comments are not supported in store SQL batches"));
            }
            b';' => {
                append(&sql[start..index], &mut statements)?;
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if quote.is_some() {
        return Err(Fault::Schema.error("unterminated quote in store SQL batch"));
    }
    append(&sql[start..], &mut statements)?;
    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_delimiters_and_line_comments_do_not_split_statements() {
        let split = statements("-- ignored; line\nSELECT 'one;two', 'it''s;quoted'; -- ignored;\nSELECT `a;b`, \"c;d\";").unwrap();
        assert_eq!(split, ["SELECT 'one;two', 'it''s;quoted'", "SELECT `a;b`, \"c;d\""]);
        assert!(statements(" ; -- tail;\n -- another;").unwrap().is_empty());
        assert_eq!(statements("SELECT '/* text; */';").unwrap(), ["SELECT '/* text; */'"]);
        assert_eq!(statements("SELECT '# literal', '--2';").unwrap(), ["SELECT '# literal', '--2'"]);
    }

    #[test]
    fn unsupported_script_syntax_is_refused_before_any_batch_is_returned() {
        for sql in [
            "SELECT 1; /* split; here */ SELECT 2;",
            "SELECT 1; /*! executable SQL */;",
            "DELIMITER $$\nSELECT 1$$",
            "SELECT 1; -- tail\n delimiter $$",
            "SELECT 'unterminated;",
            "SELECT 1; SELECT 'x\\'; CREATE TABLE hidden(v TEXT); -- ' ;",
            "SELECT 1; SELECT \"x\\\";",
            "SELECT 1; SELECT `x\\`;",
            "SELECT 1; SELECT 1--2; CREATE TABLE hidden(v TEXT);",
            "SELECT 1; --",
            "SELECT 1; # not a SQLite comment;",
        ] {
            assert_eq!(statements(sql).unwrap_err().fault, Fault::Schema, "{sql}");
        }
    }
}
