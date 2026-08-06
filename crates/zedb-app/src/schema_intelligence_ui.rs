use std::{cell::RefCell, rc::Rc, sync::Arc};

use anyhow::Result;
use gpui::{App, Context, Task, Window};
use gpui_component::{
    input::{CompletionProvider, DocumentColorProvider, HoverProvider, InputState},
    Rope,
};
use lsp_types::{
    Color, ColorInformation, CompletionContext, CompletionItem, CompletionItemKind,
    CompletionResponse, CompletionTextEdit, Hover, HoverContents, MarkupContent, MarkupKind,
    Position, Range, TextEdit,
};
use zedb_ch::{
    schema_cache::{SchemaCache, SchemaSnapshot},
    schema_intelligence::{self, RecognizedKind, SuggestionKind},
};

#[derive(Default)]
struct ProviderContext {
    cache: Option<SchemaCache>,
    default_database: Option<String>,
}

pub struct SchemaProvider {
    context: RefCell<ProviderContext>,
}

impl SchemaProvider {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            context: RefCell::new(ProviderContext::default()),
        })
    }

    pub fn set_context(&self, cache: Option<SchemaCache>, default_database: Option<String>) {
        *self.context.borrow_mut() = ProviderContext {
            cache,
            default_database,
        };
    }

    pub fn snapshot(&self) -> Option<(Arc<SchemaSnapshot>, Option<String>)> {
        let context = self.context.borrow();
        Some((
            context.cache.as_ref()?.snapshot(),
            context.default_database.clone(),
        ))
    }
}

impl CompletionProvider for SchemaProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let Some((snapshot, default_database)) = self.snapshot() else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let sql = text.to_string();
        let items =
            schema_intelligence::completions(&snapshot, default_database.as_deref(), &sql, offset)
                .into_iter()
                .map(|suggestion| CompletionItem {
                    label: suggestion.label.clone(),
                    detail: (!suggestion.detail.is_empty()).then_some(suggestion.detail),
                    kind: Some(match suggestion.kind {
                        SuggestionKind::Database => CompletionItemKind::MODULE,
                        SuggestionKind::Object => CompletionItemKind::STRUCT,
                        SuggestionKind::Column => CompletionItemKind::FIELD,
                    }),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: byte_range_to_lsp(&sql, suggestion.replace),
                        new_text: suggestion.label,
                    })),
                    ..Default::default()
                })
                .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut Context<InputState>) -> bool {
        new_text.chars().last().is_some_and(|character| {
            character == '.' || character == '_' || character.is_alphanumeric()
        })
    }
}

impl HoverProvider for SchemaProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let Some((snapshot, default_database)) = self.snapshot() else {
            return Task::ready(Ok(None));
        };
        let sql = text.to_string();
        let hover =
            schema_intelligence::hover(&snapshot, default_database.as_deref(), &sql, offset).map(
                |hover| Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover.markdown,
                    }),
                    range: Some(byte_range_to_lsp(&sql, hover.range)),
                },
            );
        Task::ready(Ok(hover))
    }
}

impl DocumentColorProvider for SchemaProvider {
    fn document_colors(
        &self,
        text: &Rope,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<Vec<ColorInformation>>> {
        let Some((snapshot, default_database)) = self.snapshot() else {
            return Task::ready(Ok(Vec::new()));
        };
        let sql = text.to_string();
        let colors = schema_intelligence::recognized_identifiers(
            &snapshot,
            default_database.as_deref(),
            &sql,
        )
        .into_iter()
        .map(|identifier| {
            // Rendered as text color by the vendored editor patch: light
            // blue for recognized databases, soft green for tables and
            // views, a paler blue for columns.
            let color = match identifier.kind {
                RecognizedKind::Database => Color {
                    red: 0.42,
                    green: 0.68,
                    blue: 0.96,
                    alpha: 1.0,
                },
                RecognizedKind::Object => Color {
                    red: 0.45,
                    green: 0.83,
                    blue: 0.65,
                    alpha: 1.0,
                },
                RecognizedKind::Column => Color {
                    red: 0.63,
                    green: 0.78,
                    blue: 0.95,
                    alpha: 1.0,
                },
            };
            ColorInformation {
                range: byte_range_to_lsp(&sql, identifier.range),
                color,
            }
        })
        .collect();
        Task::ready(Ok(colors))
    }
}

pub fn byte_range_to_lsp(sql: &str, range: std::ops::Range<usize>) -> Range {
    Range {
        start: byte_offset_to_lsp(sql, range.start),
        end: byte_offset_to_lsp(sql, range.end),
    }
}

fn byte_offset_to_lsp(sql: &str, offset: usize) -> Position {
    let prefix = &sql[..offset.min(sql.len())];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let character = prefix[line_start..].encode_utf16().count() as u32;
    Position::new(line, character)
}
