//! The driver-agnostic result shape every backend decodes into, so the grid
//! and the export paths never learn a driver's wire types.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use chrono::{DateTime, NaiveDate, Utc};

/// A single cell value, decoded from a driver into a driver-agnostic shape.
///
/// Integer widths are normalized (all signed ints to `Int`, unsigned to
/// `UInt`) except 128-bit, which cannot losslessly widen. The original
/// column type string is preserved separately in [`ColumnMeta`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Int128(i128),
    UInt128(u128),
    Float(f64),
    /// Fixed-point number: `value / 10^scale`.
    Decimal {
        value: i128,
        scale: u8,
    },
    String(String),
    /// Non-UTF-8 string/fixed-string payloads.
    Bytes(Vec<u8>),
    Uuid([u8; 16]),
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
    /// Enum value resolved to its symbolic name.
    Enum(String),
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
    Array(Vec<Value>),
    Tuple(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Int128(v) => write!(f, "{v}"),
            Value::UInt128(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Decimal { value, scale } => {
                let scale = *scale as u32;
                if scale == 0 {
                    return write!(f, "{value}");
                }
                let divisor = 10i128.pow(scale);
                let int = value / divisor;
                let frac = (value % divisor).unsigned_abs();
                let sign = if *value < 0 && int == 0 { "-" } else { "" };
                write!(f, "{sign}{int}.{frac:0>width$}", width = scale as usize)
            }
            Value::String(v) => write!(f, "{v}"),
            Value::Bytes(v) => {
                for b in v {
                    write!(f, "\\x{b:02x}")?;
                }
                Ok(())
            }
            Value::Uuid(b) => write!(
                f,
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
            ),
            Value::Date(v) => write!(f, "{v}"),
            Value::DateTime(v) => write!(f, "{}", v.format("%Y-%m-%d %H:%M:%S%.f")),
            Value::Enum(v) => write!(f, "{v}"),
            Value::Ipv4(v) => write!(f, "{v}"),
            Value::Ipv6(v) => write!(f, "{v}"),
            Value::Array(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Value::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, ")")
            }
            Value::Map(pairs) => {
                write!(f, "{{")?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

/// Column metadata as reported by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: String,
    /// The server's type string, verbatim (e.g. `Nullable(DateTime64(3))`).
    pub type_name: String,
}

/// A fully materialized query result. Streaming arrives in M7.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_display() {
        let d = |value, scale| Value::Decimal { value, scale }.to_string();
        assert_eq!(d(123456, 4), "12.3456");
        assert_eq!(d(-123456, 4), "-12.3456");
        assert_eq!(d(-56, 4), "-0.0056");
        assert_eq!(d(5, 0), "5");
        assert_eq!(d(10, 1), "1.0");
    }
}
