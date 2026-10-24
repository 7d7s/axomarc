//! View-model bridges for the TypedHandler adapter (Wave 1).
//!
//! `kelson_command::TypedHandler` gives handlers compile-time typed
//! input/output over the raw `Value` contract. These bridges connect the
//! MVVM presentation traits to that boundary:
//! - [`Validated`] implements `TypedInput`: JSON -> `RequestViewModel` ->
//!   validated business input (FormRequest role; parse AND validate before
//!   the handler runs).
//! - [`TypedResource`] implements `TypedOutput`: business result ->
//!   `ResponseViewModel` -> wire JSON (the Resource role; renamed from
//!   `Resource` in Wave 3 Phase 7 to give the name to the typed
//!   transformer trait in [`crate::resource`]).
//!
//! The traits live here (Web) because the Execution layer must not know
//! HTTP; modules opt in by depending on both crates.

use kelson_command::{TypedInput, TypedOutput};
use kelson_foundation::AppError;
use serde_json::Value;

use crate::{RequestViewModel, ResponseViewModel};

/// Typed input bridge: JSON -> [`RequestViewModel`] -> validated input.
/// Parse failures and validation failures are both validation-category
/// errors (422 envelopes at the HTTP edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validated<M: RequestViewModel> {
    pub input: M::Input,
}

impl<M> TypedInput for Validated<M>
where
    M: RequestViewModel + serde::de::DeserializeOwned + Send + 'static,
    M::Input: Send + 'static,
{
    fn from_value(value: Value) -> Result<Self, AppError> {
        let model: M = serde_json::from_value(value)
            .map_err(|e| AppError::validation(format!("invalid request shape: {e}")))?;
        let input = model.validate()?;
        Ok(Validated { input })
    }
}

/// Typed output bridge: business result -> [`ResponseViewModel`] -> wire.
/// (Wave 3 Phase 7: renamed from `Resource` — the name now belongs to the
/// typed transformer trait in [`crate::resource`].)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedResource<R: ResponseViewModel> {
    pub source: R::Source,
}

impl<R> TypedOutput for TypedResource<R>
where
    R: ResponseViewModel + Send + 'static,
    R::Source: Send + 'static,
{
    fn into_value(self) -> Value {
        let representation = R::from_source(&self.source);
        serde_json::to_value(&representation).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct NameForm {
        #[serde(default)]
        name: String,
    }

    impl RequestViewModel for NameForm {
        type Input = String;
        fn validate(self) -> Result<String, AppError> {
            let name = self.name.trim().to_string();
            if name.is_empty() {
                return Err(AppError::validation("name is required"));
            }
            Ok(name)
        }
    }

    #[derive(serde::Serialize)]
    struct NameResource {
        upper: String,
    }

    impl ResponseViewModel for NameResource {
        type Source = String;
        fn from_source(source: &String) -> Self {
            Self {
                upper: source.to_uppercase(),
            }
        }
    }

    #[test]
    fn validated_parses_then_validates() {
        let ok = <Validated<NameForm> as TypedInput>::from_value(
            serde_json::json!({ "name": " kelson " }),
        )
        .unwrap();
        assert_eq!(ok.input, "kelson");

        let blank =
            <Validated<NameForm> as TypedInput>::from_value(serde_json::json!({ "name": "  " }));
        assert!(blank.is_err());

        let bad_shape = <Validated<NameForm> as TypedInput>::from_value(serde_json::json!([
            "not", "an", "object"
        ]));
        assert!(bad_shape.is_err());
    }

    #[test]
    fn resource_renders_the_wire_representation() {
        let resource = TypedResource::<NameResource> {
            source: "kelson".to_string(),
        };
        assert_eq!(
            <TypedResource<NameResource> as TypedOutput>::into_value(resource),
            serde_json::json!({ "upper": "KELSON" })
        );
    }
}
