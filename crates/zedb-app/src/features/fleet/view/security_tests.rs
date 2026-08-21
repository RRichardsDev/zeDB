use super::*;

fn pending() -> PendingFleetAction {
    PendingFleetAction {
        action: FleetAction::UpgradeAll,
        connection: "production".into(),
        repo_root: "/repos/expected".into(),
    }
}

#[test]
fn fleet_confirmation_requires_the_reviewed_context() {
    let pending = pending();
    let valid = |connection, repo, unlocked| FleetConfirmation {
        pending: &pending,
        current_connection: connection,
        current_repo: repo,
        write_unlocked: unlocked,
        running: false,
        completed: false,
        structural: false,
        acknowledged: false,
        required_phrase: None,
        typed_phrase: "",
    };

    assert!(fleet_confirmation_valid(&valid(
        Some("production"),
        Some(std::path::Path::new("/repos/expected")),
        true,
    )));
    assert!(!fleet_confirmation_valid(&valid(
        Some("staging"),
        Some(std::path::Path::new("/repos/expected")),
        true,
    )));
    assert!(!fleet_confirmation_valid(&valid(
        Some("production"),
        Some(std::path::Path::new("/repos/other")),
        true,
    )));
    assert!(!fleet_confirmation_valid(&valid(
        Some("production"),
        Some(std::path::Path::new("/repos/expected")),
        false,
    )));
}

#[test]
fn fleet_confirmation_rechecks_every_consent_gate() {
    let pending = pending();
    let check = |running, completed, acknowledged, typed_phrase| FleetConfirmation {
        pending: &pending,
        current_connection: Some("production"),
        current_repo: Some(std::path::Path::new("/repos/expected")),
        write_unlocked: true,
        running,
        completed,
        structural: true,
        acknowledged,
        required_phrase: Some("all"),
        typed_phrase,
    };

    assert!(fleet_confirmation_valid(&check(false, false, true, "all")));
    assert!(!fleet_confirmation_valid(&check(true, false, true, "all")));
    assert!(!fleet_confirmation_valid(&check(false, true, true, "all")));
    assert!(!fleet_confirmation_valid(&check(
        false, false, false, "all"
    )));
    assert!(!fleet_confirmation_valid(&check(
        false, false, true, "wrong"
    )));
}

#[test]
fn fleet_action_phrases_fail_closed_for_production_and_irreversible_work() {
    let action = FleetAction::UpgradeDatabase("analytics".into());
    assert_eq!(
        action.required_phrase(zedb_core::EnvTier::Production, false),
        Some("analytics".into())
    );
    assert_eq!(
        action.required_phrase(zedb_core::EnvTier::Dev, true),
        Some("irreversible".into())
    );
    assert_eq!(action.required_phrase(zedb_core::EnvTier::Dev, false), None);
}
