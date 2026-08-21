# Integration tests

Cargo compiles each Rust file in this directory as a separate black-box test
crate. Tests here exercise public dependency or process boundaries and cannot
reach `zedb-app`'s private `Workspace` state because the app is a binary target.

Feature-level unit and security-policy tests stay close to their implementation
under `src/`. Larger focused suites use a neighboring `security_tests.rs` file
included only under `cfg(test)`. This keeps private policy private while making
the regression coverage easy to find.
