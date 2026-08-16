//! Per-connection schema metadata kept off the editor's network path.
//!
//! Readers take an immutable `Arc` snapshot through `ArcSwap`. Refreshes build
//! a replacement away from readers and publish it with one atomic store.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use arc_swap::ArcSwap;

use crate::{ChClient, ChError, Result};

mod model;

pub use model::{
    CachedColumn, CachedDatabase, CachedObject, CachedObjectKind, ColumnRecord, SchemaSnapshot,
    TableRecord,
};

const SNAPSHOT_FORMAT: u32 = 1;
const DEFAULT_COLUMN_DATABASE_LIMIT: usize = 64;

#[derive(Clone)]
pub struct SchemaCache {
    current: Arc<ArcSwap<SchemaSnapshot>>,
    snapshot_path: Arc<PathBuf>,
    touch_clock: Arc<AtomicU64>,
    column_database_limit: usize,
}

impl SchemaCache {
    pub fn for_connection(connection_name: &str) -> io::Result<Self> {
        let root = match std::env::var_os("ZEDB_CACHE_DIR") {
            Some(root) => PathBuf::from(root),
            None => dirs::cache_dir()
                .ok_or_else(|| io::Error::other("could not determine cache directory"))?
                .join("zedb")
                .join("schema"),
        };
        Self::open(connection_snapshot_path(&root, connection_name))
    }

    pub fn open(snapshot_path: impl Into<PathBuf>) -> io::Result<Self> {
        Self::open_with_limit(snapshot_path, DEFAULT_COLUMN_DATABASE_LIMIT)
    }

    pub fn open_with_limit(
        snapshot_path: impl Into<PathBuf>,
        column_database_limit: usize,
    ) -> io::Result<Self> {
        let snapshot_path = snapshot_path.into();
        let snapshot = match fs::read(&snapshot_path) {
            Ok(bytes) => serde_json::from_slice::<SchemaSnapshot>(&bytes)
                .ok()
                .filter(|snapshot| snapshot.format == SNAPSHOT_FORMAT)
                .unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => SchemaSnapshot::default(),
            Err(error) => return Err(error),
        };
        let touch_clock = snapshot
            .databases
            .values()
            .map(|database| database.touched)
            .max()
            .unwrap_or(0);
        Ok(Self {
            current: Arc::new(ArcSwap::from_pointee(snapshot)),
            snapshot_path: Arc::new(snapshot_path),
            touch_clock: Arc::new(AtomicU64::new(touch_clock)),
            column_database_limit: column_database_limit.max(1),
        })
    }

    pub fn snapshot(&self) -> Arc<SchemaSnapshot> {
        self.current.load_full()
    }

    /// Where this cache persists its snapshot, for read-only consumers
    /// (e.g. the MCP server serving schema_search and lint_sql).
    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn needs_columns(&self, database: &str) -> bool {
        self.snapshot().database(database).is_some_and(|database| {
            database
                .objects
                .values()
                .any(|object| object.columns.is_none())
        })
    }

    pub fn publish_tables(&self, records: Vec<TableRecord>) -> io::Result<()> {
        self.publish_catalog(records, Vec::new())
    }

    /// Publish tables plus the full database list, so databases with
    /// no tables yet (a fresh service, a just-created fleet) still
    /// exist in the snapshot instead of vanishing from the sidebar.
    pub fn publish_catalog(
        &self,
        records: Vec<TableRecord>,
        database_names: Vec<String>,
    ) -> io::Result<()> {
        let previous = self.snapshot();
        let mut databases: HashMap<String, CachedDatabase> = HashMap::new();
        for record in records {
            let old_object = previous.object(&record.database, &record.name);
            let database =
                databases
                    .entry(record.database.clone())
                    .or_insert_with(|| CachedDatabase {
                        name: record.database.clone(),
                        objects: HashMap::new(),
                        touched: previous
                            .database(&record.database)
                            .map(|database| database.touched)
                            .unwrap_or(0),
                    });
            database.objects.insert(
                record.name.clone(),
                CachedObject {
                    name: record.name,
                    engine: record.engine,
                    kind: record.kind,
                    total_rows: record.total_rows,
                    total_bytes: record.total_bytes,
                    comment: record.comment,
                    columns: old_object.and_then(|object| object.columns.clone()),
                },
            );
        }
        for name in database_names {
            databases
                .entry(name.clone())
                .or_insert_with(|| CachedDatabase {
                    name: name.clone(),
                    objects: HashMap::new(),
                    touched: previous
                        .database(&name)
                        .map(|database| database.touched)
                        .unwrap_or(0),
                });
        }
        self.publish(SchemaSnapshot {
            format: SNAPSHOT_FORMAT,
            refreshed_at_ms: now_ms(),
            databases,
        })
    }

