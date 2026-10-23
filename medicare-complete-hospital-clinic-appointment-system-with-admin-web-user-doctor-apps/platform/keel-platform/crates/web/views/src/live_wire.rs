//! The live wire (D-6, F-10b / S-12): the diff, the session registry and
//! the transport over `kelson-realtime` — the wire half of the live
//! loop.
//!
//! The spine ([`crate::live`]) holds state and re-renders through the
//! same checked render the request path uses. This module carries what
//! a browser talks to:
//!
//! * **the diff** ([`state_diff`]) is a leaf walk of the two serialised
//!   states: only the parts that changed travel, a removed leaf rides as
//!   `path -> null`, and a whole-subtree kind change is one entry, not a
//!   storm.
//! * **the registry** ([`LiveSessions`]) is the one place a session id
//!   resolves: a read for an id it does not host is **stale** and
//!   refused — a closed, expired or never-known session cannot serve
//!   its state again.
//! * **the wire** ([`LiveWire`], feature `live-transport`) publishes a
//!   session's diff to the hub: channel `global:live-<id>`, event
//!   `live.update`, payload `{ session, revision, changes }` (wire-safe
//!   JSON, no PHI — the hub's rule). The first publish for an id diffs
//!   against the empty state, so the subscriber gets every leaf; a
//!   publish with nothing changed carries nothing.
//!
//! Who may SUBSCRIBE is not this module's decision: channel
//! authorization is the runtime's job (the hub's rule), and a live id is
//! an opaque handle, not a credential.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
#[cfg(feature = "live-transport")]
use serde_json::json;

use crate::engine::{ViewError, ViewsEngine, VIEW_RENDER};
use crate::live::{LiveRender, LiveSession};
use crate::viewmodel::ViewModel;

// ---------------------------------------------------------------- the diff

/// One changed leaf: the dotted path from the view model's root, and the
/// value the path now carries. A removed leaf carries `Null`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffEntry {
    /// The dotted path the change happened at ("count", "rows.2.state").
    pub path: String,
    /// The value the path carries now (a removed leaf carries `Null`).
    pub after: Value,
}

/// The leaf walk of two serialised states: what changed, and **only**
/// that.
///
/// Objects and arrays walk element-wise; a whole-subtree kind change
/// (a number becoming an object, an array becoming a string, ...) is one
/// entry for the subtree. Nothing that did not change appears.
pub fn state_diff(before: &Value, after: &Value) -> Vec<DiffEntry> {
    let mut out = Vec::new();
    diff_walk(before, after, String::new(), &mut out);
    out
}

fn diff_walk(before: &Value, after: &Value, path: String, out: &mut Vec<DiffEntry>) {
    let child_path = |path: &str, key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };
    match (before, after) {
        (Value::Object(previous), Value::Object(current)) => {
            for (key, value) in current {
                let child = child_path(&path, key);
                match previous.get(key) {
                    Some(old) => diff_walk(old, value, child, out),
                    None => out.push(DiffEntry {
                        path: child,
                        after: value.clone(),
                    }),
                }
            }
            for (key, _) in previous {
                if !current.contains_key(key) {
                    out.push(DiffEntry {
                        path: child_path(&path, key),
                        after: Value::Null,
                    });
                }
            }
        }
        (Value::Array(previous), Value::Array(current)) => {
            let common = previous.len().min(current.len());
            for index in 0..common {
                diff_walk(
                    &previous[index],
                    &current[index],
                    format!("{path}.{index}"),
                    out,
                );
            }
            for (index, value) in current.iter().enumerate().skip(common) {
                out.push(DiffEntry {
                    path: format!("{path}.{index}"),
                    after: value.clone(),
                });
            }
            for index in common..previous.len() {
                out.push(DiffEntry {
                    path: format!("{path}.{index}"),
                    after: Value::Null,
                });
            }
        }
        _ => {
            if before != after {
                out.push(DiffEntry {
                    path,
                    after: after.clone(),
                });
            }
        }
    }
}

// ------------------------------------------------------------ the registry

/// A live session's opaque handle (`live-7`). Not a credential — who may
/// subscribe to a session's channel is the runtime's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LiveId(u64);

impl std::fmt::Display for LiveId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "live-{}", self.0)
    }
}

