CREATE TABLE IF NOT EXISTS ${db}.events ON CLUSTER ${cluster}
(
    `id` UInt64,
    `kind` LowCardinality(String),
    `occurred_at` DateTime64(3),
    `status` String DEFAULT 'pending',
    `statusV2` String DEFAULT 'pending'
)
ENGINE = ReplicatedMergeTree('/clickhouse/tables/{uuid}/{shard}', '{replica}')
ORDER BY (kind, occurred_at)
TTL toDateTime(occurred_at) + toIntervalDay(${ttl_days})
SETTINGS index_granularity = 8192;
