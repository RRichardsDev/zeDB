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
