//! Parser for ClickHouse type strings as reported by the server, e.g.
//! `Nullable(DateTime64(3, 'UTC'))` or `Enum8('a' = 1, 'b' = 2)`.
//!
//! The parser is deliberately strict: an unrecognized type becomes
//! [`ChError::UnsupportedType`] rather than a silent guess, so gaps in the
//! type zoo surface loudly.

use crate::error::{ChError, Result};

const MAX_TYPE_STRING_BYTES: usize = 64 * 1024;
const MAX_TYPE_DEPTH: usize = 64;
const MAX_FIXED_STRING_BYTES: usize = 64 * 1024 * 1024;
const MAX_TUPLE_ITEMS: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub enum ChType {
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    UInt128,
    Int8,
    Int16,
    Int32,
    Int64,
    Int128,
    Float32,
    Float64,
    Bool,
    String,
    FixedString(usize),
    Uuid,
    Date,
    Date32,
    DateTime {
        tz: Option<String>,
    },
    DateTime64 {
        precision: u8,
        tz: Option<String>,
    },
    /// All Decimal variants, normalized. Storage width follows precision.
    Decimal {
        precision: u8,
        scale: u8,
    },
    Enum8(Vec<(String, i16)>),
    Enum16(Vec<(String, i16)>),
    Ipv4,
    Ipv6,
    Nullable(Box<ChType>),
    /// Transparent in RowBinary: values are serialized as the inner type.
    LowCardinality(Box<ChType>),
    Array(Box<ChType>),
    Tuple(Vec<ChType>),
    Map(Box<ChType>, Box<ChType>),
    /// The JSON object type. The driver asks the server to serialize
    /// it as a string in RowBinary (output_format_binary_write_json_as_string),
    /// so the payload is the document's text. Type parameters
    /// (max_dynamic_paths, typed paths, SKIP) don't change that and
    /// are skipped unparsed.
    Json,
}

pub fn parse_type(input: &str) -> Result<ChType> {
    if input.len() > MAX_TYPE_STRING_BYTES {
        return Err(ChError::TypeParse {
            input: "<type string omitted>".into(),
            reason: format!("type string exceeds {MAX_TYPE_STRING_BYTES} byte limit"),
        });
    }
    let mut p = Parser {
        input,
        chars: input.char_indices().peekable(),
        depth: 0,
    };
    let ty = p.parse()?;
    p.skip_ws();
    match p.chars.next() {
        None => Ok(ty),
        Some((i, c)) => Err(p.err(format!("unexpected {c:?} at offset {i}"))),
    }
}

