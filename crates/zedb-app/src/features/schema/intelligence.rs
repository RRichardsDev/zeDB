use std::{cell::RefCell, rc::Rc, sync::Arc};

use anyhow::Result;
use gpui::{App, Context, Task, Window};
use gpui_component::{
    input::{CompletionProvider, HoverProvider, InputState},
    Rope,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Hover, HoverContents, MarkupContent, MarkupKind, Position, Range, TextEdit,
};
use zedb_ch::{
    schema_cache::{SchemaCache, SchemaSnapshot},
    schema_intelligence::{self, SuggestionKind},
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
        // Placeholder values in effect at the cursor, so `${db}.` and
        // `{db:Identifier}.` complete like the database they name.
        let variables = crate::collect_variable_declarations(&sql)
            .map(|declarations| crate::params_at(&declarations, Some(offset)))
            .unwrap_or_default();
        let params = crate::params_at(&crate::collect_param_declarations(&sql), Some(offset));
        let items = schema_intelligence::completions_with_placeholders(
            &snapshot,
            default_database.as_deref(),
            &sql,
            offset,
            &variables,
            &params,
        )
        .into_iter()
        .map(|suggestion| CompletionItem {
            label: suggestion.label.clone(),
            detail: (!suggestion.detail.is_empty()).then_some(suggestion.detail),
            kind: Some(match suggestion.kind {
                SuggestionKind::Database => CompletionItemKind::MODULE,
                SuggestionKind::Object => CompletionItemKind::STRUCT,
                SuggestionKind::Column => CompletionItemKind::FIELD,
                SuggestionKind::Function => CompletionItemKind::FUNCTION,
                SuggestionKind::Keyword => CompletionItemKind::KEYWORD,
                SuggestionKind::Type => CompletionItemKind::TYPE_PARAMETER,
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
            // ':' opens the type menu inside a `{name:Type}` placeholder.
            character == '.' || character == '_' || character == ':' || character.is_alphanumeric()
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
        let sql = text.to_string();
        // Placeholders first: hovering `${db}` or `{db:Identifier}` shows
        // the value in effect there, without needing a schema snapshot.
        if let Some((mut markdown, value, range)) = crate::variable_hover(&sql, offset) {
            // The value often names something the schema knows; say what
            // it is, the way hovering the name directly would.
            if let Some(value) = value {
                if let Some((snapshot, default_database)) = self.snapshot() {
                    if let Some(database) = snapshot
                        .databases
                        .values()
                        .find(|database| database.name.eq_ignore_ascii_case(&value))
                    {
                        markdown.push_str(&format!(
                            "\n\nDatabase with {} objects",
                            database.objects.len()
                        ));
                    } else if let Some(object) = default_database
                        .as_deref()
                        .and_then(|database| snapshot.object(database, &value))
                    {
                        markdown.push_str(&format!("\n\n{} table", object.engine));
                    }
                }
            }
            return Task::ready(Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(byte_range_to_lsp(&sql, range)),
            })));
        }
        let Some((snapshot, default_database)) = self.snapshot() else {
            return Task::ready(Ok(None));
        };
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
