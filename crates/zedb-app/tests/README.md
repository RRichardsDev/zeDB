# Integration tests

Cargo compiles each Rust file in this directory as a separate black-box test
crate. zedb-app is a library plus a thin `zedb` binary (`main.rs` only calls
`zedb_app::run()`), so tests here can link `zedb_app` but still see only its
public surface; `Workspace` state stays crate-private on purpose.

Feature-level unit and security-policy tests stay close to their implementation
under `src/`. Larger focused suites use a neighboring `security_tests.rs` file
included only under `cfg(test)`. This keeps private policy private while making
the regression coverage easy to find.

## Window tests

`#[gpui::test]` window tests also live under `src/`, in a `gpui_tests.rs`
file next to the feature they exercise (they drive crate-private Workspace
state, so they stay in-crate rather than here); `src/test_harness.rs` holds
the shared scaffolding. They run the
real Workspace in a headless test window on gpui's deterministic executor:
entities, focus, key dispatch, and render all behave as in the app, so flows
like the fleet confirmation ladder are tested end to end (simulated typing
included) without a signed build or a display. `Workspace::new_for_test`
builds the state without `new`'s launch side effects, so tests never touch
preferences or session files, the Keychain, or the network. They run with
plain `cargo test -p zedb-app`.
