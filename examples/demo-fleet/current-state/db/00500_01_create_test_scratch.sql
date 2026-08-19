-- migration 00500: add a scratch test table.
CREATE TABLE IF NOT EXISTS ${db}.test_scratch ON CLUSTER ${cluster}
(
    id         UInt64,
    label      LowCardinality(String),
    value      Float64,
    created_at DateTime64(3) DEFAULT now64(3)
)
ENGINE = ReplicatedMergeTree('/clickhouse/tables/{uuid}/{shard}', '{replica}')
ORDER BY (label, created_at);
