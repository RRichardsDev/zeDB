//! Decoder for ClickHouse's `RowBinaryWithNamesAndTypes` output format.
//!
//! Layout: varuint column count, then names, then type strings (each
//! length-prefixed), then rows until EOF. Values are little-endian.

use chrono::{DateTime, Duration, NaiveDate};
use zedb_core::{ColumnMeta, QueryResult, Value};

use crate::error::{ChError, Result};
use crate::types::{parse_type, ChType};

const MAX_MATERIALIZED_RESPONSE_BYTES: usize = 1024 * 1024 * 1024;
const MAX_STREAM_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const MAX_COLUMNS: usize = 16_384;
const MAX_HEADER_STRING_BYTES: usize = 64 * 1024;
const MAX_VALUE_BYTES: usize = 64 * 1024 * 1024;
const MAX_COLLECTION_ITEMS: usize = 100_000;
const MAX_DECODED_VALUES: usize = 2_000_000;
const MAX_VALUE_DEPTH: usize = 64;

#[derive(Clone, Copy)]
struct DecodeBudget {
    remaining_values: usize,
}

impl DecodeBudget {
    fn new() -> Self {
        Self {
            remaining_values: MAX_DECODED_VALUES,
        }
    }

    fn consume(&mut self, count: usize) -> Result<()> {
        self.remaining_values = self.remaining_values.checked_sub(count).ok_or_else(|| {
            ChError::Decode(format!(
                "decoded value count exceeds limit of {MAX_DECODED_VALUES}"
            ))
        })?;
        Ok(())
    }

    fn ensure(&self, count: usize) -> Result<()> {
        if count > self.remaining_values {
            return Err(ChError::Decode(format!(
                "decoded value count exceeds limit of {MAX_DECODED_VALUES}"
            )));
        }
        Ok(())
    }
}

/// Incrementally decodes a `RowBinaryWithNamesAndTypes` response.
///
/// ClickHouse does not frame individual rows, so an incomplete row is retained
/// until the next network chunk arrives. Complete rows are returned immediately.
pub(crate) struct StreamingDecoder {
    buffer: Vec<u8>,
    columns: Option<Vec<ColumnMeta>>,
    types: Vec<ChType>,
    budget: DecodeBudget,
}

impl StreamingDecoder {
    pub(crate) fn new() -> Self {
        Self {
            buffer: Vec::new(),
            columns: None,
            types: Vec::new(),
            budget: DecodeBudget::new(),
        }
    }