    pub fn publish_columns(&self, database: &str, records: Vec<ColumnRecord>) -> io::Result<()> {
        let mut next = (*self.snapshot()).clone();
        let Some(target) = next.databases.get_mut(database) else {
            return Ok(());
        };
        let mut by_object: HashMap<String, HashMap<String, CachedColumn>> = HashMap::new();
        for record in records {
            by_object
                .entry(record.object)
                .or_default()
                .insert(record.column.name.clone(), record.column);
        }
        for object in target.objects.values_mut() {
            object.columns = Some(by_object.remove(&object.name).unwrap_or_default());
        }
        target.touched = self.touch_clock.fetch_add(1, Ordering::Relaxed) + 1;
        evict_cold_columns(&mut next, self.column_database_limit, database);
        next.refreshed_at_ms = now_ms();
        self.publish(next)
    }

    pub fn touch_database(&self, database: &str) {
        let mut next = (*self.snapshot()).clone();
        if let Some(target) = next.databases.get_mut(database) {
            target.touched = self.touch_clock.fetch_add(1, Ordering::Relaxed) + 1;
            self.current.store(Arc::new(next));
        }
    }

    pub fn invalidate_database(&self, database: &str) -> io::Result<()> {
        let mut next = (*self.snapshot()).clone();
        if let Some(target) = next.databases.get_mut(database) {
            for object in target.objects.values_mut() {
                object.columns = None;
            }
            next.refreshed_at_ms = now_ms();
            self.publish(next)?;
        }
        Ok(())
    }

    pub fn invalidate_object(&self, database: &str, object: &str) -> io::Result<()> {
        let mut next = (*self.snapshot()).clone();
        if let Some(target) = next
            .databases
            .get_mut(database)
            .and_then(|database| database.objects.get_mut(object))
        {
            target.columns = None;
            next.refreshed_at_ms = now_ms();
            self.publish(next)?;
        }
        Ok(())
    }

    pub async fn refresh_tables(&self, client: &ChClient) -> Result<()> {
        let mut records = client.list_schema_cache_tables().await?;
        // The database list travels separately: a freshly created
        // database has no rows in system.tables, and a cache built
        // only from tables would leave it invisible in the sidebar.
        let database_names = client
            .list_databases()
            .await?
            .into_iter()
            .map(|meta| meta.name)
            .collect::<Vec<_>>();
        // Distributed tables report no storage of their own; sum the
        // underlying local table across shards (best effort: a partly
        // unreachable cluster just leaves the totals empty).
        let distributed: Vec<(String, String)> = records
            .iter()
            .filter(|record| record.engine == "Distributed")
            .map(|record| (record.database.clone(), record.name.clone()))
            .collect();
        for (database, name) in distributed {
            let Ok(details) = client.object_details(&database, &name).await else {
                continue;
            };
            let Some((cluster, local_db, local_table)) =
                crate::schema::distributed_target(&details.engine_full)
            else {
                continue;
            };
            let Ok((bytes, rows)) = client
                .distributed_totals(&cluster, &local_db, &local_table)
                .await
            else {
                continue;
            };
            if let Some(record) = records
                .iter_mut()
                .find(|record| record.database == database && record.name == name)
            {
                record.total_bytes = bytes;
                record.total_rows = rows;
            }
        }
        self.publish_catalog(records, database_names)
            .map_err(|error| ChError::Decode(format!("could not persist schema cache: {error}")))?;
        Ok(())
    }

    pub async fn refresh_columns(&self, client: &ChClient, database: &str) -> Result<()> {
        let records = client.list_schema_cache_columns(database).await?;
        self.publish_columns(database, records)
            .map_err(|error| ChError::Decode(format!("could not persist schema cache: {error}")))?;
        Ok(())
    }

    fn publish(&self, snapshot: SchemaSnapshot) -> io::Result<()> {
        persist_atomic(&self.snapshot_path, &snapshot)?;
        self.current.store(Arc::new(snapshot));
        Ok(())
    }
}

pub fn connection_snapshot_path(root: &Path, connection_name: &str) -> PathBuf {
    let readable: String = connection_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let hash = connection_name
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    root.join(format!("{readable}-{hash:016x}.json"))
}

fn persist_atomic(path: &Path, snapshot: &SchemaSnapshot) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(snapshot).map_err(io::Error::other)?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

