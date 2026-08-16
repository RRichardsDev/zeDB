use std::collections::HashMap;

use gpui::Entity;
use gpui_component::input::InputState;
use zedb_ch::{
    schema_cache::{CachedObject, CachedObjectKind, SchemaCache},
    ColumnInfo, DatabaseMeta, ObjectDetails, SchemaObjectKind, SchemaObjectMeta, TableStorage,
};

use crate::components::text_input::TextInput;
use crate::schema_intelligence_ui::SchemaProvider;

pub(crate) struct SchemaState {
    pub(crate) filter: Entity<TextInput>,
    pub(crate) connection: Option<String>,
    pub(crate) cache: Option<SchemaCache>,
    pub(crate) provider: std::rc::Rc<SchemaProvider>,
    pub(crate) warming: std::collections::HashSet<String>,
    pub(crate) loading: bool,
    pub(crate) databases: Vec<DatabaseNode>,
    pub(crate) error: Option<String>,
    pub(crate) selected_object: Option<SelectedSchemaObject>,
    pub(crate) cardinality_cache: HashMap<(String, String, String), Vec<u64>>,
    pub(crate) measured_cache: HashMap<(String, String, String), HashMap<usize, f64>>,
    pub(crate) pending_apply: Option<(usize, Vec<String>)>,
}

impl SchemaState {
    pub(crate) fn new(filter: Entity<TextInput>, provider: std::rc::Rc<SchemaProvider>) -> Self {
        Self {
            filter,
            connection: None,
            cache: None,
            provider,
            warming: std::collections::HashSet::new(),
            loading: false,
            databases: Vec::new(),
            error: None,
            selected_object: None,
            cardinality_cache: HashMap::new(),
            measured_cache: HashMap::new(),
            pending_apply: None,
        }
    }
}

pub(crate) struct DatabaseNode {
    pub(crate) meta: DatabaseMeta,
    pub(crate) expanded: bool,
    pub(crate) filter_collapsed: bool,
    pub(crate) loading: bool,
    pub(crate) objects: Option<Vec<SchemaObjectMeta>>,
    pub(crate) error: Option<String>,
}

pub(crate) fn database_nodes_from_cache(cache: &SchemaCache) -> Vec<DatabaseNode> {
    let snapshot = cache.snapshot();
    let mut databases: Vec<_> = snapshot
        .databases
        .values()
        .map(|database| DatabaseNode {
            meta: DatabaseMeta {
                name: database.name.clone(),
            },
            expanded: false,
            filter_collapsed: false,
            loading: false,
            objects: None,
            error: None,
        })
        .collect();
    databases.sort_by(|left, right| left.meta.name.cmp(&right.meta.name));
    databases
}

pub(crate) fn schema_object_from_cache(object: &CachedObject) -> SchemaObjectMeta {
    SchemaObjectMeta {
        name: object.name.clone(),
        engine: object.engine.clone(),
        kind: match object.kind {
            CachedObjectKind::Table => SchemaObjectKind::Table,
            CachedObjectKind::View => SchemaObjectKind::View,
            CachedObjectKind::MaterializedView => SchemaObjectKind::MaterializedView,
            CachedObjectKind::Dictionary => SchemaObjectKind::Dictionary,
        },
        total_rows: object.total_rows,
        total_bytes: object.total_bytes,
    }
}

pub(crate) struct SelectedSchemaObject {
    pub(crate) database: String,
    pub(crate) object: SchemaObjectMeta,
    pub(crate) loading: bool,
    pub(crate) columns: Vec<ColumnInfo>,
    pub(crate) details: Option<ObjectDetails>,
    pub(crate) storage: Option<TableStorage>,
    pub(crate) cardinalities: Option<Vec<u64>>,
    pub(crate) cardinality_loading: bool,
    pub(crate) cardinality_error: Option<String>,
    pub(crate) cardinality_confirming: bool,
    pub(crate) measured: HashMap<usize, f64>,
    pub(crate) applying: Option<usize>,
    pub(crate) applying_slow: bool,
    pub(crate) partitions: Option<Vec<zedb_ch::PartitionStats>>,
    pub(crate) partitions_loading: bool,
    pub(crate) partitions_error: Option<String>,
    pub(crate) merges: Vec<zedb_ch::MergeInfo>,
    pub(crate) dependencies: Option<zedb_ch::ObjectDependencies>,
    pub(crate) dependencies_loading: bool,
    pub(crate) dependencies_error: Option<String>,
    pub(crate) projections: Option<Vec<zedb_ch::ProjectionInfo>>,
    pub(crate) projections_loading: bool,
    pub(crate) projections_error: Option<String>,
    pub(crate) workload: Option<zedb_ch::workload::WorkloadReport>,
    pub(crate) workload_loading: bool,
    pub(crate) workload_error: Option<String>,
    pub(crate) ddl_editor: Entity<InputState>,
    pub(crate) engine_editor: Entity<InputState>,
    pub(crate) tab: ObjectInspectorTab,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectInspectorTab {
    Overview,
    Columns,
    Parts,
    Projections,
    Workload,
    Dependencies,
    Ddl,
}
