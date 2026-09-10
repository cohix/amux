//! Internal `SandboxBackend` trait — NOT pub outside `src/engine/sandbox/`.
//!
//! The sandbox-paradigm analogue of the container tier's `ContainerBackend`.
//! Covers only what `SandboxRuntime` calls today: lifecycle (stop),
//! discovery (list/stats) and identity (name/cli_binary). Sandbox creation
//! and exec go through the interactive launch path
//! (`dsbx::run_interactive`), not this trait.
//!
//! Implementations: `dsbx::DSbxBackend`.

use crate::engine::agent_runtime::{AgentHandle, AgentStats};
use crate::engine::error::EngineError;

/// What every sandbox backend must support. The concrete type is hidden
/// behind `Arc<dyn SandboxBackend>` and never escapes this module.
pub(super) trait SandboxBackend: Send + Sync {
    /// Stop a running sandbox (preserve the persistent volume).
    fn stop(&self, handle: &AgentHandle) -> Result<(), EngineError>;

    /// Enumerate handles for running awman sandboxes.
    fn list_running(&self) -> Result<Vec<AgentHandle>, EngineError>;

    /// Per-handle resource stats. Sandbox-class runtimes can't provide
    /// per-resource metrics today; implementations return zeros.
    fn stats(&self, handle: &AgentHandle) -> Result<AgentStats, EngineError>;

    /// Static name used by `SandboxRuntime::runtime_name`.
    fn name(&self) -> &'static str;

    /// CLI binary for this backend (`sbx`).
    fn cli_binary(&self) -> &'static str;
}