/// The read side of a live session, as the registry and the wire see it.
///
/// The application keeps the typed handle ([`LiveSession`]) and its
/// `change`; the erased surface serves value, revision and re-render —
/// the parts a browser-facing transport consumes. A session that rides
/// the registry is long-lived across threads, so its state must be
/// `Send`.
pub trait LiveNode: Send + Sync + std::fmt::Debug {
    /// The view the session lives on.
    fn view_name(&self) -> &str;
    /// The state's revision (how many changes the session applied).
    fn revision(&self) -> u64;
    /// The state as its checked value.
    fn state_value(&self) -> Result<Value, ViewError>;
    /// Re-render through the spine's checked render.
    fn rerender(&self, engine: &ViewsEngine) -> Result<LiveRender, ViewError>;
}

impl<S: ViewModel + Send + 'static> LiveNode for LiveSession<S> {
    fn view_name(&self) -> &str {
        LiveSession::view_name(self)
    }

    fn revision(&self) -> u64 {
        LiveSession::revision(self)
    }

    fn state_value(&self) -> Result<Value, ViewError> {
        LiveSession::state_value(self)
    }

    fn rerender(&self, engine: &ViewsEngine) -> Result<LiveRender, ViewError> {
        LiveSession::rerender(self, engine)
    }
}

/// The one place a live session resolves.
///
/// An entry exists while the session is served. After [`LiveSessions::close`]
/// the id is **stale**: every read of it is refused, because a registry
/// that no longer hosts a session has nothing to serve — serving a
/// remembered state after the session went away is the leak the
/// stale-refusal closes.
#[derive(Debug, Default)]
pub struct LiveSessions {
    seq: AtomicU64,
    slots: Mutex<HashMap<LiveId, Arc<dyn LiveNode>>>,
}

impl LiveSessions {
    /// A registry with no hosted sessions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Host a session; the returned id is its handle **for its lifetime
    /// on this registry**.
    pub fn push<N: LiveNode + 'static>(&self, node: Arc<N>) -> LiveId {
        let id = LiveId(self.seq.fetch_add(1, Ordering::SeqCst));
        self.slots.lock().unwrap().insert(id, node);
        id
    }

    /// The session an id names — while the registry still hosts it. An
    /// unhosted id (never known, or closed) is **stale** and refused.
    pub fn get(&self, id: &LiveId) -> Result<Arc<dyn LiveNode>, ViewError> {
        self.slots
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| {
                ViewError::new(
                    VIEW_RENDER,
                    format!("the live session {id} is stale: the registry no longer hosts it"),
                )
            })
    }

    /// Retire a session: the id goes stale and the wire refuses it.
    /// Closing an already-stale id changes nothing.
    pub fn close(&self, id: &LiveId) -> bool {
        self.slots.lock().unwrap().remove(id).is_some()
    }

    /// How many sessions the registry hosts.
    pub fn len(&self) -> usize {
        self.slots.lock().unwrap().len()
    }

    /// Whether nothing is hosted.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// --------------------------------------------------------------- the wire

/// The wire half of the live loop (`feature = "live-transport"`): a
/// session's diffs, published over the realtime hub.
///
/// The wire does not invent a protocol: it rides the hub's channels
/// (`global:live-<id>`, event `live.update`), and the hub's own rule —
/// wire-safe JSON, no PHI, authorization decided by the runtime — stays
/// the rule.
#[cfg(feature = "live-transport")]
pub struct LiveWire {
    hub: kelson_realtime::BroadcastHub,
    engine: Arc<ViewsEngine>,
    sessions: Arc<LiveSessions>,
    published: Mutex<HashMap<LiveId, Value>>,
}

#[cfg(feature = "live-transport")]
impl LiveWire {
    /// A wire over one hub, rendering through one engine, resolving ids
    /// through one registry.
    pub fn new(
        hub: kelson_realtime::BroadcastHub,
        engine: Arc<ViewsEngine>,
        sessions: Arc<LiveSessions>,
    ) -> Self {
        Self {
            hub,
            engine,
            sessions,
            published: Mutex::new(HashMap::new()),
        }
    }

    /// The channel a session's diffs travel on: `global:live-<id>`.
    pub fn channel(&self, id: &LiveId) -> String {
        kelson_realtime::global_channel(&id.to_string())
    }

