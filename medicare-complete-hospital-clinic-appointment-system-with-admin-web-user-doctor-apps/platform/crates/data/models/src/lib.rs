//! # keel-models
//! Data model + relationship contracts (blueprint L15/L16, S3 item 42).
//!
//! Framework-level schema descriptions: models declare typed fields plus
//! relationships; the registry validates the whole graph before anything can
//! render SQL or seed stores. Deliberately contract-only — no ORM execution.
//! Codegen (L66) and stores consume these definitions; identifiers validated
//! here reuse the same charset as [`keel_query::Ident`] so nothing unsafe can
//! reach generated SQL. S8 adds the model runtime (mass-assignment boundary,
//! casts, scopes, accessors/mutators, events, relation loading) and the
//! semantic [`store::ModelStore`] CRUD port.

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

pub mod bindings;
pub mod runtime;
pub mod store;

pub use bindings::{RouteBindingRegistry, RouteBindingResolver, RouteBindingsRef};
pub use runtime::{
    json_to_bind, json_value_to_bind, parse_timestamp, record_to_columns, ModelEvent,
    ModelObserver, ModelPhase, ModelRuntime,
};
pub use store::{
    now_rfc3339, FieldFilter, FilterOp, InMemoryModelStore, ListOpts, ModelStore, Record,
};

use std::sync::Arc;

/// Registry carrier for the model store port. The Platform phase registers
/// one (Pg-backed in production, in-memory in degraded boot/TestKit); module
/// providers consume it without naming the host crate.
#[derive(Clone)]
pub struct ModelStoreRef(pub Arc<dyn ModelStore>);

/// Scalar field types understood by the persistence generators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldType {
    Uuid,
    Bool,
    Int32,
    Int64,
    Float,
    Decimal,
    Text,
    /// CLOB: unbounded text (curtain-lined, never indexed).
    TextLarge,
    Timestamp,
    Date,
    Json,
    /// Named enum; the label list is the domain.
    Enum {
        variants: Vec<String>,
    },
}

impl FieldType {
    pub fn sql_type(&self) -> &'static str {
        match self {
            FieldType::Uuid => "uuid",
            FieldType::Bool => "boolean",
            FieldType::Int32 => "integer",
            FieldType::Int64 => "bigint",
            FieldType::Float => "double precision",
            FieldType::Decimal => "numeric",
            FieldType::Text => "text",
            FieldType::TextLarge => "text",
            FieldType::Timestamp => "timestamptz",
            FieldType::Date => "date",
            FieldType::Json => "jsonb",
            FieldType::Enum { .. } => "text",
        }
    }
}

/// One column on a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub field_type: FieldType,
    /// REQUIRED columns reject NULL at write time.
    pub required: bool,
    /// UNIQUE constraint + implicit index.
    pub unique: bool,
    /// Non-unique secondary index.
    pub indexed: bool,
    /// Redacted in logs/projections unless an elevated plane allows it (L27).
    pub sensitive: bool,
    /// Optional DEFAULT rendered into DDL.
    pub default_sql: Option<String>,
}

impl FieldDef {
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type,
            required: false,
            unique: false,
            indexed: false,
            sensitive: false,
            default_sql: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }
    pub fn indexed(mut self) -> Self {
        self.indexed = true;
        self
    }
    pub fn sensitive(mut self) -> Self {
        self.sensitive = true;
        self
    }
    pub fn with_default(mut self, sql: impl Into<String>) -> Self {
        self.default_sql = Some(sql.into());
        self
    }
}

/// Relationship cardinality between two models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationKind {
    /// This model holds the FK pointing at the target (N:1).
    BelongsTo,
    /// Target holds the FK pointing back (1:N).
    HasMany,
    /// Target holds a unique FK pointing back (1:1).
    HasOne,
    /// Join model carries both FKs (N:M).
    ManyThrough,
}

/// One declared relationship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationDef {
    pub name: String,
    pub kind: RelationKind,
    /// Target model (registry-validated).
    pub target: String,
    /// FK column on the OWNING side (this model for BelongsTo; target for
    /// HasMany/HasOne; the join model for ManyThrough).
    pub foreign_key: String,
    /// Referenced key (defaults to the target primary key).
    pub target_key: Option<String>,
    /// Join model name, REQUIRED for ManyThrough.
    pub through: Option<String>,
}

