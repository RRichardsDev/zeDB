use super::*;

pub(super) fn tls_connector() -> Result<tokio_rustls::TlsConnector> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| ChError::NativeTransport(format!("TLS setup failed: {error}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// Ask the server over HTTP which native ports it actually listens on:
/// `(tcp_port_secure, tcp_port)`. Either is `None` when unset or when the
/// server predates `getServerPort`; callers fall back to 9440/9000.
pub(super) async fn discovered_native_ports(http: &crate::ChClient) -> (Option<u16>, Option<u16>) {
    // Two statements: getServerPort throws for a port that isn't
    // configured, and a secure-only or plain-only server is normal.
    let secure = first_port(
        http.query_http("SELECT getServerPort('tcp_port_secure')")
            .await,
    );
    let plain = first_port(http.query_http("SELECT getServerPort('tcp_port')").await);
    (secure, plain)
}

/// How far the port we reach the server's HTTP interface on sits from the
/// port the server believes it serves. Port remapping (docker publishes,
/// kubectl port-forwards) usually shifts every published port by the same
/// amount, so the native connect tries the shifted ports before the
/// advertised ones; the server-identity check validates whichever answers.
pub(super) async fn discovered_http_offset(http: &crate::ChClient, url: &str) -> i32 {
    let Some(reached) = port_of(url) else {
        return 0;
    };
    let setting = if url.trim_start().starts_with("https") {
        "SELECT getServerPort('https_port')"
    } else {
        "SELECT getServerPort('http_port')"
    };
    match first_port(http.query_http(setting).await) {
        Some(advertised) => i32::from(reached) - i32::from(advertised),
        None => 0,
    }
}

fn first_port(result: Result<QueryResult>) -> Option<u16> {
    result
        .ok()
        .and_then(|result| result.rows.into_iter().next())
        .and_then(|row| row.into_iter().next())
        .and_then(|value| match value {
            Value::UInt(port) => u16::try_from(port).ok(),
            Value::Int(port) => u16::try_from(port).ok(),
            _ => None,
        })
}

/// The host of a ClickHouse HTTP URL (`http(s)://host:port/...` -> `host`).
pub fn host_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed.host_str().map(|host| {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    })
}

/// The explicit port of a ClickHouse HTTP URL, if one is written.
pub fn port_of(url: &str) -> Option<u16> {
    reqwest::Url::parse(url).ok()?.port()
}

/// A `SET` right-hand side: numeric values go bare, anything else quoted.
pub(super) fn setting_literal(value: &str) -> String {
    if value.parse::<f64>().is_ok() {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

pub(super) fn map_err(error: klickhouse::KlickhouseError) -> ChError {
    match error {
        klickhouse::KlickhouseError::ServerException { code, message, .. } => ChError::Server {
            code: Some(code),
            message,
        },
        other => ChError::NativeTransport(other.to_string()),
    }
}

pub(super) fn block_columns(block: &klickhouse::block::Block) -> Vec<ColumnMeta> {
    block
        .column_types
        .iter()
        .map(|(name, type_)| ColumnMeta {
            name: name.clone(),
            type_name: type_.to_string(),
        })
        .collect()
}

/// Column-major block data to row-major [`Value`] rows, appended.
pub(super) fn append_block_rows(block: klickhouse::block::Block, rows: &mut Vec<Vec<Value>>) {
    let row_count = block.rows as usize;
    if row_count == 0 {
        return;
    }
    let types: Vec<klickhouse::Type> = block.column_types.values().cloned().collect();
    let mut columns: Vec<std::vec::IntoIter<klickhouse::Value>> = block
        .column_data
        .into_iter()
        .map(|(_, data)| data.into_iter())
        .collect();
    for _ in 0..row_count {
        let mut row = Vec::with_capacity(columns.len());
        for (index, column) in columns.iter_mut().enumerate() {
            let value = column.next().unwrap_or(klickhouse::Value::Null);
            row.push(map_value(types.get(index), value));
        }
        rows.push(row);
    }
}

/// Peel Nullable/LowCardinality wrappers to the value-shaped inner type.
fn strip_wrappers(type_: &klickhouse::Type) -> &klickhouse::Type {
    match type_ {
        klickhouse::Type::Nullable(inner) | klickhouse::Type::LowCardinality(inner) => {
            strip_wrappers(inner)
        }
        other => other,
    }
}

/// One klickhouse value into the driver-agnostic model, mirroring the
/// RowBinary decoder's normalizations (ints widen to Int/UInt, enums
/// resolve to their names, non-UTF-8 strings become bytes).
pub(super) fn map_value(type_: Option<&klickhouse::Type>, value: klickhouse::Value) -> Value {
    use klickhouse::Value as KV;
    let type_ = type_.map(strip_wrappers);
    match value {
        KV::Null => Value::Null,
        KV::Int8(v) => Value::Int(v.into()),
        KV::Int16(v) => Value::Int(v.into()),
        KV::Int32(v) => Value::Int(v.into()),
        KV::Int64(v) => Value::Int(v),
        KV::Int128(v) => Value::Int128(v),
        KV::Int256(v) => Value::String(v.to_string()),
        KV::UInt8(v) => Value::UInt(v.into()),
        KV::UInt16(v) => Value::UInt(v.into()),
        KV::UInt32(v) => Value::UInt(v.into()),
        KV::UInt64(v) => Value::UInt(v),
        KV::UInt128(v) => Value::UInt128(v),
        KV::UInt256(v) => Value::String(v.to_string()),
        KV::Float32(v) => Value::Float(v.into()),
        KV::Float64(v) => Value::Float(v),
        KV::BFloat16(v) => Value::Float(v.to_f32().into()),
        KV::Decimal32(scale, v) => Value::Decimal {
            value: v.into(),
            scale: scale as u8,
        },
        KV::Decimal64(scale, v) => Value::Decimal {
            value: v.into(),
            scale: scale as u8,
        },
        KV::Decimal128(scale, v) => Value::Decimal {
            value: v,
            scale: scale as u8,
        },
        KV::Decimal256(_, v) => Value::String(v.to_string()),
        KV::String(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Value::String(text),
            Err(error) => Value::Bytes(error.into_bytes()),
        },
        KV::Uuid(uuid) => Value::Uuid(*uuid.as_bytes()),
        KV::Date(date) => Value::Date(date.into()),
        KV::DateTime(dt) => match chrono::DateTime::<chrono::Utc>::try_from(dt) {
            Ok(utc) => Value::DateTime(utc),
            Err(_) => Value::Null,
        },
        KV::DateTime64(dt) => match chrono::DateTime::<chrono::Utc>::try_from(dt) {
            Ok(utc) => Value::DateTime(utc),
            Err(_) => Value::Null,
        },
        KV::Enum8(v) => enum_name(type_, v.into()).unwrap_or(Value::Int(v.into())),
        KV::Enum16(v) => enum_name(type_, v).unwrap_or(Value::Int(v.into())),
        KV::Array(items) => {
            let inner = type_.and_then(|t| t.unarray());
            Value::Array(
                items
                    .into_iter()
                    .map(|item| map_value(inner, item))
                    .collect(),
            )
        }
        KV::Tuple(items) => {
            let inners: Option<&Vec<klickhouse::Type>> = match type_ {
                Some(klickhouse::Type::Tuple(inners)) => Some(inners),
                _ => None,
            };
            Value::Tuple(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        map_value(inners.and_then(|inners| inners.get(index)), item)
                    })
                    .collect(),
            )
        }
        KV::Map(keys, values) => {
            let kv = type_.and_then(|t| t.unmap());
            Value::Map(
                keys.into_iter()
                    .zip(values)
                    .map(|(key, value)| {
                        (
                            map_value(kv.map(|(k, _)| k), key),
                            map_value(kv.map(|(_, v)| v), value),
                        )
                    })
                    .collect(),
            )
        }
        KV::Ipv4(ip) => Value::Ipv4(ip.into()),
        KV::Ipv6(ip) => Value::Ipv6(ip.into()),
        geo @ (KV::Point(_) | KV::Ring(_) | KV::Polygon(_) | KV::MultiPolygon(_)) => {
            Value::String(format!("{geo:?}"))
        }
    }
}

/// Resolve an enum discriminant to its symbolic name via the column type.
fn enum_name(type_: Option<&klickhouse::Type>, v: i16) -> Option<Value> {
    match type_ {
        Some(klickhouse::Type::Enum8(entries)) => entries
            .iter()
            .find(|(_, ev)| i16::from(*ev) == v)
            .map(|(name, _)| Value::Enum(name.clone())),
        Some(klickhouse::Type::Enum16(entries)) => entries
            .iter()
            .find(|(_, ev)| *ev == v)
            .map(|(name, _)| Value::Enum(name.clone())),
        _ => None,
    }
}
