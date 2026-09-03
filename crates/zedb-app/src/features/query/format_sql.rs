//! Format SQL: the selection or the whole buffer re-laid by the connected
//! server's own formatter, `formatQuery()`, so the layout is exactly
//! how ClickHouse reads the statement (lambdas, SETTINGS, placeholders,
//! table functions) rather than a generic SQL pretty-printer's guess.
//!
//! The server formats one statement at a time and drops comments, so
//! the work is planned here: each statement goes up on its own,
//! leading comment lines are carried over by hand, and a statement the
//! formatter would damage (a comment inside it) or cannot parse (a
//! `${var}` use, an `@set` line) is left exactly as written and the
//! status line says so. The edit goes through the editor's normal
//! replace path, so cmd-z undoes it in one step.

use gpui::{Context, EntityInputHandler, Window};

use super::{split_statements, sql_is_blank};
use crate::{rt, Workspace};

/// What happens to one statement of the target text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    /// Left byte-for-byte as written.
    Keep,
    /// Sent to the server; `leading` (comment lines above the
    /// statement, already trimmed) is put back above the result.
    Format { leading: Vec<String>, body: String },
}

#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// One per `split_statements` segment, in order, with its range.
    pub(crate) units: Vec<(usize, usize, Unit)>,
    /// Statements kept because a comment sits inside them.
    pub(crate) kept_comments: usize,
    /// Statements kept because they use `${var}` variables.
    pub(crate) kept_variables: usize,
}

impl Plan {
    pub(crate) fn bodies(&self) -> Vec<String> {
        self.units
            .iter()
            .filter_map(|(_, _, unit)| match unit {
                Unit::Format { body, .. } => Some(body.clone()),
                Unit::Keep => None,
            })
            .collect()
    }

    /// Why nothing at all can be formatted, when that is the case.
    fn nothing_reason(&self) -> &'static str {
        if self.kept_comments > 0 {
            "Nothing to format: every statement has a comment inside it, which the server's formatter would drop"
        } else if self.kept_variables > 0 {
            "Nothing to format: every statement uses ${var} variables the server cannot parse"
        } else {
            "Nothing to format"
        }
    }
}

pub(crate) fn plan(text: &str) -> Plan {
    let mut plan = Plan::default();
    for (start, end) in split_statements(text) {
        let segment = &text[start..end.min(text.len())];
        let unit = if sql_is_blank(segment) || segment.trim_start().starts_with("@set") {
            Unit::Keep
        } else {
            let (leading, body_start, inline_comment) = scan_comments(segment);
            let body = segment[body_start..].trim();
            if inline_comment {
                plan.kept_comments += 1;
                Unit::Keep
            } else if body.contains("${") {
                plan.kept_variables += 1;
                Unit::Keep
            } else {
                Unit::Format {
                    leading,
                    body: body.to_string(),
                }
            }
        };
        plan.units.push((start, end, unit));
    }
    plan
}

