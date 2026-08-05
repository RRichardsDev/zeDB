-- migration 00000: baseline schema.
CREATE DATABASE IF NOT EXISTS ${db};

CREATE TABLE ${db}.events
(
    id         UInt64,
    kind       LowCardinality(String),
    occurred_at DateTime64(3)
)
ENGINE = MergeTree
ORDER BY (kind, occurred_at)
TTL toDateTime(occurred_at) + INTERVAL ${ttl_days} DAY;