struct Parser<'a> {
    input: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, reason: String) -> ChError {
        ChError::TypeParse {
            input: self.input.to_string(),
            reason,
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.chars.peek(), Some((_, c)) if c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn eat(&mut self, expected: char) -> Result<()> {
        self.skip_ws();
        match self.chars.next() {
            Some((_, c)) if c == expected => Ok(()),
            other => Err(self.err(format!("expected {expected:?}, found {other:?}"))),
        }
    }

    fn peek_char(&mut self) -> Option<char> {
        self.skip_ws();
        self.chars.peek().map(|&(_, c)| c)
    }

    fn ident(&mut self) -> String {
        self.skip_ws();
        let mut out = String::new();
        while let Some(&(_, c)) = self.chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                out.push(c);
                self.chars.next();
            } else {
                break;
            }
        }
        out
    }

    fn quoted_string(&mut self) -> Result<String> {
        self.eat('\'')?;
        let mut out = String::new();
        loop {
            match self.chars.next() {
                Some((_, '\\')) => match self.chars.next() {
                    Some((_, c)) => out.push(unescape(c)),
                    None => return Err(self.err("eof in escape".into())),
                },
                Some((_, '\'')) => return Ok(out),
                Some((_, c)) => out.push(c),
                None => return Err(self.err("unterminated string".into())),
            }
        }
    }

    fn number<T: std::str::FromStr>(&mut self) -> Result<T> {
        self.skip_ws();
        let mut out = String::new();
        if matches!(self.chars.peek(), Some((_, '-'))) {
            out.push('-');
            self.chars.next();
        }
        while matches!(self.chars.peek(), Some((_, c)) if c.is_ascii_digit()) {
            out.push(self.chars.next().unwrap().1);
        }
        out.parse()
            .map_err(|_| self.err(format!("invalid number {out:?}")))
    }

    fn enum_entries(&mut self) -> Result<Vec<(String, i16)>> {
        self.eat('(')?;
        let mut entries = Vec::new();
        loop {
            let name = self.quoted_string()?;
            self.eat('=')?;
            let value: i16 = self.number()?;
            entries.push((name, value));
            match self.peek_char() {
                Some(',') => {
                    self.chars.next();
                }
                Some(')') => {
                    self.chars.next();
                    return Ok(entries);
                }
                other => return Err(self.err(format!("expected , or ) in enum, found {other:?}"))),
            }
        }
    }

    /// Optional `('tz')` argument for DateTime, or the `, 'tz'` tail of
    /// DateTime64.
    fn tz_arg(&mut self) -> Result<Option<String>> {
        Ok(Some(self.quoted_string()?))
    }

    fn parse(&mut self) -> Result<ChType> {
        if self.depth >= MAX_TYPE_DEPTH {
            return Err(self.err(format!("type nesting exceeds limit of {MAX_TYPE_DEPTH}")));
        }
        self.depth += 1;
        let result = self.parse_inner();
        self.depth -= 1;
        result
    }

    fn parse_inner(&mut self) -> Result<ChType> {
        let name = self.ident();
        match name.as_str() {
            "UInt8" => Ok(ChType::UInt8),
            "UInt16" => Ok(ChType::UInt16),
            "UInt32" => Ok(ChType::UInt32),
            "UInt64" => Ok(ChType::UInt64),
            "UInt128" => Ok(ChType::UInt128),
            "Int8" => Ok(ChType::Int8),
            "Int16" => Ok(ChType::Int16),
            "Int32" => Ok(ChType::Int32),
            "Int64" => Ok(ChType::Int64),
            "Int128" => Ok(ChType::Int128),
            "Float32" => Ok(ChType::Float32),
            "Float64" => Ok(ChType::Float64),
            "Bool" => Ok(ChType::Bool),
            "String" => Ok(ChType::String),
            "UUID" => Ok(ChType::Uuid),
            "Date" => Ok(ChType::Date),
            "Date32" => Ok(ChType::Date32),
            "IPv4" => Ok(ChType::Ipv4),
            "IPv6" => Ok(ChType::Ipv6),
            "FixedString" => {
                self.eat('(')?;
                let n: usize = self.number()?;
                self.eat(')')?;
                if n > MAX_FIXED_STRING_BYTES {
                    return Err(self.err(format!(
                        "FixedString length exceeds limit of {MAX_FIXED_STRING_BYTES}"
                    )));
                }
                Ok(ChType::FixedString(n))
            }
            "DateTime" => {
                if self.peek_char() == Some('(') {
                    self.chars.next();
                    let tz = self.tz_arg()?;
                    self.eat(')')?;
                    Ok(ChType::DateTime { tz })
                } else {
                    Ok(ChType::DateTime { tz: None })
                }
            }
            "DateTime64" => {
                self.eat('(')?;
                let precision: u8 = self.number()?;
                let tz = if self.peek_char() == Some(',') {
                    self.chars.next();
                    self.tz_arg()?
                } else {
                    None
                };
                self.eat(')')?;
                if precision > 9 {
                    return Err(self.err("DateTime64 precision must be at most 9".into()));
                }
                Ok(ChType::DateTime64 { precision, tz })
            }
            "Decimal" => {
                self.eat('(')?;
                let precision: u8 = self.number()?;
                self.eat(',')?;
                let scale: u8 = self.number()?;
                self.eat(')')?;
                if precision == 0 || precision > 38 || scale > precision {
                    return Err(self.err("invalid Decimal precision or scale".into()));
                }
                Ok(ChType::Decimal { precision, scale })
            }
            "Decimal32" | "Decimal64" | "Decimal128" => {
                self.eat('(')?;
                let scale: u8 = self.number()?;
                self.eat(')')?;
                let precision = match name.as_str() {
                    "Decimal32" => 9,
                    "Decimal64" => 18,
                    _ => 38,
                };
                if scale > precision {
                    return Err(self.err("Decimal scale exceeds precision".into()));
                }
                Ok(ChType::Decimal { precision, scale })
            }
            "Enum8" => Ok(ChType::Enum8(self.enum_entries()?)),
            "Enum16" => Ok(ChType::Enum16(self.enum_entries()?)),
            "Nullable" => {
                self.eat('(')?;
                let inner = self.parse()?;
                self.eat(')')?;
                Ok(ChType::Nullable(Box::new(inner)))
            }
            "LowCardinality" => {
                self.eat('(')?;
                let inner = self.parse()?;
                self.eat(')')?;
                Ok(ChType::LowCardinality(Box::new(inner)))
            }
            "Array" => {
                self.eat('(')?;
                let inner = self.parse()?;
                self.eat(')')?;
                Ok(ChType::Array(Box::new(inner)))
            }
            "Map" => {
                self.eat('(')?;
                let key = self.parse()?;
                self.eat(',')?;
                let value = self.parse()?;
                self.eat(')')?;
                Ok(ChType::Map(Box::new(key), Box::new(value)))
            }
            "Tuple" => {
                self.eat('(')?;
                let mut items = Vec::new();
                loop {
                    if items.len() >= MAX_TUPLE_ITEMS {
                        return Err(self.err(format!(
                            "tuple item count exceeds limit of {MAX_TUPLE_ITEMS}"
                        )));
                    }
                    items.push(self.tuple_element()?);
                    match self.peek_char() {
                        Some(',') => {
                            self.chars.next();
                        }
                        Some(')') => {
                            self.chars.next();
                            return Ok(ChType::Tuple(items));
                        }
                        other => {
                            return Err(
                                self.err(format!("expected , or ) in tuple, found {other:?}"))
                            );
                        }
                    }
                }
            }
            "JSON" => {
                if self.peek_char() == Some('(') {
                    self.skip_parens()?;
                }
                Ok(ChType::Json)
            }
            other => Err(ChError::UnsupportedType(if other.is_empty() {
                self.input.to_string()
            } else {
                other.to_string()
            })),
        }
    }

    /// Consume a balanced parenthesized argument list without
    /// interpreting it, respecting quoted strings.
    fn skip_parens(&mut self) -> Result<()> {
        self.eat('(')?;
        let mut depth = 1usize;
        while let Some((_, c)) = self.chars.next() {
            match c {
                '(' => {
                    if depth >= MAX_TYPE_DEPTH {
                        return Err(self.err(format!(
                            "JSON argument nesting exceeds limit of {MAX_TYPE_DEPTH}"
                        )));
                    }
                    depth += 1;
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                '\'' => loop {
                    match self.chars.next() {
                        Some((_, '\\')) => {
                            self.chars.next();
                        }
                        Some((_, '\'')) => break,
                        Some(_) => {}
                        None => return Err(self.err("unterminated string".into())),
                    }
                },
                _ => {}
            }
        }
        Err(self.err("unterminated parens".into()))
    }

    /// A tuple element is either `Type` or `name Type` (named tuples). A
    /// leading identifier followed by another identifier is a name.
    fn tuple_element(&mut self) -> Result<ChType> {
        self.skip_ws();
        if self.peek_char() == Some('`') {
            // Backquoted element name.
            self.chars.next();
            for (_, c) in self.chars.by_ref() {
                if c == '`' {
                    break;
                }
            }
            return self.parse();
        }
        // Both forms start with an identifier, so the only way to tell
        // them apart is to read it and look at what follows; checkpoint
        // first so it can be re-read as a type.
        let checkpoint = self.chars.clone();
        let first = self.ident();
        self.skip_ws();
        let next_is_ident = matches!(self.chars.peek(), Some((_, c)) if c.is_ascii_alphabetic());
        if next_is_ident {
            // `first` was an element name; the real type follows.
            self.parse()
        } else {
            // `first` was the type itself; rewind and parse normally.
            self.chars = checkpoint;
            let _ = first;
            self.parse()
        }
    }
}