    pub(crate) fn columns(&self) -> Option<&[ColumnMeta]> {
        self.columns.as_deref()
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<Value>>> {
        let buffered = self
            .buffer
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| ChError::Decode("stream buffer length overflow".into()))?;
        if buffered > MAX_STREAM_BUFFER_BYTES {
            return Err(ChError::Decode(format!(
                "incomplete RowBinary data exceeds {MAX_STREAM_BUFFER_BYTES} byte stream buffer limit"
            )));
        }
        self.buffer.extend_from_slice(chunk);
        if self.columns.is_none() && !self.try_decode_header()? {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut consumed = 0;
        loop {
            let mut reader = Reader {
                buf: &self.buffer,
                pos: consumed,
            };
            let row_start = reader.pos;
            let mut row = Vec::with_capacity(self.types.len());
            let mut row_budget = self.budget;
            for ty in &self.types {
                match read_value(&mut reader, ty, &mut row_budget, 0) {
                    Ok(value) => row.push(value),
                    Err(error) if is_incomplete(&error) => {
                        self.buffer.drain(..consumed);
                        return Ok(rows);
                    }
                    Err(error) => return Err(error),
                }
            }
            if reader.pos == row_start {
                break;
            }
            consumed = reader.pos;
            self.budget = row_budget;
            rows.push(row);
            if consumed == self.buffer.len() {
                break;
            }
        }
        self.buffer.drain(..consumed);
        Ok(rows)
    }

    pub(crate) fn finish(self) -> Result<()> {
        if self.columns.is_none() {
            // DDL and other resultless statements return a completely
            // empty body; no header only means truncation when bytes
            // actually arrived.
            if self.buffer.is_empty() {
                return Ok(());
            }
            return Err(ChError::Decode("response ended before its header".into()));
        }
        if !self.buffer.is_empty() {
            return Err(ChError::Decode(format!(
                "response ended with {} bytes of an incomplete row",
                self.buffer.len()
            )));
        }
        Ok(())
    }

    fn try_decode_header(&mut self) -> Result<bool> {
        let mut reader = Reader {
            buf: &self.buffer,
            pos: 0,
        };
        let result = (|| {
            let n_cols = reader.bounded_len("column count", MAX_COLUMNS)?;
            let mut names = Vec::with_capacity(n_cols);
            for _ in 0..n_cols {
                names.push(reader.string(MAX_HEADER_STRING_BYTES)?);
            }
            let mut type_names = Vec::with_capacity(n_cols);
            let mut types = Vec::with_capacity(n_cols);
            for _ in 0..n_cols {
                let type_name = reader.string(MAX_HEADER_STRING_BYTES)?;
                types.push(parse_type(&type_name)?);
                type_names.push(type_name);
            }
            Ok::<_, ChError>((names, type_names, types))
        })();

        match result {
            Ok((names, type_names, types)) => {
                let columns = names
                    .into_iter()
                    .zip(type_names)
                    .map(|(name, type_name)| ColumnMeta { name, type_name })
                    .collect();
                self.types = types;
                self.columns = Some(columns);
                self.buffer.drain(..reader.pos);
                Ok(true)
            }
            Err(error) if is_incomplete(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

fn is_incomplete(error: &ChError) -> bool {
    matches!(error, ChError::Decode(message) if message.starts_with("unexpected end of data"))
}

pub fn decode(buf: &[u8]) -> Result<QueryResult> {
    if buf.len() > MAX_MATERIALIZED_RESPONSE_BYTES {
        return Err(ChError::Decode(format!(
            "RowBinary response exceeds {MAX_MATERIALIZED_RESPONSE_BYTES} byte limit"
        )));
    }
    let mut r = Reader { buf, pos: 0 };
    let n_cols = r.bounded_len("column count", MAX_COLUMNS)?;
    let mut names = Vec::with_capacity(n_cols);
    for _ in 0..n_cols {
        names.push(r.string(MAX_HEADER_STRING_BYTES)?);
    }
    let mut type_names = Vec::with_capacity(n_cols);
    let mut types = Vec::with_capacity(n_cols);
    for _ in 0..n_cols {
        let s = r.string(MAX_HEADER_STRING_BYTES)?;
        types.push(parse_type(&s)?);
        type_names.push(s);
    }

    let mut rows = Vec::new();
    let mut budget = DecodeBudget::new();
    while !r.at_end() {
        let row_start = r.pos;
        let mut row = Vec::with_capacity(n_cols);
        for ty in &types {
            row.push(read_value(&mut r, ty, &mut budget, 0)?);
        }
        if r.pos == row_start {
            return Err(ChError::Decode(
                "RowBinary data remains after a zero-column header".into(),
            ));
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
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| ChError::Decode("input offset overflow".into()))?;
        if end > self.buf.len() {
            return Err(ChError::Decode(format!(
                "unexpected end of data at offset {} (wanted {n} bytes of {})",
                self.pos,
                self.buf.len()
            )));
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn varuint(&mut self) -> Result<u64> {
        let mut out: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            if shift == 63 && b > 1 {
                return Err(ChError::Decode("varuint overflows u64".into()));
            }
            out |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
        }
        Err(ChError::Decode("varuint too long".into()))
    }

    fn bounded_len(&mut self, label: &str, max: usize) -> Result<usize> {
        let raw = self.varuint()?;
        let len = usize::try_from(raw)
            .map_err(|_| ChError::Decode(format!("{label} does not fit in memory")))?;
        if len > max {
            return Err(ChError::Decode(format!(
                "{label} {len} exceeds limit of {max}"
            )));
        }
        Ok(len)
    }

    fn string(&mut self, max: usize) -> Result<String> {
        let len = self.bounded_len("header string length", max)?;
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

fn read_value(
    r: &mut Reader,
    ty: &ChType,
    budget: &mut DecodeBudget,
    depth: usize,
) -> Result<Value> {
    if depth >= MAX_VALUE_DEPTH {
        return Err(ChError::Decode(format!(
            "value nesting exceeds limit of {MAX_VALUE_DEPTH}"
        )));
    }
    budget.consume(1)?;
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
            let len = r.bounded_len("string length", MAX_VALUE_BYTES)?;
            bytes_to_value(r.take(len)?)
        }
        // The driver requests JSON as its string form
        // (output_format_binary_write_json_as_string).
        ChType::Json => {
            let len = r.bounded_len("JSON length", MAX_VALUE_BYTES)?;
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
                read_value(r, inner, budget, depth + 1)?
            }
        }
        // RowBinary serializes LowCardinality columns as their inner type.
        ChType::LowCardinality(inner) => read_value(r, inner, budget, depth + 1)?,
        ChType::Array(inner) => {
            let len = r.bounded_len("array length", MAX_COLLECTION_ITEMS)?;
            budget.ensure(len)?;
            let mut items = Vec::with_capacity(len.min(1024));
            for _ in 0..len {
                items.push(read_value(r, inner, budget, depth + 1)?);
            }
            Value::Array(items)
        }
        ChType::Tuple(items) => {
            budget.ensure(items.len())?;
            let mut out = Vec::with_capacity(items.len().min(1024));
            for item_ty in items {
                out.push(read_value(r, item_ty, budget, depth + 1)?);
            }
            Value::Tuple(out)
        }
        ChType::Map(key, value) => {
            let len = r.bounded_len("map length", MAX_COLLECTION_ITEMS)?;
            let child_values = len
                .checked_mul(2)
                .ok_or_else(|| ChError::Decode("map value count overflow".into()))?;
            budget.ensure(child_values)?;
            let mut pairs = Vec::with_capacity(len.min(1024));
            for _ in 0..len {
                let k = read_value(r, key, budget, depth + 1)?;
                let v = read_value(r, value, budget, depth + 1)?;
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

    fn varuint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return out;
            }
        }
    }

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
    fn empty_body_finishes_cleanly() {
        // DDL statements return no bytes at all; that is a valid,
        // resultless response, not a truncated one.
        let decoder = StreamingDecoder::new();
        decoder.finish().expect("empty body is not an error");

        let mut partial = StreamingDecoder::new();
        partial.push(&[3]).ok();
        assert!(partial.finish().is_err(), "partial header is truncation");
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

    #[test]
    fn rejects_attacker_controlled_lengths_before_allocation() {
        let too_many_columns = varuint((MAX_COLUMNS + 1) as u64);
        assert!(decode(&too_many_columns).is_err());

        let mut oversized_string = header(&[("value", "String")]);
        oversized_string.extend(varuint((MAX_VALUE_BYTES + 1) as u64));
        assert!(decode(&oversized_string).is_err());

        let mut oversized_array = header(&[("value", "Array(UInt8)")]);
        oversized_array.extend(varuint((MAX_COLLECTION_ITEMS + 1) as u64));
        assert!(decode(&oversized_array).is_err());
    }

    #[test]
    fn checked_reader_offsets_cannot_wrap() {
        let mut reader = Reader {
            buf: &[],
            pos: usize::MAX,
        };
        assert!(matches!(reader.take(1), Err(ChError::Decode(_))));
    }

    #[test]
    fn overflowing_varuint_is_rejected() {
        let bytes = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        let mut reader = Reader {
            buf: &bytes,
            pos: 0,
        };
        assert!(reader.varuint().is_err());
    }

    #[test]
    fn zero_column_header_with_trailing_data_is_rejected() {
        assert!(decode(&[0, 1]).is_err());
    }

    #[test]
    fn streaming_decoder_handles_every_chunk_boundary() {
        let mut buf = header(&[("id", "UInt64"), ("name", "String")]);
        buf.extend_from_slice(&7u64.to_le_bytes());
        buf.extend_from_slice(&[2, b'h', b'i']);
        buf.extend_from_slice(&9u64.to_le_bytes());
        buf.extend_from_slice(&[3, b'b', b'y', b'e']);

        for split in 0..=buf.len() {
            let mut decoder = StreamingDecoder::new();
            let mut rows = decoder.push(&buf[..split]).unwrap();
            rows.extend(decoder.push(&buf[split..]).unwrap());
            assert_eq!(decoder.columns().unwrap()[0].name, "id");
            decoder.finish().unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![Value::UInt(7), Value::String("hi".into())],
                    vec![Value::UInt(9), Value::String("bye".into())],
                ],
                "failed at chunk boundary {split}"
            );
        }
    }

    #[test]
    fn streaming_decoder_rejects_truncated_final_row() {
        let mut decoder = StreamingDecoder::new();
        let mut buf = header(&[("id", "UInt64")]);
        buf.extend_from_slice(&[1, 2, 3]);
        assert!(decoder.push(&buf).unwrap().is_empty());
        assert!(matches!(decoder.finish(), Err(ChError::Decode(_))));
    }
}
