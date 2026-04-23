//! # rustdec-plugin
//!
//! Lua plugin system for RustDec.
//!
//! ## Current state: stub
//!
//! This crate compiles and exposes the intended public API, but all
//! operations are no-ops.  No script is executed, no hook is fired.
//! The goal is to validate the build chain and reserve the interfaces
//! before the V2 implementation.
//!
//! ## Intended usage (V2)
//!
//! ```rust,ignore
//! let engine = PluginEngine::new()?;
//! engine.load_file("plugins/rename.lua")?;
//! engine.call_hook(Hook::OnFunctionLifted)?;
//! ```

mod hooks;

pub use hooks::Hook;

use thiserror::Error;
use tracing::debug;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("Lua error: {0}")]
    Lua(#[from] mlua::Error),

    #[error("Plugin file not found: {0}")]
    NotFound(String),

    #[error("Hook '{0}' returned an error: {1}")]
    HookFailed(String, String),
}

pub type PluginResult<T> = Result<T, PluginError>;

// ── PluginEngine ──────────────────────────────────────────────────────────────

/// Lua plugin engine.
///
/// Manages the Lua VM lifecycle and the hook registry.
/// V1 (stub): the VM is created but no script is loaded.
pub struct PluginEngine {
    /// Lua VM — initialised but sandbox is empty.
    lua: mlua::Lua,
    /// Number of loaded scripts (always 0 in stub).
    loaded: usize,
}

impl PluginEngine {
    /// Create a new engine with an empty Lua VM.
    pub fn new() -> PluginResult<Self> {
        let lua = mlua::Lua::new();
        debug!("PluginEngine: Lua VM initialised (stub — no scripts loaded)");
        Ok(Self { lua, loaded: 0 })
    }

    /// Load a Lua script from a file path.
    ///
    /// **Stub**: logs only, no script is executed.
    pub fn load_file(&mut self, path: &str) -> PluginResult<()> {
        debug!(path = %path, "PluginEngine: load_file called (stub — skipped)");
        // TODO V2: read the file and execute it inside the sandboxed VM.
        Ok(())
    }

    /// Load a Lua script from a string.
    ///
    /// **Stub**: logs only.
    pub fn load_string(&mut self, _src: &str, chunk_name: &str) -> PluginResult<()> {
        debug!(chunk = %chunk_name, "PluginEngine: load_string called (stub — skipped)");
        // TODO V2: execute the source inside the sandboxed VM.
        Ok(())
    }

    /// Fire a named hook.
    ///
    /// **Stub**: silent no-op.
    pub fn call_hook(&self, hook: Hook) -> PluginResult<()> {
        debug!(hook = ?hook, "PluginEngine: call_hook (stub — no-op)");
        // TODO V2: check whether a Lua handler is registered for this hook
        // and call it with the serialised arguments.
        Ok(())
    }

    /// Number of scripts currently loaded.
    pub fn loaded_count(&self) -> usize {
        self.loaded
    }
}

impl Default for PluginEngine {
    fn default() -> Self {
        Self::new().expect("failed to create Lua VM")
    }
}
