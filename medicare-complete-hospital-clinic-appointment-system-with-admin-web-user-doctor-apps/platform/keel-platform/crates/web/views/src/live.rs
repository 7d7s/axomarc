//! The live spine (D-6, F-10a / S-11): a stateful view model plus a
//! re-render — **no transport**.
//!
//! LF-032 deliberately excluded stateful components from the Laravel
//! parity claim. This spine is the instrument that lifts that exclusion
//! with evidence instead of with prose, and the exclusions are the point:
//!
//! * **opt-in per view.** A session exists only on a view whose binding
//!   declared itself live ([`crate::registry::ViewBinding::live`]). Every
//!   other view keeps rendering exactly as it did — the live loop is
//!   additive and cannot grow onto a page by accident.
//! * **the state IS the view model.** A session holds one object of the
//!   view's declared model type. There is no parallel state shape and no
//!   wire protocol: the application applies state changes (closures
//!   against the declared type), and the framework re-renders through the
//!   SAME schema-checked render the request path uses.
//! * **the revision is the state's, not the wire's.** Every applied
//!   change advances the session's revision; a re-render is stamped with
//!   the revision of the state it rendered. A re-render without a change
//!   between them returns the same revision again — nothing moved.
//!
//! Deliberately ABSENT until the transport slice (S-12): the wire
//! transport (over `kelson-realtime`), the diff of what changed, and the
//! stale-session refusal. None of those cases is claimed here; the spine
//! carries none of them.

use std::sync::Mutex;

use serde_json::json;

use crate::engine::ViewsEngine;
use crate::viewmodel::ViewModel;

/// One re-render of a live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRender {
    /// The view that was rendered.
    pub view: String,
    /// The rendered markup (the HTML plane).
    pub html: String,
    /// The state revision this render is stamped with: how many state
    /// changes the session had applied when it rendered.
    pub revision: u64,
}

/// One stateful session on a live view.
///
/// The session is the presentation state machine's middle: the template
/// knows how to render a state, the ViewModel owns the state and its
/// declaration, and the session is nothing more than one state object,
/// its revision, and the re-render that binds them.
pub struct LiveSession<S: ViewModel> {
    view: String,
    inner: Mutex<LiveInner<S>>,
}

/// The state object and its revision, under one lock. A re-render holds
/// the lock across the render, so a render never observes a half-applied
/// change and the revision it reports is exactly the state it rendered.
#[derive(Debug)]
struct LiveInner<S> {
    state: S,
    revision: u64,
}

impl<S: ViewModel> std::fmt::Debug for LiveSession<S> {
    /// The state is not part of the diagnostics: a view model is not a
    /// printed value, it is the contract the template renders from.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("view", &self.view)
            .field("revision", &self.inner.lock().unwrap().revision)
            .finish()
    }
}

impl<S: ViewModel> LiveSession<S> {
    /// Spawn a session on a live view. The engine front door
    /// ([`ViewsEngine::live_session`]) checks the opt-in; this
    /// constructor does not — it is the crate-internal primitive.
    pub(crate) fn spawn(view: impl Into<String>, state: S) -> Self {
        Self {
            view: view.into(),
            inner: Mutex::new(LiveInner { state, revision: 0 }),
        }
    }

    /// The view the session lives on.
    pub fn view_name(&self) -> &str {
        &self.view
    }

    /// Apply a state change: the closure mutates the current state, and
    /// the applied change advances the revision.
    ///
    /// A change is NOT a render: an application that moves several
    /// fields pays one re-render, not many. The re-render is explicit.
    pub fn change(&self, change: impl FnOnce(&mut S)) {
        let mut inner = self.inner.lock().unwrap();
        change(&mut inner.state);
        inner.revision += 1;
    }

    /// The state's revision: how many changes the session has applied.
    pub fn revision(&self) -> u64 {
        self.inner.lock().unwrap().revision
    }

    /// The state as its **checked** value: serialised and validated
    /// against the declaration — exactly what a bound render builds
    /// from. The transport slice's diff consumes this, revision-stamped.
    /// Hold the state lock, not a render: a changed-revision-without-
    /// re-render still reads the current declared state.
    pub fn state_value(&self) -> Result<serde_json::Value, crate::engine::ViewError> {
        let inner = self.inner.lock().unwrap();
        crate::viewmodel::view_model_value(&inner.state)
    }

    /// Re-render the session.
    ///
    /// The current state is serialised, checked against its declaration,
    /// bound into the `vm` namespace, and rendered through the binding's
    /// checks ([`ViewsEngine::render_bound`]) — the SAME checked render
    /// the request path uses, so a live state can never be a looser
    /// contract than a request-time one. The render is stamped with the
    /// revision of the state it rendered; a re-render without a change
    /// between them returns the same revision again.
    pub fn rerender(&self, engine: &ViewsEngine) -> Result<LiveRender, crate::engine::ViewError> {
        let inner = self.inner.lock().unwrap();
        let html = engine.render_bound(&self.view, json!({}), &inner.state)?;
        Ok(LiveRender {
            view: self.view.clone(),
            html,
            revision: inner.revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model the spine cases run against: one declared integer.
    #[derive(serde::Serialize)]
    struct MeterVm {
        depth: u64,
    }

    impl ViewModel for MeterVm {
        const VIEW: &'static str = "meter";
        fn schema() -> &'static [crate::viewmodel::FieldSchema] {
            crate::view_schema![crate::viewmodel::FieldSchema::required(
                "depth",
                crate::viewmodel::FieldKind::Int
            ),]
        }
    }

    #[test]
    fn a_change_advances_the_revision_without_a_render() {
        let session = LiveSession::spawn("meter", MeterVm { depth: 0 });
        assert_eq!(session.revision(), 0);

        session.change(|vm| vm.depth += 1);
        assert_eq!(session.revision(), 1, "one applied change, one revision");

        // A change is not a render: the revision is the state's, and the
        // re-render (the next case) is what turns it into markup.
        assert_eq!(session.view_name(), "meter");
    }
}