impl RelationDef {
    pub fn belongs_to(
        name: impl Into<String>,
        target: impl Into<String>,
        foreign_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::BelongsTo,
            target: target.into(),
            foreign_key: foreign_key.into(),
            target_key: None,
            through: None,
        }
    }

    pub fn has_many(
        name: impl Into<String>,
        target: impl Into<String>,
        foreign_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::HasMany,
            target: target.into(),
            foreign_key: foreign_key.into(),
            target_key: None,
            through: None,
        }
    }

    pub fn has_one(
        name: impl Into<String>,
        target: impl Into<String>,
        foreign_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::HasOne,
            target: target.into(),
            foreign_key: foreign_key.into(),
            target_key: None,
            through: None,
        }
    }

    pub fn many_through(
        name: impl Into<String>,
        target: impl Into<String>,
        through: impl Into<String>,
        foreign_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::ManyThrough,
            target: target.into(),
            foreign_key: foreign_key.into(),
            target_key: None,
            through: Some(through.into()),
        }
    }
}

/// A model: identity, columns, relationships, tenancy posture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDef {
    /// SingVal model name (registry key), e.g. `Appointment`.
    pub name: String,
    /// Physical table, e.g. `appointments`.
    pub table: String,
    /// TENANT-plane model: every row scoped by `tenant_id` (L11).
    pub tenant_scoped: bool,
    /// Soft-delete via `deleted_at` instead of physical removal.
    pub soft_deletes: bool,
    pub fields: Vec<FieldDef>,
    pub relations: Vec<RelationDef>,
}

impl ModelDef {
    pub fn new(name: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            table: table.into(),
            tenant_scoped: false,
            soft_deletes: true,
            fields: Vec::new(),
            relations: Vec::new(),
        }
    }

    pub fn tenant_scoped(mut self) -> Self {
        self.tenant_scoped = true;
        self
    }

    pub fn hard_deletes(mut self) -> Self {
        self.soft_deletes = false;
        self
    }

    pub fn field(mut self, field: FieldDef) -> Self {
        self.fields.push(field);
        self
    }

    pub fn relation(mut self, relation: RelationDef) -> Self {
        self.relations.push(relation);
        self
    }

    pub fn field_named(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// Registry of models; validates names, FK targets, and relationship graphs.
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    models: Vec<ModelDef>,
}

/// Identifier charset shared with [`keel_query::Ident`]: lowercase, digits,
/// underscores; leading letter/underscore; <= 64 chars.
fn validate_ident(raw: &str, what: &str) -> Result<(), AppError> {
    let err = |why: &str| AppError::validation(format!("invalid {what} identifier {raw:?}: {why}"));
    if raw.is_empty() {
        return Err(err("empty"));
    }
    if raw.len() > 64 {
        return Err(err("longer than 64 chars"));
    }
    let mut chars = raw.chars();
    let first = chars.next().expect("non-empty");
    if !(first.is_ascii_lowercase() || first == '_' || first.is_ascii_uppercase()) {
        return Err(err("must start with a letter or _"));
    }
    if chars.any(|c| !(c.is_ascii_alphanumeric() || c == '_')) {
        return Err(err("must contain only letters, digits, _"));
    }
    Ok(())
}

