//! Export current query results (docs/PHASE-7.1-IDEAS.md): palette
//! only. Step one picks the scope (the tab's max-rows cap, or all
//! rows), step two the format, with the location defaulting quietly
//! to ~/Downloads. The download streams the server's own output
//! format straight to disk, bypassing decode and the grid.

use gpui::Context;

use crate::components::text_input::TextInput;
use crate::{rt, Workspace};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Parquet,
    JsonEachRow,
}

impl ExportFormat {
    pub(crate) const ALL: [ExportFormat; 3] = [
        ExportFormat::Csv,
        ExportFormat::Parquet,
        ExportFormat::JsonEachRow,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Parquet => "Parquet",
            ExportFormat::JsonEachRow => "JSONEachRow",
        }
    }

    /// The ClickHouse FORMAT name.
    fn format_name(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSVWithNames",
            ExportFormat::Parquet => "Parquet",
            ExportFormat::JsonEachRow => "JSONEachRow",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Parquet => "parquet",
            ExportFormat::JsonEachRow => "jsonl",
        }
    }
}

pub enum ExportStep {
    /// How many rows: the tab's cap, or everything.
    Scope,
    Configure,
}

pub struct ExportState {
    pub step: ExportStep,
    pub statement: String,
    /// None = all rows; Some(n) = wrap in LIMIT n.
    pub row_cap: Option<usize>,
    pub format: ExportFormat,
    pub path_input: gpui::Entity<TextInput>,
    /// The quiet default location is shown as text until edited.
    pub editing_path: bool,
    pub running: bool,
    pub progress_bytes: u64,
    pub started_at: Option<std::time::Instant>,
    /// Abort handle for a running download; Cancel kills and closes.
    pub abort: Option<tokio::task::AbortHandle>,
    pub error: Option<String>,
    pub generation: u64,
}

fn default_path(format: ExportFormat) -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let downloads = dirs::download_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("zedb-export-{stamp}.{}", format.extension()));
    downloads.to_string_lossy().to_string()
}

impl Workspace {
    /// Palette entry: only makes sense with a displayed result.
    pub(crate) fn export_available(&self) -> bool {
        self.connection.connected.is_some()
            && self
                .query
                .tabs
                .get(self.query.active_tab)
                .and_then(|tab| tab.displayed_statement.as_ref())
                .is_some()
    }

    pub(crate) fn export_open(&mut self, cx: &mut Context<Self>) {
        let Some(statement) = self
            .query
            .tabs
            .get(self.query.active_tab)
            .and_then(|tab| tab.displayed_statement.clone())
        else {
            self.flash_warning("Run a query first, then export its results", cx);
            return;
        };
        let format = ExportFormat::Csv;
        let path_input = Self::input(default_path(format), "File path", false, cx);
        self.export = Some(ExportState {
            step: ExportStep::Scope,
            statement,
            row_cap: None,
            format,
            path_input,
            editing_path: false,
            running: false,
            progress_bytes: 0,
            started_at: None,
            abort: None,
            error: None,
            generation: 0,
        });
        cx.notify();
    }

    pub(crate) fn export_set_format(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let Some(export) = self.export.as_mut() else {
            return;
        };
        export.format = format;
        // The quiet default tracks the format; a hand-edited path is
        // the user's to keep.
        if !export.editing_path {
            let path = default_path(format);
            export.path_input.update(cx, |input, cx| {
                input.set_text(path, cx);
            });
        }
        cx.notify();
    }

    /// Native save panel; lands the chosen path in the input.
    pub(crate) fn export_browse(&mut self, cx: &mut Context<Self>) {
        let Some(export) = self.export.as_ref() else {
            return;
        };
        let current = std::path::PathBuf::from(export.path_input.read(cx).text());
        let directory = current
            .parent()
            .map(|parent| parent.to_path_buf())
            .filter(|parent| parent.is_dir())
            .or_else(dirs::download_dir)
            .unwrap_or_else(std::env::temp_dir);
        let suggested = current
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        let receiver = cx.prompt_for_new_path(&directory, suggested.as_deref());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(chosen))) = receiver.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(export) = this.export.as_ref() {
                    let text = chosen.to_string_lossy().to_string();
                    export.path_input.update(cx, |input, cx| {
                        input.set_text(text, cx);
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn export_start(&mut self, cx: &mut Context<Self>) {
        let Some(connected) = self.connection.connected.as_ref() else {
            return;
        };
        let config = connected.client_config.clone();
        let Some(export) = self.export.as_mut() else {
            return;
        };
        if export.running {
            return;
        }
        let path = export.path_input.read(cx).text().trim().to_string();
        if path.is_empty() {
            export.error = Some("Choose a file path".into());
            cx.notify();
            return;
        }
        let statement = export.statement.trim().trim_end_matches(';').to_string();
        let sql = match export.row_cap {
            Some(cap) => format!("SELECT * FROM (\n{statement}\n) LIMIT {cap}"),
            None => statement,
        };
        let format_name = export.format.format_name();
        export.running = true;
        export.error = None;
        export.progress_bytes = 0;
        export.started_at = Some(std::time::Instant::now());
        export.generation += 1;
        let generation = export.generation;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let path_buf = std::path::PathBuf::from(path.clone());
        let task = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            client
                .download_to_file(&sql, format_name, &path_buf, |written| {
                    let _ = sender.send(written);
                })
                .await
        });
        export.abort = Some(task.abort_handle());
        // Progress pump: throttled by the channel draining per update.
        cx.spawn(async move |this, cx| {
            while let Some(written) = receiver.recv().await {
                let live = this
                    .update(cx, |this, cx| {
                        let Some(export) = this.export.as_mut() else {
                            return false;
                        };
                        if export.generation != generation {
                            return false;
                        }
                        export.progress_bytes = written;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let Some(export) = this.export.as_mut() else {
                    return;
                };
                if export.generation != generation {
                    return;
                }
                export.running = false;
                match outcome {
                    Ok(Ok(written)) => {
                        this.export = None;
                        this.notice = Some(format!(
                            "Exported {} to {path}",
                            Self::format_bytes(written)
                        ));
                        this.notice_warning = false;
                        this.notice_flash_id += 1;
                    }
                    Ok(Err(error)) => export.error = Some(error.to_string()),
                    Err(_) => export.error = Some("export task failed".into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Cancel closes the dialog; a running download is aborted and
    /// its partial file removed.
    pub(crate) fn export_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(export) = self.export.take() {
            if let Some(abort) = export.abort {
                abort.abort();
            }
            if export.running {
                let path = export.path_input.read(cx).text().trim().to_string();
                if !path.is_empty() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        cx.notify();
    }
}
