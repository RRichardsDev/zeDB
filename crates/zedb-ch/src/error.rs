use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// The server returned an exception (non-2xx response).
    #[error("clickhouse error{}: {message}", code.map(|c| format!(" (code {c})")).unwrap_or_default())]
    Server { code: Option<i32>, message: String },

    #[error("unsupported ClickHouse type: {0}")]
    UnsupportedType(String),

    #[error("failed to parse type string {input:?}: {reason}")]
    TypeParse { input: String, reason: String },

    #[error("failed to decode RowBinary response: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, ChError>;
