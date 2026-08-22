use std::time::Duration;

use gpui::Context;

use super::model::*;
use crate::{rt, Workspace};

impl Workspace {
    pub(crate) fn ops_toggle(&mut self, cx: &mut Context<Self>) {
        if self.connection.connected.is_none() {
            self.flash_warning("Connect to a cluster to see its ops view", cx);
            return;
        }
        self.show_ops = !self.show_ops;
        if self.show_ops {
            self.show_query_editor = false;
            self.show_fleet = false;
            self.ops_start_poll(cx);
        }
        cx.notify();
    }

    /// The connection target changed (new cluster, new node, or gone):
    /// drop everything shown, and if the view is open against a live
    /// connection, fetch the new target immediately and restart the
    /// cadence from zero.
    pub(crate) fn ops_reset(&mut self, cx: &mut Context<Self>) {
        self.ops_clear_data();
        // The new target may not know the old scope's cluster.
        self.ops.scope = OpsScope::Node;
        if self.connection.connected.is_none() {
            self.show_ops = false;
        } else if self.show_ops {
            self.ops_start_poll(cx);
        }
        cx.notify();
    }

    pub(crate) fn ops_clear_data(&mut self) {
        self.ops.poll_generation += 1;
        self.ops.processes.clear();
        self.ops.connections.clear();
        self.ops.merges.clear();
        self.ops.mutations.clear();
        self.ops.replica_total = 0;
        self.ops.replica_problems.clear();
        self.ops.queue_issues.clear();
        self.ops.keeper.clear();
        self.ops.kafka_consumers.clear();
        self.ops.view_failures.clear();
        self.ops.async_inserts.clear();
        self.ops.disks.clear();
        self.ops.top_tables.clear();
        self.ops.as_of = None;
        self.ops.error = None;
        self.ops.killing = None;
    }

    /// Clusters the connected node reported membership of; the scope
    /// dropdown's options. Single-host clusters are skipped by shape
    /// (the implicit self-only cluster every node carries is not a
    /// topology); the name is never consulted, because ClickHouse
    /// Cloud's real cluster is literally named "default".
    pub(crate) fn ops_cluster_options(&self) -> Vec<String> {
        let Some(connected) = &self.connection.connected else {
            return Vec::new();
        };
        let Some(health) = self.connection.endpoint_health.get(&connected.name) else {
            return Vec::new();
        };
        let mut clusters: Vec<String> = health
            .iter()
            .filter(|node| node.node_index == connected.active_node)
            .flat_map(|node| topology_clusters(&node.memberships))
            .collect();
        clusters.dedup();
        clusters
    }

    pub fn ops_set_scope(&mut self, cluster: Option<String>, cx: &mut Context<Self>) {
        let scope = match cluster {
            Some(name) => OpsScope::Cluster(name),
            None => OpsScope::Node,
        };
        if self.ops.scope == scope {
            return;
        }
        self.ops_clear_data();
        self.ops.scope = scope;
        if self.show_ops {
            self.ops_start_poll(cx);
        }
        cx.notify();
    }

    /// The current fast-lane cadence: watching eyes get fresh data,
    /// a backgrounded window does not burn queries nobody sees.
    pub(crate) fn ops_poll_secs(&self) -> u64 {
        if self.window_active {
            POLL_ACTIVE_SECS
        } else {
            POLL_INACTIVE_SECS
        }
    }

    /// The connection config for poll queries only: identical, plus
    /// log_queries=0 so a fast cadence does not flood query_log (and
    /// the ops view does not mostly show itself). Kills and every
    /// user-initiated query still log normally.
    fn ops_poll_config(config: &zedb_ch::ChConfig) -> zedb_ch::ChConfig {
        let mut config = config.clone();
        config.driver.settings.push(zedb_core::DriverSetting {
            name: "log_queries".into(),
            value: "0".into(),
        });
        config
    }

