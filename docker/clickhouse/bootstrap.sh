#!/usr/bin/env bash
set -euo pipefail

client=(clickhouse-client --host clickhouse-1 --user zedb --password zedb)

for attempt in {1..30}; do
    if "${client[@]}" --multiquery < /bootstrap/testing.sql; then
        break
    fi
    if [[ "$attempt" == "30" ]]; then
        echo "Could not bootstrap the ClickHouse cluster" >&2
        exit 1
    fi
    sleep 2
done

clickhouse-client --host clickhouse-2 --user zedb --password zedb \
    --query "SYSTEM SYNC REPLICA default.testing"

node_1_count=$("${client[@]}" --query "SELECT count() FROM default.testing")
node_2_count=$(clickhouse-client --host clickhouse-2 --user zedb --password zedb \
    --query "SELECT count() FROM default.testing")

if [[ "$node_1_count" != "1000000" || "$node_2_count" != "1000000" ]]; then
    echo "Expected 1000000 rows on each replica, got $node_1_count and $node_2_count" >&2
    exit 1
fi

echo "Cluster ready: default.testing has 1000000 rows on both replicas"

# Sharded demo (Phase 5 dev target): shard_demo.events is a Distributed
# table over zedb_sharded (2 shards x 1 replica); events_local holds
# each shard's slice. Idempotent: seeds only when empty.
"${client[@]}" --query "CREATE DATABASE IF NOT EXISTS shard_demo ON CLUSTER zedb_sharded" >/dev/null
"${client[@]}" --query "
    CREATE TABLE IF NOT EXISTS shard_demo.events_local ON CLUSTER zedb_sharded (
        id UInt64,
        user_id UInt64,
        kind LowCardinality(String),
        at DateTime
    ) ENGINE = MergeTree ORDER BY (user_id, id)" >/dev/null
"${client[@]}" --query "
    CREATE TABLE IF NOT EXISTS shard_demo.events ON CLUSTER zedb_sharded
    AS shard_demo.events_local
    ENGINE = Distributed(zedb_sharded, shard_demo, events_local, user_id)" >/dev/null

sharded_count=$("${client[@]}" --query "SELECT count() FROM shard_demo.events")
if [[ "$sharded_count" == "0" ]]; then
    "${client[@]}" --query "
        INSERT INTO shard_demo.events
        SELECT
            number,
            rand() % 1000,
            ['click', 'view', 'purchase'][number % 3 + 1],
            now() - number
        FROM numbers(200000)"
    "${client[@]}" --query "SYSTEM FLUSH DISTRIBUTED shard_demo.events"
    sharded_count=$("${client[@]}" --query "SELECT count() FROM shard_demo.events")
fi

shard_1_local=$("${client[@]}" --query "SELECT count() FROM shard_demo.events_local")
shard_2_local=$(clickhouse-client --host clickhouse-2 --user zedb --password zedb \
    --query "SELECT count() FROM shard_demo.events_local")
echo "Sharded demo ready: shard_demo.events has $sharded_count rows" \
    "($shard_1_local on shard 1, $shard_2_local on shard 2)"
