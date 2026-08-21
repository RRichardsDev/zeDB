#[cfg(test)]
mod gpui_tests;
mod loading;
mod model;
mod relationships;
mod selection;
#[path = "view/inspector.rs"]
mod view_inspector;
#[path = "view/parts.rs"]
mod view_parts;
#[path = "view/relationships.rs"]
mod view_relationships;
mod workload;

pub(crate) use model::{
    apply_in_place_allowed, database_nodes_from_cache, schema_object_from_cache, DatabaseNode,
    ObjectInspectorTab, PendingApply, SchemaState, SelectedSchemaObject,
};
