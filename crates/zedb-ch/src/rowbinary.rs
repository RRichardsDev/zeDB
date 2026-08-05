//! Decoder for ClickHouse's `RowBinaryWithNamesAndTypes` output format.
//!
//! Layout: varuint column count, then names, then type strings (each
//! length-prefixed), then rows until EOF. Values are little-endian.

use chrono::{DateTime, Duration, NaiveDate};
use zedb_core::{ColumnMeta, QueryResult, Value};

use crate::error::{ChError, Result};
use crate::types::{parse_type, ChType};

pub fn decode(buf: &[u8]) -> Result<QueryResult> {
    let mut r = Reader { buf, pos: 0 };
    let n_cols = r.varuint()? as usize;
    let mut names = Vec::with_capacity(n_cols);
    for _ in 0..n_cols {
        names.push(r.string()?);
    }
    let mut type_names = Vec::with_capacity(n_cols);
    let mut types = Vec::with_capacity(n_cols);
    for _ in 0..n_cols {
        let s = r.string()?;
        types.push(parse_type(&s)?);
        type_names.push(s);
    }

    let mut rows = Vec::new();
    while !r.at_end() {
        let mut row = Vec::with_capacity(n_cols);
        for ty in &types {
            row.push(read_value(&mut r, ty)?);
        }
        rows.push(row);
    }

    let columns = names
        .into_iter()
        .zip(type_names)
        .map(|(name, type_name)| ColumnMeta { name, type_name })
        .collect();
    Ok(QueryResult { columns, rows })
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            return Err(ChError::Decode(format!(
                "unexpected end of data at offset {} (wanted {n} bytes of {})",
                self.pos,
                self.buf.len()
            )));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn varuint(&mut self) -> Result<u64> {
        let mut out: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            out |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
        }
        Err(ChError::Decode("varuint too long".into()))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.varuint()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| ChError::Decode("invalid utf-8 in header string".into()))
    }
}

macro_rules! le {
    ($r:expr, $ty:ty) => {{
        let bytes = $r.take(std::mem::size_of::<$ty>())?;
        <$ty>::from_le_bytes(bytes.try_into().unwrap())
    }};
}

