//! Free-text search across the cached schema, for the command palette.

use super::RecognizedKind;
use crate::schema_cache::SchemaSnapshot;

/// One `schema_search` match: a dotted path plus a short detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSearchHit {
    pub path: String,
    pub kind: RecognizedKind,
    pub detail: String,
}

/// Case-insensitive substring search over database, object, and column
/// names. Returns up to `limit` hits (databases, then objects, then
/// columns, each alphabetical) plus the total match count.
pub fn schema_search(
    snapshot: &SchemaSnapshot,
    query: &str,
    limit: usize,
) -> (Vec<SchemaSearchHit>, usize) {
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for database in snapshot.databases.values() {
        if database.name.to_lowercase().contains(&needle) {
            hits.push(SchemaSearchHit {
                path: database.name.clone(),
                kind: RecognizedKind::Database,
                detail: format!("{} objects", database.objects.len()),
            });
        }
        for object in database.objects.values() {
            if object.name.to_lowercase().contains(&needle) {
                hits.push(SchemaSearchHit {
                    path: format!("{}.{}", database.name, object.name),
                    kind: RecognizedKind::Object,
                    detail: object.engine.clone(),
                });
            }
            let Some(columns) = object.columns.as_ref() else {
                continue;
            };
            for column in columns.values() {
                if column.name.to_lowercase().contains(&needle) {
                    hits.push(SchemaSearchHit {
                        path: format!("{}.{}.{}", database.name, object.name, column.name),
                        kind: RecognizedKind::Column,
                        detail: column.type_name.clone(),
                    });
                }
            }
        }
    }
    hits.sort_by(|left, right| {
        let rank = |kind: RecognizedKind| match kind {
            RecognizedKind::Database => 0,
            RecognizedKind::Object => 1,
            RecognizedKind::Column => 2,
        };
        rank(left.kind)
            .cmp(&rank(right.kind))
            .then_with(|| left.path.cmp(&right.path))
    });
    let total = hits.len();
    hits.truncate(limit);
    (hits, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_intelligence::fixtures::{columns, snapshot};

    #[test]
    fn schema_search_matches_all_levels_and_caps() {
        let snapshot = snapshot(Some(columns()));
        let (hits, total) = schema_search(&snapshot, "event", 10);
        assert_eq!(total, 2);
        assert_eq!(hits[0].path, "analytics.events");
        assert_eq!(hits[1].path, "analytics.events.event_id");

        let (capped, total) = schema_search(&snapshot, "e", 1);
        assert_eq!(capped.len(), 1);
        assert!(total >= 2);
        assert!(schema_search(&snapshot, "zzz", 10).0.is_empty());
    }
}
