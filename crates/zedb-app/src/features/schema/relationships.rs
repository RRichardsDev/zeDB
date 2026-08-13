use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// Load the materialized-view lineage for the selected object
    /// (Phase 9, Part C), off-thread. No-op if already loaded or loading.
    pub(crate) fn load_dependencies(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.dependencies.is_some() || selected.dependencies_loading {
                return;
            }
            let Some(connected) = &self.connection.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.schema.selected_object {
            selected.dependencies_loading = true;
            selected.dependencies_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .object_dependencies(&database_name, &object_name)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this
                    .connection
                    .connected
                    .as_ref()
                    .map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.schema.selected_object else {
                    return;
                };
                if selected.database != database_name || selected.object.name != object_name {
                    return;
                }
                selected.dependencies_loading = false;
                match result {
                    Ok(Ok(dependencies)) => selected.dependencies = Some(dependencies),
                    Ok(Err(error)) => selected.dependencies_error = Some(error.to_string()),
                    Err(error) => selected.dependencies_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load the projections attached to the selected object (Phase 9,
    /// Part C), off-thread. No-op if already loaded or loading.
    pub(crate) fn load_projections(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.projections.is_some() || selected.projections_loading {
                return;
            }
            let Some(connected) = &self.connection.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.schema.selected_object {
            selected.projections_loading = true;
            selected.projections_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .table_projections(&database_name, &object_name)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this
                    .connection
                    .connected
                    .as_ref()
                    .map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.schema.selected_object else {
                    return;
                };
                if selected.database != database_name || selected.object.name != object_name {
                    return;
                }
                selected.projections_loading = false;
                match result {
                    Ok(Ok(projections)) => selected.projections = Some(projections),
                    Ok(Err(error)) => selected.projections_error = Some(error.to_string()),
                    Err(error) => selected.projections_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Select the `db.table` a dependency node points at, landing on the
    /// Dependencies tab so the lineage can be walked node by node. Uses the
    /// loaded schema meta when available, else a minimal placeholder the
    /// detail load fills in.
    pub(crate) fn navigate_to_dependency(
        &mut self,
        full: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((database, name)) = full.split_once('.') else {
            return;
        };
        let meta = self
            .schema
            .databases
            .iter()
            .find(|node| node.meta.name == database)
            .and_then(|node| node.objects.as_ref())
            .and_then(|objects| objects.iter().find(|object| object.name == name).cloned())
            .unwrap_or_else(|| SchemaObjectMeta {
                name: name.to_string(),
                engine: String::new(),
                kind: SchemaObjectKind::Table,
                total_rows: None,
                total_bytes: None,
            });
        self.select_schema_object(
            database.to_string(),
            meta,
            ObjectInspectorTab::Dependencies,
            window,
            cx,
        );
    }
}
