use std::time::Duration;

use gpui::{Context, Timer};

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
        self.ops.disks.clear();
        self.ops.top_tables.clear();
        self.ops.as_of = None;
        self.ops.error = None;
        self.ops.killing = None;
    }

    /// Clusters the connected node reported membership of; the scope
    /// dropdown's options. The implicit per-node "default" cluster is
    /// not a topology.
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
            .flat_map(|node| node.memberships.iter())
            .filter(|membership| membership.cluster != "default")
            .map(|membership| membership.cluster.clone())
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

    /// Fetch immediately, then every POLL_SECS while the view stays
    /// visible. Generation-guarded like the health poll; hiding the
    /// view or reconnecting ends the loop.
    pub(crate) fn ops_start_poll(&mut self, cx: &mut Context<Self>) {
        self.ops.poll_generation += 1;
        let generation = self.ops.poll_generation;
        self.ops.tick = 0;
        self.ops_fetch(cx);
        self.ops_fetch_slow(cx);
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(POLL_SECS)).await;
            let live = this
                .update(cx, |this, cx| {
                    let live = this.ops.poll_generation == generation
                        && this.show_ops
                        && this.connection.connected.is_some();
                    if live {
                        this.ops.tick += 1;
                        this.ops_fetch(cx);
                        if this.ops.tick % 5 == 0 {
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
        let config = connected.client_config.clone();
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
        let config = connected.client_config.clone();
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
        let disks_query = format!(
            "SELECT name, free_space, total_space{host_col} FROM {} ORDER BY {}",
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
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let replica_total = client.query(&replica_total_query).await;
            let replica_problems = client.query(&replica_problems_query).await;
            let queue_issues = client.query(&queue_issues_query).await;
            let keeper = client.query(&keeper_query).await;
            let disks = client.query(&disks_query).await;
            let top_tables = client.query(&top_tables_query).await;
            (
                replica_total,
                replica_problems,
                queue_issues,
                keeper,
                disks,
                top_tables,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.ops.slow_fetch_in_flight = false;
                let Ok((replica_total, replica_problems, queue_issues, keeper, disks, top_tables)) =
                    result
                else {
                    return;
                };
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
                if let Ok(disks) = disks {
                    this.ops.disks = disks
                        .rows
                        .iter()
                        .map(|row| OpsDisk {
                            name: text(row.first()),
                            free: number(row.get(1)),
                            total: number(row.get(2)),
                            node: text(row.get(3)),
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