    /// Render the session (through the spine's checked render), diff
    /// the checked state against the last published value, and carry
    /// **only the changed parts** on the wire.
    ///
    /// The first publish for an id diffs against the empty state, so the
    /// subscriber starts from every leaf. A publish whose state did not
    /// change carries nothing ([`None`]) and puts no event on the hub.
    /// A stale id is refused before a render is paid for.
    pub fn publish(
        &self,
        id: &LiveId,
        node: &dyn LiveNode,
    ) -> Result<Option<Vec<DiffEntry>>, ViewError> {
        self.sessions.get(id)?;
        let render = node.rerender(&self.engine)?;
        let after = node.state_value()?;
        let mut published = self.published.lock().unwrap();
        let before = published.get(id).cloned().unwrap_or_else(|| json!({}));
        let diff = state_diff(&before, &after);
        published.insert(*id, after);
        drop(published);
        if diff.is_empty() {
            return Ok(None);
        }
        self.hub.publish(kelson_realtime::RealtimeEvent {
            channel: self.channel(id),
            event: String::from("live.update"),
            payload: json!({
                "session": id.to_string(),
                "revision": render.revision,
                "changes": diff,
            }),
            at: time::OffsetDateTime::now_utc(),
        });
        Ok(Some(diff))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_diff_carries_only_the_changed_parts() {
        let before = json!({
            "count": 7,
            "title": "same",
            "nested": { "a": 1, "b": 2 },
            "items": [1, 2, 3],
        });

        // A nested leaf and only it: the siblings that match travel
        // nowhere.
        let after = json!({
            "count": 7,
            "title": "same",
            "nested": { "a": 1, "b": 9 },
            "items": [1, 2, 3],
        });
        let diff = state_diff(&before, &after);
        assert_eq!(
            diff.as_slice(),
            &[DiffEntry {
                path: "nested.b".to_string(),
                after: json!(9),
            }],
            "only the changed part travels: {diff:?}"
        );

        // A removed leaf rides as null; an added leaf rides its value.
        let after = json!({
            "count": 7,
            "title": "same",
            "nested": { "a": 1 },
            "items": [1, 2, 3],
            "added": true,
        });
        let diff = state_diff(&before, &after);
        assert_eq!(diff.len(), 2, "{diff:?}");
        assert!(diff.contains(&DiffEntry {
            path: "nested.b".to_string(),
            after: Value::Null,
        }));
        assert!(diff.contains(&DiffEntry {
            path: "added".to_string(),
            after: json!(true),
        }));

        // A whole-subtree kind change is one entry, not a walk of the
        // new shape.
        let after = json!({
            "count": 7,
            "title": "same",
            "nested": "flattened",
            "items": [1, 2, 3],
        });
        let diff = state_diff(&before, &after);
        assert_eq!(
            diff.as_slice(),
            &[DiffEntry {
                path: "nested".to_string(),
                after: json!("flattened"),
            }]
        );

        // Nothing moved: nothing travels.
        assert!(
            state_diff(&before, &before).is_empty(),
            "an unchanged state diffs to nothing"
        );
    }

    // ------------------------------------------------------------ the registry

    /// The model the registry cases hold behind the erased surface.
    #[derive(serde::Serialize)]
    struct CountVm {
        count: u64,
    }

    impl crate::viewmodel::ViewModel for CountVm {
        const VIEW: &'static str = "counter";
        fn schema() -> &'static [crate::viewmodel::FieldSchema] {
            crate::view_schema![
                crate::viewmodel::FieldSchema::required("count", crate::viewmodel::FieldKind::Int),
            ]
        }
    }

    #[test]
    fn a_stale_session_is_refused() {
        let sessions = LiveSessions::new();
        assert!(sessions.is_empty());
        let session = Arc::new(LiveSession::spawn("counter", CountVm { count: 0 }));

        let id = sessions.push(session.clone());
        assert_eq!(sessions.len(), 1);
        let hosted = sessions.get(&id).expect("the registry hosts it");
        assert_eq!(hosted.view_name(), "counter");
        assert_eq!(hosted.revision(), 0);

        // After close the id is stale: the session cannot serve its
        // state again.
        assert!(sessions.close(&id), "the hosted id goes stale");
        let error = sessions.get(&id).expect_err("a closed id is stale");
        assert_eq!(error.code, VIEW_RENDER);
        assert!(error.message.contains("stale"), "{}", error.message);
        assert!(
            !sessions.close(&id),
            "closing an already-stale id changes nothing"
        );

        // An id the registry never hosted is stale too.
        let error = sessions.get(&LiveId(999)).expect_err("unknown is stale");
        assert_eq!(error.code, VIEW_RENDER);
        assert!(
            error.message.contains("stale"),
            "the refusal does not distinguish the two stales: {}",
            error.message
        );
    }
}

// -------------------------------------------------------- the wire cases

#[cfg(all(test, feature = "live-transport"))]
mod wire_tests {
    use super::*;
    use std::path::PathBuf;

    use crate::engine::{ViewsConfig, ViewsEngine};

    /// The model the wire cases publish.
    #[derive(serde::Serialize)]
    struct WireVm {
        count: u64,
    }

