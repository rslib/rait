use calamine::Data;
use serde_json::{Number, Value};

pub use rait_core::cells::{header, parse};

pub fn from_data(cell: &Data) -> Value {
    match cell {
        Data::Empty => Value::Null,
        Data::Int(n) => Value::Number((*n).into()),
        Data::Float(f) => float(*f),
        Data::String(s) => {
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s.clone())
            }
        }
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(dt) if dt.format("%H:%M:%S").to_string() == "00:00:00" => {
                Value::String(dt.date().format("%Y-%m-%d").to_string())
            }
            Some(dt) => Value::String(dt.format("%Y-%m-%dT%H:%M:%S").to_string()),
            None => float(dt.as_f64()),
        },
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::String(s.clone()),
        Data::Error(err) => Value::String(format!("#{err:?}")),
    }
}

/// Spreadsheets store every number as a float; show whole ones as integers.
fn float(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        return Value::Number((f as i64).into());
    }
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// The kind of a value, for schema inference.
pub fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "empty",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(s) if is_date(s) => "date",
        Value::String(s)
            if s.len() >= 19 && is_date(&s[..10]) && matches!(&s[10..11], "T" | " ") =>
        {
            "datetime"
        }
        _ => "string",
    }
}

/// Combine the kinds seen so far with one more value's kind.
pub fn merge(seen: &'static str, next: &'static str) -> &'static str {
    match (seen, next) {
        (a, "empty") => a,
        ("empty", b) => b,
        (a, b) if a == b => a,
        ("int", "float") | ("float", "int") => "float",
        ("date", "datetime") | ("datetime", "date") => "datetime",
        _ => "mixed",
    }
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_kinds() {
        assert_eq!(kind(&json!("2026-01-01")), "date");
        assert_eq!(kind(&json!("2026-01-01T10:00:00")), "datetime");
        assert_eq!(merge(merge("empty", "int"), "float"), "float");
        assert_eq!(merge("int", "string"), "mixed");
    }
}