fn evict_cold_columns(snapshot: &mut SchemaSnapshot, limit: usize, keep: &str) {
    let mut warmed: Vec<(String, u64)> = snapshot
        .databases
        .iter()
        .filter(|(_, database)| {
            database
                .objects
                .values()
                .any(|object| object.columns.is_some())
        })
        .map(|(name, database)| (name.clone(), database.touched))
        .collect();
    if warmed.len() <= limit {
        return;
    }
    warmed.sort_by_key(|(_, touched)| *touched);
    let remove = warmed.len() - limit;
    for (name, _) in warmed
        .into_iter()
        .filter(|(name, _)| name != keep)
        .take(remove)
    {
        if let Some(database) = snapshot.databases.get_mut(&name) {
            for object in database.objects.values_mut() {
                object.columns = None;
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use tempfile::tempdir;

    use super::*;

    fn tables(databases: usize, objects: usize) -> Vec<TableRecord> {
        (0..databases)
            .flat_map(|database| {
                (0..objects).map(move |object| TableRecord {
                    total_bytes: None,
                    database: format!("db_{database:04}"),
                    name: format!("table_{object:03}"),
                    engine: "MergeTree".into(),
                    kind: CachedObjectKind::Table,
                    total_rows: Some(10),
                    comment: String::new(),
                })
            })
            .collect()
    }

    fn columns(objects: usize) -> Vec<ColumnRecord> {
        (0..objects)
            .flat_map(|object| {
                (0..20).map(move |column| ColumnRecord {
                    object: format!("table_{object:03}"),
                    column: CachedColumn {
                        name: format!("column_{column:03}"),
                        type_name: "UInt64".into(),
                        codec_expression: String::new(),
                        comment: String::new(),
                    },
                })
            })
            .collect()
    }

    #[test]
    fn empty_databases_survive_a_catalog_publish() {
        let directory = tempdir().unwrap();
        let cache = SchemaCache::open(directory.path().join("schema.json")).unwrap();
        cache
            .publish_catalog(tables(1, 2), vec!["db_0000".into(), "just_created".into()])
            .unwrap();
        let snapshot = cache.snapshot();
        assert_eq!(snapshot.databases.len(), 2);
        let empty = snapshot.databases.get("just_created").unwrap();
        assert!(empty.objects.is_empty());
    }

    #[test]
    fn persisted_snapshot_is_available_on_open() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("schema.json");
        let cache = SchemaCache::open(&path).unwrap();
        cache.publish_tables(tables(2, 3)).unwrap();
        cache.publish_columns("db_0001", columns(3)).unwrap();

        let reopened = SchemaCache::open(path).unwrap();
        assert_eq!(reopened.snapshot().databases.len(), 2);
        assert_eq!(
            reopened
                .snapshot()
                .column("db_0001", "table_001", "column_019")
                .unwrap()
                .type_name,
            "UInt64"
        );
    }

    #[test]
    fn tables_warm_before_columns_on_large_fleet() {
        let directory = tempdir().unwrap();
        let cache = SchemaCache::open(directory.path().join("schema.json")).unwrap();
        cache.publish_tables(tables(500, 20)).unwrap();

        let snapshot = cache.snapshot();
        assert_eq!(snapshot.databases.len(), 500);
        assert_eq!(snapshot.warmed_databases(), 0);
        assert!(snapshot.object("db_0499", "table_019").is_some());
    }

    #[test]
    fn cold_column_sets_are_evicted_without_losing_tables() {
        let directory = tempdir().unwrap();
        let cache = SchemaCache::open_with_limit(directory.path().join("schema.json"), 2).unwrap();
        cache.publish_tables(tables(4, 1)).unwrap();
        for database in ["db_0000", "db_0001", "db_0002"] {
            cache.publish_columns(database, columns(1)).unwrap();
        }

        let snapshot = cache.snapshot();
        assert_eq!(snapshot.databases.len(), 4);
        assert_eq!(snapshot.warmed_databases(), 2);
        assert!(snapshot
            .object("db_0002", "table_000")
            .unwrap()
            .columns
            .is_some());
    }

    #[test]
    fn invalidation_becomes_uncertain_instead_of_unknown() {
        let directory = tempdir().unwrap();
        let cache = SchemaCache::open(directory.path().join("schema.json")).unwrap();
        cache.publish_tables(tables(1, 1)).unwrap();
        cache.publish_columns("db_0000", columns(1)).unwrap();
        assert!(cache
            .snapshot()
            .column("db_0000", "table_000", "column_000")
            .is_some());

        cache.invalidate_database("db_0000").unwrap();
        assert!(cache.snapshot().object("db_0000", "table_000").is_some());
        assert!(cache
            .snapshot()
            .object("db_0000", "table_000")
            .unwrap()
            .columns
            .is_none());
    }

    #[test]
    fn lookups_remain_cheap_on_synthetic_large_fleet() {
        let directory = tempdir().unwrap();
        let cache = SchemaCache::open(directory.path().join("schema.json")).unwrap();
        cache.publish_tables(tables(500, 20)).unwrap();
        cache.publish_columns("db_0250", columns(20)).unwrap();
        let snapshot = cache.snapshot();
        let started = Instant::now();
        for _ in 0..100_000 {
            assert!(snapshot
                .column("db_0250", "table_019", "column_019")
                .is_some());
        }
        assert!(started.elapsed().as_millis() < 250);
    }

    #[test]
    fn snapshot_paths_are_stable_and_connection_specific() {
        let root = Path::new("/tmp/cache");
        assert_eq!(
            connection_snapshot_path(root, "Production EU"),
            connection_snapshot_path(root, "Production EU")
        );
        assert_ne!(
            connection_snapshot_path(root, "Production EU"),
            connection_snapshot_path(root, "Production US")
        );
    }
}
