//! # kelson-models
//! Data model + relationship contracts (blueprint L15/L16, S3 item 42).
//!
//! Framework-level schema descriptions: models declare typed fields plus
//! relationships; the registry validates the whole graph before anything can
//! render SQL or seed stores. Deliberately contract-only — no ORM execution.
//! Codegen (L66) and stores consume these definitions; identifiers validated
//! here reuse the same charset as [`kelson_query::Ident`] so nothing unsafe can
//! reach generated SQL. S8 adds the model runtime (mass-assignment boundary,
//! casts, scopes, accessors/mutators, events, relation loading) and the
//! semantic [`store::ModelStore`] CRUD port.

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod bindings;
pub mod casts;
pub mod probes;
pub mod prune;
pub mod runtime;
pub mod store;

pub use bindings::{resolve_binding, RouteBindingRegistry, RouteBindingResolver, RouteBindingsRef};
pub use casts::{Cast, EncryptedCast};
pub use probes::{ModelProbe, ProbeRegistry, ProbeRegistryRef, StoreProbe};
pub use prune::{PruneRegistry, PruneRegistryRef};
pub use runtime::{
    json_to_bind, json_value_to_bind, parse_timestamp, record_to_columns, record_to_columns_for,
    EagerConstraint, ModelEvent, ModelObserver, ModelPhase, ModelRuntime, PivotSync,
    PrunePredicate, RecordChanges,
};
pub use store::{
    now_rfc3339, AggregateOp, ExistsFilter, FieldFilter, FilterOp, InMemoryModelStore,
    ListOpts, ModelStore, Record,
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
    /// Optional cast spec applied at the mass-assignment boundary (Wave 3
    /// Phase 5): `date`, `datetime`, `decimal:<n>`, `json`, `array`,
    /// `enum:<a,b>`, `encrypted`, `hashed`, or a custom name registered
    /// with `ModelRuntime::with_cast`.
    #[serde(default)]
    pub cast: Option<String>,
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
            cast: None,
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

    /// Declare a cast spec (see [`FieldDef::cast`]).
    pub fn cast(mut self, spec: impl Into<String>) -> Self {
        self.cast = Some(spec.into());
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
    /// Pivot table carries both FKs (N:M) — Laravel `belongsToMany`. The
    /// pivot is a registered join model (`through`) or synthesized from
    /// `pivot_table` (id + both FK columns).
    ManyToMany,
    /// `<name>_type` + `<name>_id` name the target per ROW (N:1 poly) —
    /// Laravel `morphTo`; no single target model, resolved per stored type
    /// through the registry's morph map.
    MorphTo,
    /// Target holds `<type>_type` + `<id>` pointing back at THIS model
    /// (1:N poly) — Laravel `morphMany`.
    MorphMany,
    /// Pivot rows carry a type column naming the owning side (N:M poly) —
    /// Laravel `morphToMany`.
    MorphToMany,
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
    /// Pivot table for ManyToMany (Laravel's alphabetical `<a>_<b>`
    /// convention — see [`RelationDef::pivot_table_for`]). Optional when
    /// `through` names a registered join model.
    pub pivot_table: Option<String>,
    /// Type column for morph relations (`<name>_type`): which model the
    /// id column points at (MorphTo), which owner a target row serves
    /// (MorphMany), or which side a pivot row links (MorphToMany).
    pub morph_type: Option<String>,
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
            pivot_table: None,
            morph_type: None,
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
            pivot_table: None,
            morph_type: None,
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
            pivot_table: None,
            morph_type: None,
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
            pivot_table: None,
            morph_type: None,
        }
    }

    /// Laravel `belongsToMany`: both sides meet in a pivot table carrying
    /// `foreign_key` (-> this model) and `target_key` (-> target). Pass the
    /// pivot table name (convention: [`RelationDef::pivot_table_for`]); set
    /// `through` afterwards to a registered join model when the pivot has
    /// extra columns — otherwise it is synthesized per load.
    pub fn many_to_many(
        name: impl Into<String>,
        target: impl Into<String>,
        pivot_table: impl Into<String>,
        foreign_key: impl Into<String>,
        target_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::ManyToMany,
            target: target.into(),
            foreign_key: foreign_key.into(),
            target_key: Some(target_key.into()),
            through: None,
            pivot_table: Some(pivot_table.into()),
            morph_type: None,
        }
    }

    /// Pivot-table naming convention: two SINGULAR model names, lowercased
    /// and stripped of a trailing `s`, joined alphabetically —
    /// `Doctor` + `Patient` -> `doctor_patient`.
    pub fn pivot_table_for(a: &str, b: &str) -> String {
        let norm = |name: &str| name.to_lowercase().trim_end_matches('s').to_string();
        let (x, y) = (norm(a), norm(b));
        if x <= y {
            format!("{x}_{y}")
        } else {
            format!("{y}_{x}")
        }
    }

    /// Laravel `morphTo`: `type_column` names the model and `id_column`
    /// carries its key on THIS row — there is no single target; loads and
    /// filters resolve through the stored type (registry morph map).
    pub fn morph_to(
        name: impl Into<String>,
        type_column: impl Into<String>,
        id_column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::MorphTo,
            // Per-row — see the morph map; never registry-resolved.
            target: String::new(),
            foreign_key: id_column.into(),
            target_key: None,
            through: None,
            pivot_table: None,
            morph_type: Some(type_column.into()),
        }
    }

    /// Laravel `morphMany`: the target carries `type_column` naming THIS
    /// model plus `id_column` pointing back at its key.
    pub fn morph_many(
        name: impl Into<String>,
        target: impl Into<String>,
        type_column: impl Into<String>,
        id_column: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::MorphMany,
            target: target.into(),
            foreign_key: id_column.into(),
            target_key: None,
            through: None,
            pivot_table: None,
            morph_type: Some(type_column.into()),
        }
    }

    /// Laravel `morphToMany`: pivot rows carry `type_column` naming THIS
    /// side; `foreign_key` links the pivot to this model, `target_key` to
    /// the target.
    pub fn morph_to_many(
        name: impl Into<String>,
        target: impl Into<String>,
        pivot_table: impl Into<String>,
        type_column: impl Into<String>,
        foreign_key: impl Into<String>,
        target_key: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: RelationKind::MorphToMany,
            target: target.into(),
            foreign_key: foreign_key.into(),
            target_key: Some(target_key.into()),
            through: None,
            pivot_table: Some(pivot_table.into()),
            morph_type: Some(type_column.into()),
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
    /// Manifest serialization policy (Wave 3 Phase 5): these fields drop
    /// from every projection (applied after `sensitive`; per-call
    /// `make_visible` can still force one back).
    #[serde(default)]
    pub hidden: Vec<String>,
    /// When non-empty, ONLY these keys pass the stored-field projection
    /// (accessors still append — the Laravel `$visible`/`$appends` posture).
    #[serde(default)]
    pub visible: Vec<String>,
    /// Wave 3 Phase 6 optimistic locking: integer column compared on every
    /// `update` (`WHERE version = $n`, `SET version = n + 1`); a zero-row
    /// match fails the write with `codes::STALE_RECORD`. `None` (default)
    /// disables compare-and-swap. Validated at registration: the name must
    /// resolve to a declared `Int32`/`Int64` field.
    #[serde(default)]
    pub version_column: Option<String>,
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
            hidden: Vec::new(),
            visible: Vec::new(),
            version_column: None,
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

    /// Add manifest-hidden fields (dropped from every projection).
    pub fn hide(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.hidden.extend(names.into_iter().map(Into::into));
        self
    }

    /// Add manifest-visible fields; a non-empty list narrows the projection
    /// to exactly these keys (accessors still append).
    pub fn show(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.visible.extend(names.into_iter().map(Into::into));
        self
    }

    /// Wave 3 Phase 6: enable optimistic locking on an integer column —
    /// every `update` CAS-compares it and bumps it to `n + 1`.
    pub fn version_column(mut self, name: impl Into<String>) -> Self {
        self.version_column = Some(name.into());
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
    /// Laravel `Relation::morphMap`: stored polymorphic type value ->
    /// registered model name. Empty = the stored value IS the model name.
    pub morph_map: BTreeMap<String, String>,
}

/// Identifier charset shared with [`kelson_query::Ident`]: lowercase, digits,
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

    /// Laravel `Relation::morphMap`: map the stored polymorphic type value
    /// (`<name>_type` column) to a registered model. Aliases are checked
    /// against registered models by [`Self::validate_graph`].
    pub fn register_morph(
        &mut self,
        alias: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<(), AppError> {
        self.morph_map.insert(alias.into(), model.into());
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
            if let Some(spec) = &field.cast {
                casts::validate_cast_spec(spec).map_err(|e| {
                    AppError::validation(format!(
                        "field {}.{} cast {spec:?}: {}",
                        model.name, field.name, e.public_message
                    ))
                })?;
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
            if rel.kind == RelationKind::ManyToMany
                && rel.pivot_table.is_none()
                && rel.through.is_none()
            {
                return Err(AppError::validation(format!(
                    "relation {}.{} is many-to-many but has no pivot table or join model",
                    model.name, rel.name
                )));
            }
            if matches!(
                rel.kind,
                RelationKind::MorphTo | RelationKind::MorphMany | RelationKind::MorphToMany
            ) && rel.morph_type.is_none()
            {
                return Err(AppError::validation(format!(
                    "relation {}.{} is {:?} but has no morph type column",
                    model.name, rel.name, rel.kind
                )));
            }
            if rel.kind == RelationKind::MorphToMany
                && rel.pivot_table.is_none()
                && rel.through.is_none()
            {
                return Err(AppError::validation(format!(
                    "relation {}.{} is morph-to-many but has no pivot table or join model",
                    model.name, rel.name
                )));
            }
            if let Some(pivot) = &rel.pivot_table {
                validate_ident(
                    pivot,
                    &format!("relation {}.{} pivot table", model.name, rel.name),
                )?;
            }
            if let Some(type_col) = &rel.morph_type {
                validate_ident(
                    type_col,
                    &format!("relation {}.{} morph type column", model.name, rel.name),
                )?;
            }
        }
        // Wave 3 Phase 6: the optimistic-lock column must be a declared
        // integer field so both stores can bind and compare it.
        if let Some(column) = &model.version_column {
            let Some(field) = model.field_named(column) else {
                return Err(AppError::validation(format!(
                    "model {} version_column {column:?} is not a declared field",
                    model.name
                )));
            };
            if !matches!(field.field_type, FieldType::Int32 | FieldType::Int64) {
                return Err(AppError::validation(format!(
                    "model {} version_column {column:?} must be an Int32 or Int64 field, got {:?}",
                    model.name, field.field_type
                )));
            }
        }
        Ok(())
    }

    /// Cross-model validation: every relation target (and `through`) must be
    /// a registered model; BelongsTo FKs must exist as fields on this model;
    /// morph-map aliases must name registered models.
    pub fn validate_graph(&self) -> Result<(), AppError> {
        for (alias, model_name) in &self.morph_map {
            if self.get(model_name).is_none() {
                return Err(AppError::validation(format!(
                    "morph map alias {alias:?} targets unknown model {model_name:?}"
                )));
            }
        }
        for model in &self.models {
            for rel in &model.relations {
                if rel.kind == RelationKind::MorphTo {
                    // No single target: the type/id columns live on THIS row.
                    let type_col = rel.morph_type.clone().unwrap_or_default();
                    if model.field_named(&type_col).is_none()
                        || model.field_named(&rel.foreign_key).is_none()
                    {
                        return Err(AppError::validation(format!(
                            "MorphTo {}.{}: columns {type_col:?}/{} missing on {}",
                            model.name, rel.name, rel.foreign_key, model.name
                        )));
                    }
                    continue;
                }
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
                if matches!(
                    rel.kind,
                    RelationKind::HasMany | RelationKind::HasOne | RelationKind::MorphMany
                ) {
                    if target.field_named(&rel.foreign_key).is_none() {
                        let label = match rel.kind {
                            RelationKind::HasMany => "HasMany",
                            RelationKind::MorphMany => "MorphMany",
                            _ => "HasOne",
                        };
                        return Err(AppError::validation(format!(
                            "{} {}.{}: foreign_key {} missing on target {}",
                            label, model.name, rel.name, rel.foreign_key, target.name
                        )));
                    }
                    if let Some(type_col) = &rel.morph_type {
                        if target.field_named(type_col).is_none() {
                            return Err(AppError::validation(format!(
                                "MorphMany {}.{}: morph type column {type_col:?} missing on target {}",
                                model.name, rel.name, target.name
                            )));
                        }
                    }
                }
                if let Some(through) = &rel.through {
                    let pivot = self.get(through).ok_or_else(|| {
                        AppError::validation(format!(
                            "relation {}.{} uses unknown join model {}",
                            model.name, rel.name, through
                        ))
                    })?;
                    if rel.kind == RelationKind::MorphToMany {
                        for col in [
                            Some(&rel.foreign_key),
                            rel.target_key.as_ref(),
                            rel.morph_type.as_ref(),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            if pivot.field_named(col).is_none() {
                                return Err(AppError::validation(format!(
                                    "MorphToMany {}.{}: pivot column {col:?} missing on {through}",
                                    model.name, rel.name
                                )));
                            }
                        }
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

    /// Wave 3 Phase 6: the optimistic-lock column must be a DECLARED
    /// integer field so both stores can bind and compare it.
    #[test]
    fn version_column_must_be_a_declared_integer_field() {
        let mut reg = ModelRegistry::new();
        let undeclared = ModelDef::new("Thing", "things")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .version_column("missing");
        let err = reg.register(undeclared).unwrap_err();
        assert!(
            err.public_message.contains("not a declared field"),
            "{}",
            err.public_message
        );

        let non_integer = ModelDef::new("Thing", "things")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("version", FieldType::Text))
            .version_column("version");
        let err = reg.register(non_integer).unwrap_err();
        assert!(
            err.public_message.contains("Int32 or Int64"),
            "{}",
            err.public_message
        );

        let ok = ModelDef::new("Thing", "things")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("version", FieldType::Int64))
            .version_column("version");
        reg.register(ok).expect("valid version column registers");
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
    fn cast_specs_and_manifest_lists_validate_at_register() {
        // Malformed builtin parameters are refused at register time.
        let bad = ModelDef::new("Ledger", "ledgers")
            .field(FieldDef::new("id", FieldType::Uuid))
            .field(FieldDef::new("amount", FieldType::Decimal).cast("decimal:x"));
        let err = ModelRegistry::new().register(bad).unwrap_err();
        assert!(err.public_message.contains("cast"), "{}", err.public_message);

        // Well-formed builtins and custom names pass; manifest lists record.
        let ok = ModelDef::new("Ledger", "ledgers")
            .field(FieldDef::new("id", FieldType::Uuid))
            .field(FieldDef::new("amount", FieldType::Decimal).cast("decimal:2"))
            .field(FieldDef::new("note", FieldType::Text).cast("money_v2"))
            .hide(["note"])
            .show(["id", "amount"]);
        let mut reg = ModelRegistry::new();
        reg.register(ok).unwrap();
        let stored = reg.get("Ledger").unwrap();
        assert_eq!(stored.hidden, vec!["note"]);
        assert_eq!(stored.visible, vec!["id", "amount"]);
        assert_eq!(
            stored.field_named("amount").unwrap().cast.as_deref(),
            Some("decimal:2")
        );
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
                pivot_table: None,
                morph_type: None,
            });
        assert!(ModelRegistry::new().register(no_through).is_err());
    }

    #[test]
    fn many_to_many_requires_a_pivot_table_or_join_model() {
        let no_pivot = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid))
            .relation(RelationDef {
                name: "doctors".into(),
                kind: RelationKind::ManyToMany,
                target: "Doctor".into(),
                foreign_key: "patient_id".into(),
                target_key: Some("doctor_id".into()),
                through: None,
                pivot_table: None,
                morph_type: None,
            });
        assert!(ModelRegistry::new().register(no_pivot).is_err());
    }

    #[test]
    fn pivot_table_for_is_alphabetical_and_singular() {
        assert_eq!(
            RelationDef::pivot_table_for("Doctor", "Patient"),
            "doctor_patient"
        );
        // Order-insensitive: both directions name the same table.
        assert_eq!(
            RelationDef::pivot_table_for("Patient", "Doctor"),
            RelationDef::pivot_table_for("Doctor", "Patient")
        );
    }

    #[test]
    fn morph_map_aliases_must_target_registered_models() {
        let mut reg = ModelRegistry::new();
        reg.register(
            ModelDef::new("Post", "posts").field(FieldDef::new("id", FieldType::Uuid)),
        )
        .unwrap();
        reg.register_morph("post", "Post").unwrap();
        assert!(reg.validate_graph().is_ok());
        reg.register_morph("ghost", "Missing").unwrap();
        assert!(reg.validate_graph().is_err());
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