    /// Fetch immediately, then on the adaptive cadence while the view
    /// stays visible. Generation-guarded like the health poll; hiding
    /// the view or reconnecting ends the loop.
    pub(crate) fn ops_start_poll(&mut self, cx: &mut Context<Self>) {
        self.ops.poll_generation += 1;
        let generation = self.ops.poll_generation;
        self.ops.tick = 0;
        self.ops_fetch(cx);
        self.ops_fetch_slow(cx);
        cx.spawn(async move |this, cx| loop {
            let Ok(delay) = this.update(cx, |this, _| this.ops_poll_secs()) else {
                break;
            };
            // The executor's timer, so window tests drive the cadence
            // with the simulated clock.
            cx.background_executor()
                .timer(Duration::from_secs(delay))
                .await;
            let live = this
                .update(cx, |this, cx| {
                    let live = this.ops.poll_generation == generation
                        && this.show_ops
                        && this.connection.connected.is_some();
                    if live {
                        this.ops.tick += delay;
                        this.ops_fetch(cx);
                        if this.ops.tick >= SLOW_POLL_SECS {
                            this.ops.tick = 0;
                            this.ops_fetch_slow(cx);
                        }
                    }
                    live
                })
                .unwrap_or(false);
            if !live {
                break;
            }
        })
        .detach();
    }

