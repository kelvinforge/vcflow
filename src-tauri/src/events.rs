/// Tauri Events are notify-only: the frontend never treats an event as a
/// substitute for calling a Command, it just reacts to state that already
/// changed. `workflow:state:changed` is the smoke-test event proving the
/// Rust -> frontend event channel works end to end.
/// Tauri event names allow only alphanumeric, `-`, `/`, `:`, `_` -- no dots.
pub const WORKFLOW_STATE_CHANGED: &str = "workflow:state:changed";

/// Emitted step-by-step during `initialize_workflow` so the Setup Card shows
/// live operation text ("Saving your work…", "Creating the develop branch…")
/// instead of a bare spinner. Payload: `{ "step": String }`.
pub const WORKFLOW_INIT_STEP: &str = "workflow:init:step";