fn read_value(r: &mut Reader, ty: &ChType) -> Result<Value> {
    Ok(match ty {
        ChType::UInt8 => Value::UInt(r.u8()?.into()),
        ChType::UInt16 => Value::UInt(le!(r, u16).into()),
        ChType::UInt32 => Value::UInt(le!(r, u32).into()),
        ChType::UInt64 => Value::UInt(le!(r, u64)),
        ChType::UInt128 => Value::UInt128(le!(r, u128)),
        ChType::Int8 => Value::Int(le!(r, i8).into()),
        ChType::Int16 => Value::Int(le!(r, i16).into()),
        ChType::Int32 => Value::Int(le!(r, i32).into()),
        ChType::Int64 => Value::Int(le!(r, i64)),
        ChType::Int128 => Value::Int128(le!(r, i128)),
        ChType::Float32 => Value::Float(le!(r, f32).into()),
        ChType::Float64 => Value::Float(le!(r, f64)),
        ChType::Bool => Value::Bool(r.u8()? != 0),
        ChType::String => {
            let len = r.varuint()? as usize;
            bytes_to_value(r.take(len)?)
        }
        ChType::FixedString(n) => bytes_to_value(r.take(*n)?),
        ChType::Uuid => {
            // Stored as two little-endian u64 halves; reverse each half to
            // recover RFC byte order.
            let raw = r.take(16)?;
            let mut b = [0u8; 16];
            for i in 0..8 {
                b[i] = raw[7 - i];
                b[8 + i] = raw[15 - i];
            }
            Value::Uuid(b)
        }
        ChType::Date => {
            let days = le!(r, u16);
            Value::Date(epoch_date() + Duration::days(days.into()))
        }
        ChType::Date32 => {
            let days = le!(r, i32);
            Value::Date(epoch_date() + Duration::days(days.into()))
        }
        ChType::DateTime { .. } => {
            let secs = le!(r, u32);
            Value::DateTime(
                DateTime::from_timestamp(secs.into(), 0)
                    .ok_or_else(|| ChError::Decode("DateTime out of range".into()))?,
            )
        }
        ChType::DateTime64 { precision, .. } => {
            let ticks = le!(r, i64);
            let p = u32::from(*precision);
            let per_sec = 10i64.pow(p);
            let secs = ticks.div_euclid(per_sec);
            let frac = ticks.rem_euclid(per_sec) as u32;
            let nanos = frac * 10u32.pow(9 - p);
            Value::DateTime(
                DateTime::from_timestamp(secs, nanos)
                    .ok_or_else(|| ChError::Decode("DateTime64 out of range".into()))?,
            )
        }
        ChType::Decimal { precision, scale } => {
            let value: i128 = if *precision <= 9 {
                le!(r, i32).into()
            } else if *precision <= 18 {
                le!(r, i64).into()
            } else if *precision <= 38 {
                le!(r, i128)
            } else {
                return Err(ChError::UnsupportedType(format!(
                    "Decimal({precision}, {scale})"
                )));
            };
            Value::Decimal {
                value,
                scale: *scale,
            }
        }
        ChType::Enum8(entries) => {
            let v: i16 = le!(r, i8).into();
            enum_name(entries, v)?
        }
        ChType::Enum16(entries) => {
            let v = le!(r, i16);
            enum_name(entries, v)?
        }
        ChType::Ipv4 => Value::Ipv4(std::net::Ipv4Addr::from(le!(r, u32))),
        ChType::Ipv6 => {
            let b: [u8; 16] = r.take(16)?.try_into().unwrap();
            Value::Ipv6(std::net::Ipv6Addr::from(b))
        }
        ChType::Nullable(inner) => {
            if r.u8()? != 0 {
                Value::Null
            } else {
                read_value(r, inner)?
            }
        }
        // RowBinary serializes LowCardinality columns as their inner type.
        ChType::LowCardinality(inner) => read_value(r, inner)?,
        ChType::Array(inner) => {
            let len = r.varuint()? as usize;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(read_value(r, inner)?);
            }
            Value::Array(items)
        }
        ChType::Tuple(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item_ty in items {
                out.push(read_value(r, item_ty)?);
            }
            Value::Tuple(out)
        }
        ChType::Map(key, value) => {
            let len = r.varuint()? as usize;
            let mut pairs = Vec::with_capacity(len);
            for _ in 0..len {
                let k = read_value(r, key)?;
                let v = read_value(r, value)?;
                pairs.push((k, v));
            }
            Value::Map(pairs)
        }
    })
}

fn bytes_to_value(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(s) => Value::String(s.to_string()),
        Err(_) => Value::Bytes(bytes.to_vec()),
    }
}

fn enum_name(entries: &[(String, i16)], v: i16) -> Result<Value> {
    entries
        .iter()
        .find(|(_, ev)| *ev == v)
        .map(|(name, _)| Value::Enum(name.clone()))
        .ok_or_else(|| ChError::Decode(format!("enum value {v} not in type definition")))
}

fn epoch_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(cols: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = vec![cols.len() as u8];
        for (name, _) in cols {
            buf.push(name.len() as u8);
            buf.extend_from_slice(name.as_bytes());
        }
        for (_, ty) in cols {
            buf.push(ty.len() as u8);
            buf.extend_from_slice(ty.as_bytes());
        }
        buf
    }

    #[test]
    fn header_and_scalar_row() {
        let mut buf = header(&[("id", "UInt64"), ("name", "String")]);
        buf.extend_from_slice(&7u64.to_le_bytes());
        buf.extend_from_slice(&[2, b'h', b'i']);
        let result = decode(&buf).unwrap();
        assert_eq!(result.columns[0].name, "id");
        assert_eq!(result.columns[1].type_name, "String");
        assert_eq!(
            result.rows,
            vec![vec![Value::UInt(7), Value::String("hi".into())]]
        );
    }

    #[test]
    fn nullable_and_array() {
        let mut buf = header(&[("v", "Array(Nullable(UInt8))")]);
        // Array of 3: [1, NULL, 3]
        buf.extend_from_slice(&[3, 0, 1, 1, 0, 3]);
        let result = decode(&buf).unwrap();
        assert_eq!(
            result.rows[0][0],
            Value::Array(vec![Value::UInt(1), Value::Null, Value::UInt(3)])
        );
    }

    #[test]
    fn truncated_input_is_an_error() {
        let mut buf = header(&[("id", "UInt64")]);
        buf.extend_from_slice(&[1, 2, 3]); // only 3 of 8 bytes
        assert!(matches!(decode(&buf), Err(ChError::Decode(_))));
    }
}