/// The comment lines before the statement's first token, where the
/// body starts, and whether any comment sits inside the body. Strings
/// and backticked names are skipped so `'--'` is not a comment.
fn scan_comments(segment: &str) -> (Vec<String>, usize, bool) {
    let bytes = segment.as_bytes();
    let mut leading = Vec::new();
    let mut i = 0;
    let mut body_start = None;
    let mut inline = false;
    while i < bytes.len() {
        match bytes[i] {
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                let start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                if body_start.is_none() {
                    leading.push(segment[start..i].trim_end().to_string());
                } else {
                    inline = true;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                if body_start.is_none() {
                    leading.push(segment[start..i].to_string());
                } else {
                    inline = true;
                }
            }
            quote @ (b'\'' | b'"' | b'`') => {
                body_start.get_or_insert(i);
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && quote != b'`' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            byte if byte.is_ascii_whitespace() => i += 1,
            _ => {
                body_start.get_or_insert(i);
                i += 1;
            }
        }
    }
    (leading, body_start.unwrap_or(bytes.len()), inline)
}

/// The target text with every formatted statement swapped in. The
/// separators between segments (`;`, or the newline after an `@set`
/// line) are the original's; a formatted statement after a `;` gets
/// a blank line above it.
pub(crate) fn assemble(text: &str, plan: &Plan, formatted: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut formatted = formatted.iter();
    let mut previous_end = 0;
    for (index, (start, end, unit)) in plan.units.iter().enumerate() {
        if index > 0 {
            out.push_str(&text[previous_end..*start]);
        }
        match unit {
            Unit::Keep => out.push_str(&text[*start..*end]),
            Unit::Format { leading, .. } => {
                let after_semicolon = *start > 0 && text[..*start].ends_with(';');
                if after_semicolon {
                    out.push_str("\n\n");
                }
                for line in leading {
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str(formatted.next().map(String::as_str).unwrap_or_default());
            }
        }
        previous_end = *end;
    }
    out.push_str(&text[previous_end.min(text.len())..]);
    out
}

/// Single-quote a string literal, escaping backslashes and quotes.
fn quote_string(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The server's message without the framing a status line has no room
/// for: `Code: 62. DB::Exception: `, the list of expected tokens, and
/// anything past the first line or a status line's width.
fn short_error(error: &zedb_ch::ChError) -> String {
    let message = match error {
        zedb_ch::ChError::Server { message, .. } => message.clone(),
        other => other.to_string(),
    };
    let message = message
        .split_once("DB::Exception: ")
        .map(|(_, rest)| rest)
        .unwrap_or(&message);
    let cut = ["Expected one of", ". In scope", " (SYNTAX_ERROR)"]
        .iter()
        .filter_map(|marker| message.find(marker))
        .min()
        .unwrap_or(message.len());
    let message = message[..cut]
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches([':', '.', ' ']);
    const WIDTH: usize = 120;
    match message.char_indices().nth(WIDTH) {
        Some((index, _)) => format!("{}\u{2026}", &message[..index]),
        None => message.to_string(),
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

impl Workspace {
    /// Command palette: Format SQL. The selection when there is one,
    /// otherwise the whole buffer.
    pub(crate) fn format_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connected) = self.connection.connected.as_ref() else {
            self.flash_warning(
                "Connect to a cluster before formatting; the server's formatQuery does the work",
                cx,
            );
            return;
        };
        let config = connected.client_config.clone();
        let Some(tab) = self.query.tabs.get(self.query.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        let editor = tab.editor.clone();
        let buffer_before = editor.read(cx).value().to_string();
        let (range_utf16, text) = editor.update(cx, |editor, cx| {
            let selection = EntityInputHandler::selected_text_range(editor, false, window, cx)
                .filter(|selection| !selection.range.is_empty());
            match selection {
                Some(selection) => {
                    let text = EntityInputHandler::text_for_range(
                        editor,
                        selection.range.clone(),
                        &mut None,
                        window,
                        cx,
                    )
                    .unwrap_or_default();
                    (selection.range, text)
                }
                None => (
                    0..buffer_before.encode_utf16().count(),
                    buffer_before.clone(),
                ),
            }
        });
        if sql_is_blank(&text) {
            self.flash_warning("Nothing to format", cx);
            return;
        }
        let plan = plan(&text);
        let bodies = plan.bodies();
        if bodies.is_empty() {
            self.flash_warning(plan.nothing_reason(), cx);
            return;
        }
        let task = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let mut formatted = Vec::with_capacity(bodies.len());
            for (index, body) in bodies.into_iter().enumerate() {
                // A literal, not a query parameter: the server parses
                // a String parameter in escaped form, so a raw newline
                // ends the value (BAD_QUERY_PARAMETER).
                let result = client
                    .query(&format!("SELECT formatQuery({})", quote_string(&body)))
                    .await
                    .map_err(|error| (index + 1, error))?;
                formatted.push(
                    result
                        .rows
                        .first()
                        .and_then(|row| row.first())
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                );
            }
            Ok::<_, (usize, zedb_ch::ChError)>(formatted)
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(tab) = this.query.tabs.iter().find(|tab| tab.id == tab_id) else {
                    return;
                };
                let editor = tab.editor.clone();
                match outcome {
                    Ok(Ok(formatted)) => {
                        if editor.read(cx).value().as_ref() != buffer_before {
                            this.flash_warning(
                                "The editor changed while formatting; nothing applied",
                                cx,
                            );
                            return;
                        }
                        let updated = assemble(&text, &plan, &formatted);
                        if updated == text {
                            this.flash_notice("Already formatted; nothing to change", cx);
                            return;
                        }
                        editor.update(cx, |editor, cx| {
                            EntityInputHandler::replace_text_in_range(
                                editor,
                                Some(range_utf16),
                                &updated,
                                window,
                                cx,
                            );
                        });
                        this.refresh_schema_diagnostics(cx);
                        let mut message =
                            format!("Formatted {}", plural(formatted.len(), "statement"));
                        if plan.kept_comments > 0 {
                            message.push_str(&format!(
                                "; kept {} with a comment inside as written",
                                plural(plan.kept_comments, "statement")
                            ));
                        }
                        if plan.kept_variables > 0 {
                            message.push_str(&format!(
                                "; kept {} using ${{var}} as written",
                                plural(plan.kept_variables, "statement")
                            ));
                        }
                        this.flash_notice(message, cx);
                    }
                    Ok(Err((
                        _,
                        zedb_ch::ChError::Server {
                            code: Some(46), ..
                        },
                    ))) => this.flash_warning(
                        "Format SQL needs ClickHouse 23.10 or newer: this server has no formatQuery()",
                        cx,
                    ),
                    Ok(Err((statement, error))) => this.flash_warning(
                        format!(
                            "Format failed on statement {statement}: {}",
                            short_error(&error)
                        ),
                        cx,
                    ),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted_for(plan: &Plan) -> Vec<String> {
        plan.bodies()
            .into_iter()
            .map(|body| format!("<{}>", body.to_ascii_uppercase()))
            .collect()
    }

    #[test]
    fn statements_go_up_one_at_a_time_with_leading_comments_kept() {
        let text = "-- head\n/* block */\nselect 1;\n\nselect 2;\n";
        let plan = plan(text);
        assert_eq!(plan.bodies(), vec!["select 1", "select 2"]);
        assert_eq!(
            plan.units[0].2,
            Unit::Format {
                leading: vec!["-- head".into(), "/* block */".into()],
                body: "select 1".into()
            }
        );
        assert_eq!(
            assemble(text, &plan, &formatted_for(&plan)),
            "-- head\n/* block */\n<SELECT 1>;\n\n<SELECT 2>;\n"
        );
    }

    #[test]
    fn inline_comments_variables_and_directives_are_left_as_written() {
        let text = "@set db=analytics\nselect 1 -- why\nfrom t;\nselect ${db}.x;\nselect 'a--b'";
        let plan = plan(text);
        assert_eq!(plan.kept_comments, 1);
        assert_eq!(plan.kept_variables, 1);
        // Only the last statement is formattable: the `--` inside a
        // string is not a comment.
        assert_eq!(plan.bodies(), vec!["select 'a--b'"]);
        assert_eq!(
            assemble(text, &plan, &formatted_for(&plan)),
            "@set db=analytics\nselect 1 -- why\nfrom t;\nselect ${db}.x;\n\n<SELECT 'A--B'>"
        );
    }

    #[test]
    fn a_statement_after_a_directive_line_keeps_the_single_newline() {
        let text = "@set a=1\nselect 1";
        let plan = plan(text);
        assert_eq!(
            assemble(text, &plan, &formatted_for(&plan)),
            "@set a=1\n<SELECT 1>"
        );
    }

    #[test]
    fn nothing_reason_names_the_blocker() {
        assert_eq!(plan("select 1 /* c */").nothing_reason(),
            "Nothing to format: every statement has a comment inside it, which the server's formatter would drop");
        assert_eq!(
            plan("select ${x}").nothing_reason(),
            "Nothing to format: every statement uses ${var} variables the server cannot parse"
        );
        assert_eq!(plan("  ").nothing_reason(), "Nothing to format");
    }

    #[test]
    fn literals_escape_quotes_backslashes_and_keep_newlines() {
        assert_eq!(
            quote_string("SELECT 'it\\'s'\n\\ x"),
            "'SELECT \\'it\\\\\\'s\\'\n\\\\ x'"
        );
    }

    #[test]
    fn long_or_multiline_errors_stay_one_status_line() {
        let error = zedb_ch::ChError::Server {
            code: Some(457),
            message: format!(
                "Code: 457. DB::Exception: Value SELECT\n    {}",
                "x".repeat(300)
            ),
        };
        let short = short_error(&error);
        assert_eq!(short, "Value SELECT");
        let error = zedb_ch::ChError::Server {
            code: Some(1),
            message: "y".repeat(300),
        };
        assert_eq!(short_error(&error).chars().count(), 121);
    }

    #[test]
    fn server_errors_are_cut_to_the_useful_part() {
        let error = zedb_ch::ChError::Server {
            code: Some(62),
            message:
                "Code: 62. DB::Exception: Syntax error: failed at position 18 (end of query): . \
                      Expected one of: expression with optional alias, lambda expression: In scope \
                      SELECT formatQuery('select from where'). (SYNTAX_ERROR)"
                    .into(),
        };
        assert_eq!(
            short_error(&error),
            "Syntax error: failed at position 18 (end of query)"
        );
    }
}
