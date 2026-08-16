mod actions;
#[path = "controller/execution.rs"]
mod controller_execution;
#[path = "controller/repo_picker.rs"]
mod controller_repo_picker;
#[path = "controller/verification.rs"]
mod controller_verification;
mod view;

pub(crate) use view::FleetState;
