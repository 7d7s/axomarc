//! The view registry: which view model a view binds to.
//!
//! A **view** in Kelson is not a loose template file: it is a template
//! **plus** the view model it declares. The registry is where that
//! declaration lives, so the framework can answer "what may this view
//! read?" without asking the application.
//!
//! Two rules keep the registry honest:
//!
//! * **a binding is optional.** A view whose template exists without a
//!   binding still renders, exactly as it did before the registry
//!   existed. The registry sharpens what the framework knows; it never
//!   takes a working page away.
//! * **the last registration wins.** An application registers its own
//!   view after the framework registered the built-in, and the
//!   application's binding is the one that applies.

use std::collections::BTreeMap;

use crate::viewmodel::{FieldSchema, ViewModel};

/// What the framework knows about one view.
#[derive(Debug, Clone)]
pub struct ViewBinding {
    /// The view's name ("dashboard.ops").
    pub view: String,
    /// The view model's type name, for diagnostics.
    pub model: String,
    /// The view's declared data requirements.
    pub schema: &'static [FieldSchema],
    /// Whether the view opted into a **stateful session** (the live
    /// spine, D-6). Default: `false` — the live loop is opt-in per view,
    /// so the LF-032 exclusion of stateful components lifts one view at
    /// a time and every other view keeps rendering exactly as it did.
    pub live: bool,
}

impl ViewBinding {
    /// Bind a view to a view model.
    ///
    /// The model's declared schema comes with the binding, so the
    /// framework never has to ask the application what the view may read.
    pub fn new<M: ViewModel>(view: impl Into<String>) -> Self {
        Self {
            view: view.into(),
            model: std::any::type_name::<M>().to_string(),
            schema: M::schema(),
            live: false,
        }
    }

    /// Declare the view **live**: it may host a stateful session whose
    /// state is its declared view model. A view that did not declare
    /// this refuses the session at spawn — the stateful loop never
    /// grows onto a page by accident.
    pub fn live(mut self) -> Self {
        self.live = true;
        self
    }

    /// Whether the view declared itself live.
    pub fn is_live(&self) -> bool {
        self.live
    }
}

/// The registry: the views and the view models they bind to.
#[derive(Debug, Clone, Default)]
pub struct ViewRegistry {
    bindings: BTreeMap<String, ViewBinding>,
}

impl ViewRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a binding. The last registration for a view wins, so an
    /// application overrides a built-in by registering after it.
    pub fn register(&mut self, binding: ViewBinding) {
        self.bindings.insert(binding.view.clone(), binding);
    }

    /// The binding for a view, if it has one.
    pub fn binding(&self, view: &str) -> Option<&ViewBinding> {
        self.bindings.get(view)
    }

    /// The registered view names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.bindings.keys().cloned().collect()
    }

    /// How many views are bound.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Whether nothing is bound.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

// --------------------------------------------------------------- commands

/// One declared command: the route that serves an operation a template
/// may bind to (`@command("op")`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRoute {
    /// The operation id the route dispatches ("cancel").
    pub op: String,
    /// The HTTP verb the route declares ("POST").
    pub verb: String,
    /// The path the route declares ("/api/v1/appointments").
    pub path: String,
}

impl CommandRoute {
    /// Declare one command: an operation bound to the route that serves it.
    pub fn new(op: impl Into<String>, verb: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            op: op.into(),
            verb: verb.into(),
            path: path.into(),
        }
    }
}

/// The command registry: which operations a template may bind to, and
/// the route each one is served by.
///
/// Nothing is free-form: `@command("op")` renders **only** an operation a
/// route has declared here, on the verb and path that route declared. A
/// command the registry does not know is a render error, so a template can
/// never invent an endpoint. Two rules, mirroring the view registry:
///
/// * **the registry is optional.** An engine without one renders views
///   without commands exactly as before; `@command` is simply refused.
/// * **the last registration wins.** Two routes declaring the same
///   operation resolve to the later declaration — the framework binds to
///   what the deployment says the operation is served by now.
#[derive(Debug, Clone, Default)]
pub struct CommandRegistry {
    routes: BTreeMap<String, CommandRoute>,
}