fn unescape(c: char) -> char {
    match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '0' => '\0',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars() {
        assert_eq!(parse_type("UInt64").unwrap(), ChType::UInt64);
        assert_eq!(parse_type("Bool").unwrap(), ChType::Bool);
        assert_eq!(
            parse_type("FixedString(16)").unwrap(),
            ChType::FixedString(16)
        );
    }

    #[test]
    fn datetimes() {
        assert_eq!(
            parse_type("DateTime").unwrap(),
            ChType::DateTime { tz: None }
        );
        assert_eq!(
            parse_type("DateTime('Europe/Amsterdam')").unwrap(),
            ChType::DateTime {
                tz: Some("Europe/Amsterdam".into())
            }
        );
        assert_eq!(
            parse_type("DateTime64(3, 'UTC')").unwrap(),
            ChType::DateTime64 {
                precision: 3,
                tz: Some("UTC".into())
            }
        );
    }

    #[test]
    fn nested() {
        assert_eq!(
            parse_type("Array(Nullable(String))").unwrap(),
            ChType::Array(Box::new(ChType::Nullable(Box::new(ChType::String))))
        );
        assert_eq!(
            parse_type("Map(String, Array(UInt8))").unwrap(),
            ChType::Map(
                Box::new(ChType::String),
                Box::new(ChType::Array(Box::new(ChType::UInt8)))
            )
        );
    }

    #[test]
    fn enums() {
        assert_eq!(
            parse_type("Enum8('a' = 1, 'b\\'c' = -2)").unwrap(),
            ChType::Enum8(vec![("a".into(), 1), ("b'c".into(), -2)])
        );
    }

    #[test]
    fn tuples_plain_and_named() {
        assert_eq!(
            parse_type("Tuple(String, UInt8)").unwrap(),
            ChType::Tuple(vec![ChType::String, ChType::UInt8])
        );
        assert_eq!(
            parse_type("Tuple(name String, age UInt8)").unwrap(),
            ChType::Tuple(vec![ChType::String, ChType::UInt8])
        );
    }

    #[test]
    fn json_with_and_without_parameters() {
        assert_eq!(parse_type("JSON").unwrap(), ChType::Json);
        assert_eq!(
            parse_type("JSON(max_dynamic_paths = 16, a.b UInt32, SKIP `x.y`)").unwrap(),
            ChType::Json
        );
        assert_eq!(
            parse_type("Array(JSON)").unwrap(),
            ChType::Array(Box::new(ChType::Json))
        );
    }

    #[test]
    fn unsupported_is_loud() {
        assert!(matches!(
            parse_type("AggregateFunction(sum, UInt64)"),
            Err(ChError::UnsupportedType(_))
        ));
    }

    #[test]
    fn rejects_unsafe_numeric_parameters() {
        assert!(parse_type("DateTime64(10)").is_err());
        assert!(parse_type("Decimal(0, 0)").is_err());
        assert!(parse_type("Decimal(9, 10)").is_err());
        assert!(parse_type("Decimal32(10)").is_err());
        assert!(parse_type("FixedString(67108865)").is_err());
    }

    #[test]
    fn rejects_excessive_type_size_and_nesting() {
        let oversized = "X".repeat(MAX_TYPE_STRING_BYTES + 1);
        assert!(parse_type(&oversized).is_err());

        let nested = format!(
            "{}UInt8{}",
            "Array(".repeat(MAX_TYPE_DEPTH),
            ")".repeat(MAX_TYPE_DEPTH)
        );
        assert!(parse_type(&nested).is_err());

        let json = format!(
            "JSON({}x{})",
            "(".repeat(MAX_TYPE_DEPTH),
            ")".repeat(MAX_TYPE_DEPTH)
        );
        assert!(parse_type(&json).is_err());
    }
}