    impl crate::viewmodel::ViewModel for WireVm {
        const VIEW: &'static str = "wire";
        fn schema() -> &'static [crate::viewmodel::FieldSchema] {
            crate::view_schema![
                crate::viewmodel::FieldSchema::required("count", crate::viewmodel::FieldKind::Int),
            ]
        }
    }

    /// An engine whose "wire" view is a LIVE binding of the wire model.
    fn live_engine(template: &str) -> (PathBuf, Arc<ViewsEngine>) {
        let dir = std::env::temp_dir().join(format!(
            "kelson-views-wire-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos()
        ));
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(&views).unwrap();
        let path = views.join("wire.html");
        std::fs::write(&path, template).unwrap();
        let engine = ViewsEngine::new(ViewsConfig {
            paths: vec![views],
            cache_dir: Some(dir.join("storage").join("framework").join("views")),
            env: "testing".into(),
        });
        let mut registry = crate::registry::ViewRegistry::new();
        registry.register(crate::registry::ViewBinding::new::<WireVm>("wire").live());
        engine.set_registry(Arc::new(registry));
        (dir, Arc::new(engine))
    }

    #[test]
    fn the_wire_carries_only_the_changed_parts() {
        let (dir, engine) = live_engine("@vm<count>{{ vm.count }}</count>");
        let hub = kelson_realtime::BroadcastHub::new();
        let sessions = Arc::new(LiveSessions::new());
        let wire = LiveWire::new(hub.clone(), engine.clone(), sessions.clone());

        let session = engine
            .live_session("wire", WireVm { count: 0 })
            .expect("the view declared itself live");
        let session = Arc::new(session);
        let id = sessions.push(session.clone());
        let mut receiver = hub.subscribe(&wire.channel(&id));

        // The FIRST publish diffs against the empty state: the
        // subscriber starts from every leaf.
        let first = wire
            .publish(&id, session.as_ref())
            .expect("the publish renders and diffs")
            .expect("the first publish carries the state");
        assert_eq!(
            first.as_slice(),
            &[DiffEntry {
                path: "count".to_string(),
                after: json!(0),
            }]
        );
        let event = receiver
            .try_recv()
            .expect("the hub put the first publish on the channel");
        assert_eq!(event.channel, wire.channel(&id));
        assert_eq!(event.event, "live.update");
        assert_eq!(event.payload["session"], json!(id.to_string()));
        assert_eq!(event.payload["revision"], json!(0));
        assert_eq!(
            event.payload["changes"],
            json!([{ "path": "count", "after": 0 }])
        );

        // A publish with nothing changed carries nothing.
        let quiet = wire.publish(&id, session.as_ref()).expect("the publish diffs");
        assert!(quiet.is_none(), "no change, no event: {quiet:?}");
        assert!(
            receiver.try_recv().is_err(),
            "the subscriber receives no event for no change"
        );

        // One change travels as exactly the leaf that changed.
        session.change(|vm| vm.count += 5);
        let changed = wire
            .publish(&id, session.as_ref())
            .expect("the publish renders and diffs")
            .expect("a change carries a diff");
        assert_eq!(
            changed.as_slice(),
            &[DiffEntry {
                path: "count".to_string(),
                after: json!(5),
            }]
        );
        let event = receiver
            .try_recv()
            .expect("the hub put the change on the channel");
        assert_eq!(event.payload["revision"], json!(1));
        assert_eq!(
            event.payload["changes"],
            json!([{ "path": "count", "after": 5 }])
        );

        drop(dir);
    }

    #[test]
    fn the_wire_refuses_to_publish_a_stale_session() {
        let (dir, engine) = live_engine("@vm<count>{{ vm.count }}</count>");
        let hub = kelson_realtime::BroadcastHub::new();
        let sessions = Arc::new(LiveSessions::new());
        let wire = LiveWire::new(hub.clone(), engine.clone(), sessions.clone());

        let session = engine
            .live_session("wire", WireVm { count: 0 })
            .expect("the view declared itself live");
        let session = Arc::new(session);
        let id = sessions.push(session.clone());
        let mut receiver = hub.subscribe(&wire.channel(&id));

        sessions.close(&id);
        let error = wire
            .publish(&id, session.as_ref())
            .expect_err("the wire refuses a session the registry does not host");
        assert_eq!(error.code, VIEW_RENDER);
        assert!(error.message.contains("stale"), "{}", error.message);
        assert!(
            receiver.try_recv().is_err(),
            "the refusal puts nothing on the wire"
        );

        drop(dir);
    }
}