    pub(crate) fn ops_fetch(&mut self, cx: &mut Context<Self>) {
        if self.ops.fetch_in_flight {
            return;
        }
        let Some(connected) = &self.connection.connected else {
            return;
        };
        self.ops.fetch_in_flight = true;
        let config = Self::ops_poll_config(&connected.client_config);
        // Cluster scope fans out to every replica; hostName() names
        // the node each row came from.
        let cluster = self.ops.scope.cluster().map(quoted);
        let from = |table: &str| match &cluster {
            Some(name) => format!("clusterAllReplicas({name}, {table})"),
            None => table.to_string(),
        };
        let host_col = if cluster.is_some() {
            ", hostName()"
        } else {
            ""
        };
        let processes_query = format!(
            "SELECT query_id, user, elapsed, read_rows, read_bytes, \
                total_rows_approx, memory_usage, query, \
                toString(address), client_name, http_user_agent, \
                os_user, initial_user{host_col} \
             FROM {} \
             WHERE query NOT LIKE '%system.processes%' \
             ORDER BY elapsed DESC \
             LIMIT 50",
            from("system.processes")
        );
        // Aggregate open-connection counters; ClickHouse has no
        // session table (HTTP is stateless), so counts are the
        // whole truth here. Cluster scope sums across nodes.
        let connections_query = format!(
            "SELECT metric, sum(value) FROM {} \
             WHERE metric IN ('HTTPConnection', 'TCPConnection', \
                'MySQLConnection', 'PostgreSQLConnection', \
                'InterserverConnection') \
             GROUP BY metric ORDER BY metric",
            from("system.metrics")
        );
        let merges_query = format!(
            "SELECT database, table, elapsed, progress, num_parts, \
                total_size_bytes_compressed, is_mutation{host_col} \
             FROM {} \
             ORDER BY elapsed DESC LIMIT 20",
            from("system.merges")
        );
        // Unfinished mutations only; failing ones first.
        let mutations_query = format!(
            "SELECT database, table, command, parts_to_do, \
                latest_fail_reason{host_col} \
             FROM {} \
             WHERE NOT is_done \
             ORDER BY latest_fail_reason != '' DESC, create_time ASC \
             LIMIT 20",
            from("system.mutations")
        );
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let processes = client.query(&processes_query).await;
            let connections = client.query(&connections_query).await;
            let merges = client.query(&merges_query).await;
            let mutations = client.query(&mutations_query).await;
            (processes, connections, merges, mutations)
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.fetch_in_flight = false;
                match result {
                    Ok((Ok(result), connections, merges, mutations)) => {
                        this.ops.processes = result
                            .rows
                            .iter()
                            .map(|row| {
                                let client_name = text(row.get(9));
                                let user_agent = text(row.get(10));
                                let address = text(row.get(8));
                                OpsProcess {
                                    query_id: text(row.first()),
                                    user: text(row.get(1)),
                                    elapsed_secs: float(row.get(2)),
                                    read_rows: number(row.get(3)),
                                    read_bytes: number(row.get(4)),
                                    total_rows: number(row.get(5)),
                                    memory_bytes: number(row.get(6)),
                                    query: text(row.get(7)),
                                    address: display_address(&address),
                                    client: if client_name.is_empty() {
                                        user_agent
                                    } else {
                                        client_name
                                    },
                                    os_user: text(row.get(11)),
                                    initial_user: text(row.get(12)),
                                    node: text(row.get(13)),
                                }
                            })
                            .collect();
                        if let Ok(connections) = connections {
                            this.ops.connections = connections
                                .rows
                                .iter()
                                .map(|row| {
                                    let label = match text(row.first()).as_str() {
                                        "HTTPConnection" => "http".to_string(),
                                        "TCPConnection" => "tcp".to_string(),
                                        "MySQLConnection" => "mysql".to_string(),
                                        "PostgreSQLConnection" => "postgres".to_string(),
                                        "InterserverConnection" => "interserver".to_string(),
                                        other => other.to_string(),
                                    };
                                    (label, number(row.get(1)))
                                })
                                .filter(|(_, count)| *count > 0)
                                .collect();
                        }
                        if let Ok(merges) = merges {
                            this.ops.merges = merges
                                .rows
                                .iter()
                                .map(|row| OpsMerge {
                                    database: text(row.first()),
                                    table: text(row.get(1)),
                                    elapsed_secs: float(row.get(2)),
                                    progress: float(row.get(3)).clamp(0.0, 1.0),
                                    num_parts: number(row.get(4)),
                                    total_size_bytes: number(row.get(5)),
                                    is_mutation: number(row.get(6)) > 0,
                                    node: text(row.get(7)),
                                })
                                .collect();
                        }
                        if let Ok(mutations) = mutations {
                            this.ops.mutations = mutations
                                .rows
                                .iter()
                                .map(|row| OpsMutation {
                                    database: text(row.first()),
                                    table: text(row.get(1)),
                                    command: text(row.get(2)),
                                    parts_to_do: number(row.get(3)),
                                    latest_fail_reason: text(row.get(4)),
                                    node: text(row.get(5)),
                                })
                                .collect();
                        }
                        this.ops.as_of = Some(chrono::Local::now());
                        this.ops.error = None;
                    }
                    Ok((Err(error), _, _, _)) => this.ops.error = Some(error.to_string()),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Replication health, disks, and largest tables: slow-moving,
    /// fetched every fifth tick.
    pub(crate) fn ops_fetch_slow(&mut self, cx: &mut Context<Self>) {
        if self.ops.slow_fetch_in_flight {
            return;
        }
        let Some(connected) = &self.connection.connected else {
            return;
        };
        self.ops.slow_fetch_in_flight = true;
        let config = Self::ops_poll_config(&connected.client_config);
        let top_clause = self.ops.top_limit.clause();
        let cluster = self.ops.scope.cluster().map(quoted);
        let from = |table: &str| match &cluster {
            Some(name) => format!("clusterAllReplicas({name}, {table})"),
            None => table.to_string(),
        };
        let host_col = if cluster.is_some() {
            ", hostName()"
        } else {
            ""
        };
        let replica_total_query = format!("SELECT count() FROM {}", from("system.replicas"));
        let replica_problems_query = format!(
            "SELECT database, table, is_readonly, is_session_expired, \
                absolute_delay, queue_size{host_col} \
             FROM {} \
             WHERE is_readonly OR is_session_expired \
                OR absolute_delay > 0 OR queue_size > 0 \
             ORDER BY absolute_delay DESC LIMIT 20",
            from("system.replicas")
        );
        let queue_issues_query = format!(
            "SELECT database, table, count() AS depth, \
                max(dateDiff('second', create_time, now())), \
                anyIf(last_exception, last_exception != ''){host_col} \
             FROM {} \
             GROUP BY database, table{} \
             ORDER BY depth DESC LIMIT 10",
            from("system.replication_queue"),
            if cluster.is_some() {
                ", hostName()"
            } else {
                ""
            },
        );
        // Best-effort: the table needs a Keeper-backed server; absence
        // just leaves the section empty.
        let keeper_query = format!(
            "SELECT name, host, session_uptime_elapsed_seconds, is_expired{host_col} \
             FROM {}",
            from("system.zookeeper_connection")
        );
        // Ingestion, all best-effort: kafka_consumers needs Kafka
        // tables, query_views_log needs its log enabled, and
        // asynchronous_inserts is empty unless async inserts are on.
        let kafka_query = format!(
            "SELECT database, table, \
                dateDiff('second', last_poll_time, now()), \
                num_messages_read, \
                if(length(exceptions.text) > 0, exceptions.text[length(exceptions.text)], ''){host_col} \
             FROM {} ORDER BY database, table",
            from("system.kafka_consumers")
        );
        let view_failures_query = format!(
            "SELECT view_name, view_target, count() AS failures, \
                any(exception){host_col} \
             FROM {} \
             WHERE exception != '' AND event_date >= today() - 1 \
                AND event_time > now() - INTERVAL 24 HOUR \
             GROUP BY view_name, view_target{} \
             ORDER BY failures DESC LIMIT 20",
            from("system.query_views_log"),
            if cluster.is_some() {
                ", hostName()"
            } else {
                ""
            },
        );
        let async_inserts_query = format!(
            "SELECT database, table, sum(total_bytes), count(), \
                max(dateDiff('second', first_update, now())){host_col} \
             FROM {} \
             GROUP BY database, table{} \
             ORDER BY sum(total_bytes) DESC LIMIT 20",
            from("system.asynchronous_inserts"),
            if cluster.is_some() {
                ", hostName()"
            } else {
                ""
            },
        );
        let disks_query = format!(
            "SELECT name, free_space, total_space, type{host_col} FROM {} ORDER BY {}",
            from("system.disks"),
            if cluster.is_some() {
                "hostName(), name"
            } else {
                "name"
            },
        );
        // Largest tables in cluster scope read one replica per shard
        // (cluster(), not clusterAllReplicas) so replicas are not
        // double-counted; the sums are then cluster-wide totals.
        let top_tables_query = format!(
            "SELECT database, table, sum(bytes_on_disk), sum(rows) \
             FROM {} WHERE active \
             GROUP BY database, table \
             ORDER BY sum(bytes_on_disk) DESC{top_clause}",
            match &cluster {
                Some(name) => format!("cluster({name}, system.parts)"),
                None => "system.parts".to_string(),
            },
        );
        // Engine-family probe: any Shared* table means coordination is
        // Cloud-managed and the ZooKeeper-era signals do not apply.
        let smt_query = "SELECT countIf(engine LIKE 'Shared%') FROM system.tables \
             WHERE database NOT IN ('system', 'information_schema', 'INFORMATION_SCHEMA')"
            .to_string();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let smt = client.query(&smt_query).await;
            let replica_total = client.query(&replica_total_query).await;
            let replica_problems = client.query(&replica_problems_query).await;
            let queue_issues = client.query(&queue_issues_query).await;
            let keeper = client.query(&keeper_query).await;
            let kafka = client.query(&kafka_query).await;
            let view_failures = client.query(&view_failures_query).await;
            let async_inserts = client.query(&async_inserts_query).await;
            let disks = client.query(&disks_query).await;
            let top_tables = client.query(&top_tables_query).await;
            (
                smt,
                replica_total,
                replica_problems,
                queue_issues,
                keeper,
                kafka,
                view_failures,
                async_inserts,
                disks,
                top_tables,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.slow_fetch_in_flight = false;
                let Ok((
                    smt,
                    replica_total,
                    replica_problems,
                    queue_issues,
                    keeper,
                    kafka,
                    view_failures,
                    async_inserts,
                    disks,
                    top_tables,
                )) = result
                else {
                    return;
                };
                if let Ok(smt) = smt {
                    this.ops.smt = smt.rows.first().map(|row| number(row.first())).unwrap_or(0) > 0;
                }
                if let Ok(total) = replica_total {
                    this.ops.replica_total = total
                        .rows
                        .first()
                        .map(|row| number(row.first()))
                        .unwrap_or(0);
                }
                if let Ok(problems) = replica_problems {
                    this.ops.replica_problems = problems
                        .rows
                        .iter()
                        .map(|row| OpsReplicaProblem {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            is_readonly: number(row.get(2)) > 0,
                            session_expired: number(row.get(3)) > 0,
                            delay_secs: number(row.get(4)),
                            queue_size: number(row.get(5)),
                            node: text(row.get(6)),
                        })
                        .collect();
                }
                if let Ok(issues) = queue_issues {
                    this.ops.queue_issues = issues
                        .rows
                        .iter()
                        .map(|row| OpsQueueIssue {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            depth: number(row.get(2)),
                            oldest_secs: number(row.get(3)),
                            exception: text(row.get(4)),
                            node: text(row.get(5)),
                        })
                        .collect();
                }
                if let Ok(keeper) = keeper {
                    this.ops.keeper = keeper
                        .rows
                        .iter()
                        .map(|row| OpsKeeper {
                            name: text(row.first()),
                            host: text(row.get(1)),
                            uptime_secs: number(row.get(2)),
                            expired: number(row.get(3)) > 0,
                            node: text(row.get(4)),
                        })
                        .collect();
                }
                if let Ok(kafka) = kafka {
                    this.ops.kafka_consumers = kafka
                        .rows
                        .iter()
                        .map(|row| OpsKafkaConsumer {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            stale_secs: number(row.get(2)),
                            messages: number(row.get(3)),
                            exception: text(row.get(4)),
                            node: text(row.get(5)),
                        })
                        .collect();
                }
                if let Ok(failures) = view_failures {
                    this.ops.view_failures = failures
                        .rows
                        .iter()
                        .map(|row| OpsViewFailure {
                            view: text(row.first()),
                            target: text(row.get(1)),
                            failures: number(row.get(2)),
                            exception: text(row.get(3)),
                            node: text(row.get(4)),
                        })
                        .collect();
                }
                if let Ok(inserts) = async_inserts {
                    this.ops.async_inserts = inserts
                        .rows
                        .iter()
                        .map(|row| OpsAsyncInsert {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            bytes: number(row.get(2)),
                            entries: number(row.get(3)),
                            oldest_secs: number(row.get(4)),
                            node: text(row.get(5)),
                        })
                        .collect();
                }
                if let Ok(disks) = disks {
                    this.ops.disks = disks
                        .rows
                        .iter()
                        .map(|row| OpsDisk {
                            name: text(row.first()),
                            free: number(row.get(1)),
                            total: number(row.get(2)),
                            kind: text(row.get(3)),
                            node: text(row.get(4)),
                        })
                        .collect();
                }
                if let Ok(top) = top_tables {
                    this.ops.top_tables = top
                        .rows
                        .iter()
                        .map(|row| OpsTopTable {
                            database: text(row.first()),
                            table: text(row.get(1)),
                            bytes: number(row.get(2)),
                            rows: number(row.get(3)),
                        })
                        .collect();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn ops_set_top_limit(&mut self, limit: OpsTopLimit, cx: &mut Context<Self>) {
        if self.ops.top_limit == limit {
            return;
        }
        self.ops.top_limit = limit;
        self.ops_fetch_slow(cx);
        cx.notify();
    }

    pub(crate) fn ops_kill(&mut self, query_id: String, cx: &mut Context<Self>) {
        let Some(connected) = &self.connection.connected else {
            return;
        };
        if connected.client_config.read_only {
            if self.active_connection_is_cloud() {
                self.flash_warning(
                    "Cloud connections start read-only, which blocks KILL QUERY; edit the \
                     connection and turn off Read only for a writable session",
                    cx,
                );
                return;
            }
            self.flash_warning("This connection is read-only; KILL QUERY needs write", cx);
            return;
        }
        self.ops.killing = Some(query_id.clone());
        self.ops_killed.insert(query_id.clone());
        let config = connected.client_config.clone();
        // In cluster scope the query may run on another node; ON
        // CLUSTER reaches it via the distributed DDL queue.
        let on_cluster = self
            .ops
            .scope
            .cluster()
            .map(|name| format!(" ON CLUSTER {}", quoted(name)))
            .unwrap_or_default();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let escaped = query_id.replace('\'', "''");
            client
                .query(&format!(
                    "KILL QUERY{on_cluster} WHERE query_id = '{escaped}'"
                ))
                .await
                .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.killing = None;
                if let Ok(Err(error)) = result {
                    this.flash_warning(format!("KILL QUERY failed: {error}"), cx);
                }
                this.ops_fetch(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Real topologies among a node's cluster memberships: judged by
/// shape (more than one host), never by name, because ClickHouse
/// Cloud's actual cluster is named "default" while every self-hosted
/// node also carries a degenerate one-host cluster of that name.
pub(crate) fn topology_clusters(memberships: &[zedb_ch::ClusterMembership]) -> Vec<String> {
    memberships
        .iter()
        .filter(|membership| membership.hosts > 1)
        .map(|membership| membership.cluster.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn membership(cluster: &str, hosts: u64) -> zedb_ch::ClusterMembership {
        zedb_ch::ClusterMembership {
            cluster: cluster.into(),
            shard: 1,
            replica: 1,
            hosts,
        }
    }

    #[test]
    fn cluster_scope_is_judged_by_shape_not_name() {
        // Self-hosted single node: its self-only "default" is not a
        // topology.
        assert!(topology_clusters(&[membership("default", 1)]).is_empty());
        // ClickHouse Cloud: the real multi-replica cluster is
        // literally named "default" and must survive.
        assert_eq!(
            topology_clusters(&[membership("default", 3)]),
            vec!["default".to_string()]
        );
        // Mixed: only the real topology remains.
        assert_eq!(
            topology_clusters(&[membership("default", 1), membership("main", 4)]),
            vec!["main".to_string()]
        );
    }
}