impl CommandRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command route. The last registration for an operation
    /// wins.
    pub fn register(&mut self, route: CommandRoute) {
        self.routes.insert(route.op.clone(), route);
    }

    /// The route that serves an operation, if any is registered.
    pub fn route(&self, op: &str) -> Option<&CommandRoute> {
        self.routes.get(op)
    }

    /// The registered operation ids, sorted.
    pub fn ops(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }

    /// How many operations are declared.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether nothing is declared.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewmodel::{FieldKind, FieldSchema};
    use serde::Serialize;

    /// A view model with one declared field.
    #[derive(Serialize)]
    struct OpsVm {
        depth: u64,
    }

    impl ViewModel for OpsVm {
        const VIEW: &'static str = "dashboard.ops";
        fn schema() -> &'static [FieldSchema] {
            crate::view_schema![FieldSchema::required("depth", FieldKind::Int)]
        }
    }

    /// Another model, for the override case.
    #[derive(Serialize)]
    struct OtherVm {
        depth: u64,
    }

    impl ViewModel for OtherVm {
        const VIEW: &'static str = "dashboard.ops";
        fn schema() -> &'static [FieldSchema] {
            crate::view_schema![FieldSchema::required("depth", FieldKind::Int)]
        }
    }

    #[test]
    fn a_registered_view_declares_its_model() {
        let mut registry = ViewRegistry::new();
        registry.register(ViewBinding::new::<OpsVm>("dashboard.ops"));

        let binding = registry
            .binding("dashboard.ops")
            .expect("the view is bound");
        assert_eq!(binding.view, "dashboard.ops");
        assert!(
            binding.model.contains("OpsVm"),
            "the binding names the model: {}",
            binding.model
        );
        assert_eq!(binding.schema.len(), 1);
        assert_eq!(binding.schema[0].path, "depth");
        assert_eq!(registry.names(), vec!["dashboard.ops".to_string()]);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn an_unregistered_view_has_no_binding() {
        let registry = ViewRegistry::new();
        assert!(registry.is_empty());
        assert!(
            registry.binding("dashboard.ops").is_none(),
            "an unbound view has no binding - and still renders, unchanged"
        );
        assert!(registry.names().is_empty());
    }

    #[test]
    fn a_registration_replaces_the_previous_one() {
        let mut registry = ViewRegistry::new();
        registry.register(ViewBinding::new::<OpsVm>("dashboard.ops"));
        registry.register(ViewBinding::new::<OtherVm>("dashboard.ops"));
        assert_eq!(
            registry.len(),
            1,
            "the last registration for a view wins"
        );
        assert!(
            registry
                .binding("dashboard.ops")
                .unwrap()
                .model
                .contains("OtherVm"),
            "the application's binding is the one that applies"
        );
    }

    #[test]
    fn a_declared_command_names_its_operation_and_route() {
        let mut registry = CommandRegistry::new();
        registry.register(CommandRoute::new("cancel", "POST", "/appointments/cancel"));

        let route = registry.route("cancel").expect("the operation is declared");
        assert_eq!(route.verb, "POST");
        assert_eq!(route.path, "/appointments/cancel");
        assert_eq!(registry.ops(), vec!["cancel".to_string()]);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn an_undeclared_operation_has_no_command_route() {
        let registry = CommandRegistry::new();
        assert!(registry.is_empty());
        assert!(
            registry.route("cancel").is_none(),
            "an operation no route declared cannot be bound - there is nothing to dispatch"
        );
        assert!(registry.ops().is_empty());
    }

    #[test]
    fn a_command_registration_replaces_the_previous_route() {
        let mut registry = CommandRegistry::new();
        registry.register(CommandRoute::new("cancel", "DELETE", "/appointments/{id}"));
        registry.register(CommandRoute::new("cancel", "POST", "/appointments/cancel"));
        assert_eq!(registry.len(), 1, "the last registration for an op wins");
        assert_eq!(
            registry.route("cancel").map(|r| r.verb.clone()),
            Some("POST".to_string()),
            "the later route declaration is the one the command binds to"
        );
    }

    #[test]
    fn a_binding_declares_itself_live() {
        let default = ViewBinding::new::<OpsVm>("counter");
        assert!(
            !default.is_live(),
            "the live loop is opt-in: a plain binding never hosts a stateful session"
        );

        let live = ViewBinding::new::<OpsVm>("counter").live();
        assert!(
            live.is_live(),
            "the declaration is the opt-in: the declared view may host a session"
        );
    }
}