fn validate_pascal(raw: &str, what: &str) -> Result<(), AppError> {
    validate_ident(raw, what)?;
    if !raw.chars().next().unwrap().is_ascii_uppercase() {
        return Err(AppError::validation(format!(
            "{what} {raw:?} must be PascalCase"
        )));
    }
    Ok(())
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a model; duplicate names are a registration error.
    pub fn register(&mut self, model: ModelDef) -> Result<(), AppError> {
        if self.models.iter().any(|m| m.name == model.name) {
            return Err(AppError::validation(format!(
                "model registered twice: {}",
                model.name
            )));
        }
        self.validate_model(&model)?;
        self.models.push(model);
        Ok(())
    }

    /// Standalone model-shape validation (names, enums, unique-field count).
    fn validate_model(&self, model: &ModelDef) -> Result<(), AppError> {
        validate_pascal(&model.name, "model")?;
        validate_ident(&model.table, "table")?;
        if model.fields.is_empty() {
            return Err(AppError::validation(format!(
                "model {} has no fields",
                model.name
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for field in &model.fields {
            validate_ident(&field.name, &format!("field {}.{}", model.name, field.name))?;
            if !seen.insert(field.name.clone()) {
                return Err(AppError::validation(format!(
                    "model {} declares field {} twice",
                    model.name, field.name
                )));
            }
            if let FieldType::Enum { variants } = &field.field_type {
                if variants.is_empty() {
                    return Err(AppError::validation(format!(
                        "enum field {}.{} needs at least one variant",
                        model.name, field.name
                    )));
                }
            }
        }
        let mut rel_names = std::collections::BTreeSet::new();
        for rel in &model.relations {
            validate_ident(&rel.name, &format!("relation {}.{}", model.name, rel.name))?;
            if !rel_names.insert(rel.name.clone()) {
                return Err(AppError::validation(format!(
                    "model {} declares relation {} twice",
                    model.name, rel.name
                )));
            }
            if rel.kind == RelationKind::ManyThrough && rel.through.is_none() {
                return Err(AppError::validation(format!(
                    "relation {}.{} is ManyThrough but has no join model",
                    model.name, rel.name
                )));
            }
        }
        Ok(())
    }

    /// Cross-model validation: every relation target (and `through`) must be
    /// a registered model; BelongsTo FKs must exist as fields on this model.
    pub fn validate_graph(&self) -> Result<(), AppError> {
        for model in &self.models {
            for rel in &model.relations {
                let target = self.get(&rel.target).ok_or_else(|| {
                    AppError::validation(format!(
                        "relation {}.{} targets unknown model {}",
                        model.name, rel.name, rel.target
                    ))
                })?;
                if rel.kind == RelationKind::BelongsTo
                    && model.field_named(&rel.foreign_key).is_none()
                {
                    return Err(AppError::validation(format!(
                        "BelongsTo {}.{}: foreign_key {} missing on {}",
                        model.name, rel.name, rel.foreign_key, model.name
                    )));
                }
                if matches!(rel.kind, RelationKind::HasMany | RelationKind::HasOne)
                    && target.field_named(&rel.foreign_key).is_none()
                {
                    return Err(AppError::validation(format!(
                        "{} {}.{}: foreign_key {} missing on target {}",
                        if rel.kind == RelationKind::HasMany {
                            "HasMany"
                        } else {
                            "HasOne"
                        },
                        model.name,
                        rel.name,
                        rel.foreign_key,
                        target.name
                    )));
                }
                if let Some(through) = &rel.through {
                    if self.get(through).is_none() {
                        return Err(AppError::validation(format!(
                            "relation {}.{} uses unknown join model {}",
                            model.name, rel.name, through
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ModelDef> {
        self.models.iter().find(|m| m.name == name)
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn models(&self) -> &[ModelDef] {
        &self.models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appointment() -> ModelDef {
        ModelDef::new("Appointment", "appointments")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(
                FieldDef::new("tenant_id", FieldType::Uuid)
                    .required()
                    .indexed(),
            )
            .field(
                FieldDef::new("patient_id", FieldType::Uuid)
                    .required()
                    .indexed(),
            )
            .field(
                FieldDef::new(
                    "status",
                    FieldType::Enum {
                        variants: vec!["booked".into(), "confirmed".into(), "done".into()],
                    },
                )
                .required(),
            )
            .field(FieldDef::new("notes", FieldType::TextLarge).sensitive())
            .relation(RelationDef::belongs_to("patient", "Patient", "patient_id"))
    }

    fn patient() -> ModelDef {
        ModelDef::new("Patient", "patients")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(
                FieldDef::new("tenant_id", FieldType::Uuid)
                    .required()
                    .indexed(),
            )
            .relation(RelationDef::has_many(
                "appointments",
                "Appointment",
                "patient_id",
            ))
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = ModelRegistry::new();
        reg.register(appointment()).unwrap();
        reg.register(patient()).unwrap();
        assert_eq!(reg.len(), 2);
        assert!(reg.get("Appointment").is_some());
        assert!(reg.get("Visit").is_none());
        assert!(reg.validate_graph().is_ok());
    }

    #[test]
    fn duplicate_model_and_field_rejected() {
        let mut reg = ModelRegistry::new();
        reg.register(appointment()).unwrap();
        assert!(reg.register(appointment()).is_err());
        let bad = ModelDef::new("Dup", "dups")
            .field(FieldDef::new("id", FieldType::Uuid))
            .field(FieldDef::new("id", FieldType::Uuid));
        assert!(reg.register(bad).is_err());
    }

    #[test]
    fn identifier_rules_enforced() {
        assert!(ModelRegistry::new()
            .register(ModelDef::new("lowercase", "t").field(FieldDef::new("id", FieldType::Uuid)))
            .is_err());
        assert!(ModelRegistry::new()
            .register(ModelDef::new("Bad-Name", "t").field(FieldDef::new("id", FieldType::Uuid)))
            .is_err());
        assert!(ModelRegistry::new()
            .register(ModelDef::new("Ok", "tbl x").field(FieldDef::new("id", FieldType::Uuid)))
            .is_err());
        assert!(ModelRegistry::new()
            .register(ModelDef::new("Empty", "empty"))
            .is_err());
    }

    #[test]
    fn enum_needs_variants() {
        let bad = ModelDef::new("Bad", "bad")
            .field(FieldDef::new("status", FieldType::Enum { variants: vec![] }).required());
        assert!(ModelRegistry::new().register(bad).is_err());
    }

    #[test]
    fn relation_graph_validation() {
        let mut reg = ModelRegistry::new();
        reg.register(appointment()).unwrap();
        assert!(reg.validate_graph().is_err(), "Patient not registered yet");
        reg.register(patient()).unwrap();
        assert!(reg.validate_graph().is_ok());
    }

    #[test]
    fn belongs_to_fk_must_exist() {
        let mut reg = ModelRegistry::new();
        let orphan = ModelDef::new("Visit", "visits")
            .field(FieldDef::new("id", FieldType::Uuid))
            .relation(RelationDef::belongs_to("patient", "Patient", "missing_fk"));
        reg.register(patient()).unwrap();
        reg.register(orphan).unwrap();
        assert!(reg.validate_graph().is_err());
    }

    #[test]
    fn has_many_fk_must_exist_on_target() {
        let mut reg = ModelRegistry::new();
        reg.register(appointment()).unwrap();
        let bad_parent = ModelDef::new("Clinic", "clinics")
            .field(FieldDef::new("id", FieldType::Uuid))
            .relation(RelationDef::has_many(
                "appointments",
                "Appointment",
                "nonexistent_col",
            ));
        reg.register(bad_parent).unwrap();
        assert!(reg.validate_graph().is_err());
    }

    #[test]
    fn many_through_requires_join_model() {
        let no_through = ModelDef::new("Tag", "tags")
            .field(FieldDef::new("id", FieldType::Uuid))
            .relation(RelationDef {
                name: "patients".into(),
                kind: RelationKind::ManyThrough,
                target: "Patient".into(),
                foreign_key: "tag_id".into(),
                target_key: None,
                through: None,
            });
        assert!(ModelRegistry::new().register(no_through).is_err());
    }

    #[test]
    fn many_through_graph_ok() {
        let mut reg = ModelRegistry::new();
        // Standalone patient: the shared fixture carries an Appointment
        // relation not relevant to this many-through graph.
        reg.register(
            ModelDef::new("Patient", "patients")
                .tenant_scoped()
                .field(FieldDef::new("id", FieldType::Uuid).required().unique()),
        )
        .unwrap();
        reg.register(
            ModelDef::new("Tag", "tags")
                .field(FieldDef::new("id", FieldType::Uuid))
                .relation(RelationDef::many_through(
                    "patients",
                    "Patient",
                    "PatientTag",
                    "tag_id",
                )),
        )
        .unwrap();
        reg.register(
            ModelDef::new("PatientTag", "patient_tags")
                .field(FieldDef::new("patient_id", FieldType::Uuid).required())
                .field(FieldDef::new("tag_id", FieldType::Uuid).required()),
        )
        .unwrap();
        assert!(reg.validate_graph().is_ok());
    }

    #[test]
    fn defaults_and_helpers() {
        let m = appointment();
        assert!(m.tenant_scoped && m.soft_deletes);
        assert_eq!(m.fields.len(), 5);
        let notes = m.field_named("notes").unwrap();
        assert!(notes.sensitive && !notes.required);
        assert_eq!(notes.field_type.sql_type(), "text");
        assert_eq!(
            FieldDef::new("n", FieldType::Int64).field_type.sql_type(),
            "bigint"
        );
        let with_default = FieldDef::new("created_at", FieldType::Timestamp).with_default("now()");
        assert_eq!(with_default.default_sql.as_deref(), Some("now()"));
    }
}
