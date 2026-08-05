use std::collections::BTreeMap;

use super::RepoError;

/// Parameters every repo has without declaring them.
pub const BUILTIN_PARAMS: [&str; 2] = ["db", "cluster"];

/// The `${name}` placeholders appearing in `sql`, in order of first use.
pub fn placeholders(sql: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = sql;
    while let Some(start) = rest.find("${") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find('}') else { break };
        let name = &rest[..end];
        if !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !names.iter().any(|seen| seen == name)
        {
            names.push(name.to_string());
        }
        rest = &rest[end + 1..];
    }
    names
}

/// Placeholders in `sql` that are not in `declared`.
pub fn undeclared_placeholders(sql: &str, declared: &[&str]) -> Vec<String> {
    placeholders(sql)
        .into_iter()
        .filter(|name| !declared.contains(&name.as_str()))
        .collect()
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Render `${name}` placeholders with the given values.
///
/// `db` and `cluster` land in identifier position in DDL and ClickHouse
/// cannot parameterize identifiers, so their values must be plain
/// identifier chunks; other parameters are expression-position and render
/// verbatim. Every placeholder must have a value.
pub fn render(sql: &str, values: &BTreeMap<String, String>) -> Result<String, RepoError> {
    let mut rendered = sql.to_string();
    for name in placeholders(sql) {
        let value = values
            .get(&name)
            .ok_or_else(|| RepoError::MissingParam(name.clone()))?;
        if BUILTIN_PARAMS.contains(&name.as_str()) && !is_identifier(value) {
            return Err(RepoError::InvalidIdentifier {
                name,
                value: value.clone(),
            });
        }
        rendered = rendered.replace(&format!("${{{name}}}"), value);
    }
    Ok(rendered)
}
