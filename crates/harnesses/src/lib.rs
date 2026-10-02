//! `pij-harnesses` — per-harness discovery, binding, readiness, busy-state,
//! and interaction-safety quirks. Drift belongs here, once per harness.

mod adapter;
mod claude_settings;
mod composer;
mod interaction;
mod models;
mod spawn;

pub mod proc;

pub use adapter::{
    BindObservation, ClaudeHarness, CodexHarness, CopilotHarness, HarnessRegistry,
    ObservedBindFacts, OmpHarness, PiHarness,
};
pub use claude_settings::{
    ClaudeHookState, ClaudeInboundState, ClaudeStatuslineState, EnsurePlan, EnsureReport,
    claude_homes, claude_homes_from, copilot_home, ensure_claude_inbound_accept,
    ensure_claude_session_start_hook, ensure_claude_statusline, ensure_claude_stop_failure_hook,
    ensure_claude_stop_hook, ensure_claude_user_prompt_submit_hook, ensure_copilot_statusline,
    inspect_claude_inbound, inspect_claude_session_start_hook, inspect_claude_statusline,
    inspect_claude_stop_failure_hook, inspect_claude_stop_hook,
    inspect_claude_user_prompt_submit_hook, install_claude_session_start_script,
    install_claude_statusline_script, install_claude_stop_script,
    install_claude_user_prompt_submit_script, install_copilot_statusline_script,
    plan_claude_session_start_hook, plan_claude_statusline, plan_claude_stop_failure_hook,
    plan_claude_stop_hook, plan_claude_user_prompt_submit_hook, plan_inbound_accept,
};
pub use composer::{ComposerRegion, composer_region};
pub use interaction::{InjectionVerdict, InteractionGate, StagedSubmission};
pub use models::ModelCatalog;
pub use spawn::{
    SpawnPlan, SpawnPlanInput, build_spawn_plan, observed_launch_command,
    validate_executable_override,
};
