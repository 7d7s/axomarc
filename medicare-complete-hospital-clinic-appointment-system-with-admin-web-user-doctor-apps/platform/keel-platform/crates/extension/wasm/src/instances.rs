//! A second instance of a generation that is already compiled.
//!
//! [`crate::WasmHost::prepare`] does two things with very different costs.
//! Compiling the artifact dominates — cranelift over a release guest takes
//! seconds, and a debug-built host pays tens of seconds for the same bytes —
//! while everything that distinguishes one *instance* of a generation from
//! another (`services`, `in_flight`, `state`) is a cell.
//!
//! Splitting the two is what lets a process compile a module once and serve
//! it from many hosts that each hold different stores. The integration
//! harness is the case that needs it: it builds one `AppState` per test, and
//! each of those needs plugins wired to *its* stores, not to whichever
//! stores an earlier test happened to install first.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use std::sync::OnceLock;

use crate::Guest;
use crate::PluginState;
use crate::WasmPlugin;

impl Clone for Guest {
    fn clone(&self) -> Self {
        match self {
            // `InstancePre` is an Arc-typed handle over the engine's compiled
            // code, so this copies a pointer, not the module.
            Guest::V1(pre) => Guest::V1(pre.clone()),
            #[cfg(feature = "component-model")]
            Guest::V2(pre) => Guest::V2(pre.clone()),
        }
    }
}

impl WasmPlugin {
    /// A fresh instance of this same compiled generation.
    ///
    /// Shares the compiled code, the validated manifest, the declared
    /// capability set and the resolved sandbox budget with `self`, and owns
    /// nothing mutable that `self` can observe: its in-flight counter starts
    /// at zero, its services cell starts empty, and it starts at
    /// [`PluginState::Prepared`].
    ///
    /// That last part is deliberate. This instance has not been committed, so
    /// the caller runs the same `commit` the boot path runs rather than
    /// handing out a plugin that skipped the `prepare -> commit` step of ADR
    /// 0019 6. An uncommitted plugin cannot dispatch — which is the correct
    /// answer for a caller that has not yet installed its stores — instead of
    /// silently serving the stores of the instance it was copied from.
    #[must_use]
    pub fn reinstantiate(&self) -> Self {
        WasmPlugin {
            engine: self.engine.clone(),
            guest: self.guest.clone(),
            manifest: self.manifest.clone(),
            declared: Arc::clone(&self.declared),
            label: self.label.clone(),
            budget: self.budget,
            // The same ticker serves both instances: it is a process-wide
            // epoch counter, and the handle only has to stay alive, which
            // either instance does.
            ticker: self.ticker.clone(),
            in_flight: AtomicU32::new(0),
            services: OnceLock::new(),
            state: AtomicU8::new(PluginState::Prepared as u8),
        }
    }
}
