use super::*;

fn pending() -> PendingApply {
    PendingApply {
        index: 2,
        apply: vec!["ALTER TABLE analytics.events MODIFY COLUMN value UInt64".into()],
        connection: "staging".into(),
        database: "analytics".into(),
        object: "events".into(),
    }
}

#[test]
fn apply_in_place_requires_a_known_non_production_writable_tier() {
    assert!(apply_in_place_allowed(false, Some(zedb_core::EnvTier::Dev)));
    assert!(apply_in_place_allowed(
        false,
        Some(zedb_core::EnvTier::Staging)
    ));
    assert!(!apply_in_place_allowed(
        false,
        Some(zedb_core::EnvTier::Production)
    ));
    assert!(!apply_in_place_allowed(
        true,
        Some(zedb_core::EnvTier::Staging)
    ));
    assert!(!apply_in_place_allowed(false, None));
}

#[test]
fn pending_apply_is_bound_to_connection_and_table() {
    let pending = pending();
    assert!(pending.matches_context(
        Some(("staging", false)),
        Some(zedb_core::EnvTier::Staging),
        Some(("analytics", "events")),
    ));
    assert!(!pending.matches_context(
        Some(("production", false)),
        Some(zedb_core::EnvTier::Staging),
        Some(("analytics", "events")),
    ));
    assert!(!pending.matches_context(
        Some(("staging", false)),
        Some(zedb_core::EnvTier::Staging),
        Some(("analytics", "other")),
    ));
    assert!(!pending.matches_context(
        Some(("staging", false)),
        Some(zedb_core::EnvTier::Production),
        Some(("analytics", "events")),
    ));
}
