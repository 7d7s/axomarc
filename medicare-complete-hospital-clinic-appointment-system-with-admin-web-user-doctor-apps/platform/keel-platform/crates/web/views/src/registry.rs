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
        }
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
}
