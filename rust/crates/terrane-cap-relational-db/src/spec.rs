use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use terrane_cap_interface::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSpec {
    pub spec_version: u64,
    pub schema_version: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub fields: BTreeMap<String, FieldSpec>,
    pub primary_key: KeySpec,
    #[serde(default)]
    pub indexes: BTreeMap<String, IndexSpec>,
    #[serde(default)]
    pub constraints: BTreeMap<String, ConstraintSpec>,
    #[serde(default)]
    pub options: TableOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySpec {
    pub partition: Vec<KeyPart>,
    #[serde(default)]
    pub sort: Vec<KeyPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyPart {
    Field(String),
    Object {
        field: String,
        #[serde(default = "asc")]
        order: String,
        #[serde(default = "reject_nulls")]
        nulls: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldSpec {
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default, rename = "default")]
    pub default_value: Option<Value>,
    #[serde(default, rename = "enum")]
    pub enum_values: Vec<Value>,
    #[serde(default)]
    pub min_length: Option<usize>,
    #[serde(default)]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
    #[serde(default)]
    pub exclusive_minimum: Option<f64>,
    #[serde(default)]
    pub exclusive_maximum: Option<f64>,
    #[serde(default)]
    pub multiple_of: Option<f64>,
    #[serde(default)]
    pub min_items: Option<usize>,
    #[serde(default)]
    pub max_items: Option<usize>,
    #[serde(default)]
    pub item_type: Option<FieldType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    String,
    Number,
    Integer,
    Boolean,
    Json,
    Object,
    Array,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexSpec {
    #[serde(default)]
    pub description: Option<String>,
    pub partition: Vec<KeyPart>,
    #[serde(default)]
    pub sort: Vec<KeyPart>,
    #[serde(default)]
    pub unique: bool,
    #[serde(default = "default_true")]
    pub sparse: bool,
    #[serde(default)]
    pub projection: ProjectionSpec,
    #[serde(default = "active")]
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProjectionSpec {
    #[default]
    Keys,
    All,
    Include {
        fields: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConstraintSpec {
    Unique {
        fields: Vec<String>,
        #[serde(default = "default_true")]
        sparse: bool,
    },
    RequiredTogether {
        fields: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableOptions {
    #[serde(default = "preserve")]
    pub unknown_fields: String,
    #[serde(default = "default_max_row_bytes")]
    pub max_row_bytes: usize,
    #[serde(default = "default_query_limit")]
    pub default_query_limit: usize,
    #[serde(default = "max_query_limit")]
    pub max_query_limit: usize,
    #[serde(default = "default_true")]
    pub canonical_json: bool,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self {
            unknown_fields: preserve(),
            max_row_bytes: default_max_row_bytes(),
            default_query_limit: default_query_limit(),
            max_query_limit: max_query_limit(),
            canonical_json: true,
        }
    }
}

pub fn parse_table_spec(table: &str, raw: &str) -> Result<TableSpec> {
    validate_name(table, "table")?;
    let spec: TableSpec = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("specJson is invalid JSON: {e}")))?;
    validate_table_spec(table, &spec)?;
    Ok(spec)
}

pub fn canonical_spec_json(spec: &TableSpec) -> Result<String> {
    serde_json::to_string(spec).map_err(|e| Error::Storage(format!("serialize table spec: {e}")))
}

pub fn validate_table_spec(table: &str, spec: &TableSpec) -> Result<()> {
    if spec.spec_version != 1 {
        return Err(Error::InvalidInput("specVersion must be 1".into()));
    }
    if spec.schema_version == 0 {
        return Err(Error::InvalidInput(
            "schemaVersion must be a positive integer".into(),
        ));
    }
    if let Some(name) = &spec.name {
        validate_name(name, "spec name")?;
        if name != table {
            return Err(Error::InvalidInput(format!(
                "spec name {name:?} does not match table {table:?}"
            )));
        }
    }
    if spec.fields.is_empty() {
        return Err(Error::InvalidInput("fields must not be empty".into()));
    }
    for (name, field) in &spec.fields {
        validate_name(name, "field")?;
        validate_field_spec(name, field)?;
    }
    validate_key_parts(
        spec,
        &spec.primary_key.partition,
        "primaryKey.partition",
        false,
    )?;
    if spec.primary_key.partition.is_empty() {
        return Err(Error::InvalidInput(
            "primaryKey.partition must contain at least one field".into(),
        ));
    }
    validate_key_parts(spec, &spec.primary_key.sort, "primaryKey.sort", true)?;
    validate_disjoint_key_parts(
        &spec.primary_key.partition,
        &spec.primary_key.sort,
        "primaryKey",
    )?;
    for (name, index) in &spec.indexes {
        validate_name(name, "index")?;
        validate_index_spec(spec, name, index)?;
    }
    for (name, constraint) in &spec.constraints {
        validate_name(name, "constraint")?;
        validate_constraint(spec, name, constraint)?;
    }
    validate_options(&spec.options)?;
    Ok(())
}

pub fn validate_name(name: &str, what: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(Error::InvalidInput(format!(
            "{what} name must not be empty"
        )));
    };
    if !first.is_ascii_alphabetic() || name.len() > 64 {
        return Err(Error::InvalidInput(format!(
            "{what} name {name:?} must match ^[A-Za-z][A-Za-z0-9_]{{0,63}}$"
        )));
    }
    if chars.any(|c| !c.is_ascii_alphanumeric() && c != '_') {
        return Err(Error::InvalidInput(format!(
            "{what} name {name:?} must match ^[A-Za-z][A-Za-z0-9_]{{0,63}}$"
        )));
    }
    Ok(())
}

pub fn key_part_field(part: &KeyPart) -> &str {
    match part {
        KeyPart::Field(field) => field,
        KeyPart::Object { field, .. } => field,
    }
}

fn validate_key_parts(
    spec: &TableSpec,
    parts: &[KeyPart],
    label: &str,
    allow_empty: bool,
) -> Result<()> {
    if !allow_empty && parts.is_empty() {
        return Err(Error::InvalidInput(format!("{label} must not be empty")));
    }
    let mut seen = BTreeSet::new();
    for part in parts {
        let field_name = key_part_field(part);
        if !seen.insert(field_name.to_string()) {
            return Err(Error::InvalidInput(format!(
                "{label} repeats field {field_name:?}"
            )));
        }
        let Some(field) = spec.fields.get(field_name) else {
            return Err(Error::InvalidInput(format!(
                "{label} references unknown field {field_name:?}"
            )));
        };
        if !matches!(
            field.field_type,
            FieldType::String | FieldType::Number | FieldType::Integer | FieldType::Boolean
        ) {
            return Err(Error::InvalidInput(format!(
                "{label} field {field_name:?} must be scalar"
            )));
        }
        if let KeyPart::Object { order, nulls, .. } = part {
            if order != "asc" {
                return Err(Error::InvalidInput(format!(
                    "{label} only supports order=asc in v1"
                )));
            }
            if nulls != "reject" {
                return Err(Error::InvalidInput(format!(
                    "{label} only supports nulls=reject in v1"
                )));
            }
        }
    }
    Ok(())
}

fn validate_disjoint_key_parts(partition: &[KeyPart], sort: &[KeyPart], label: &str) -> Result<()> {
    let partition_fields = partition
        .iter()
        .map(key_part_field)
        .collect::<BTreeSet<_>>();
    for part in sort {
        let field = key_part_field(part);
        if partition_fields.contains(field) {
            return Err(Error::InvalidInput(format!(
                "{label} repeats field {field:?} across partition and sort keys"
            )));
        }
    }
    Ok(())
}

fn validate_index_spec(spec: &TableSpec, name: &str, index: &IndexSpec) -> Result<()> {
    if index.status != "active" {
        return Err(Error::InvalidInput(format!(
            "index {name:?} status must be active"
        )));
    }
    validate_key_parts(
        spec,
        &index.partition,
        &format!("index {name}.partition"),
        false,
    )?;
    validate_key_parts(spec, &index.sort, &format!("index {name}.sort"), true)?;
    validate_disjoint_key_parts(&index.partition, &index.sort, &format!("index {name}"))?;
    if index.unique && !index.sort.is_empty() {
        return Err(Error::InvalidInput(format!(
            "unique index {name:?} must not define sort fields in v1"
        )));
    }
    match &index.projection {
        ProjectionSpec::Include { fields } => {
            if fields.is_empty() {
                return Err(Error::InvalidInput(format!(
                    "index {name:?} include projection needs fields"
                )));
            }
            for field in fields {
                if !spec.fields.contains_key(field) {
                    return Err(Error::InvalidInput(format!(
                        "index {name:?} projection references unknown field {field:?}"
                    )));
                }
            }
        }
        ProjectionSpec::Keys | ProjectionSpec::All => {}
    }
    Ok(())
}

fn validate_constraint(spec: &TableSpec, name: &str, constraint: &ConstraintSpec) -> Result<()> {
    let fields: &[String] = match constraint {
        ConstraintSpec::Unique { fields, .. } => fields,
        ConstraintSpec::RequiredTogether { fields } => fields,
    };
    if fields.is_empty() {
        return Err(Error::InvalidInput(format!(
            "constraint {name:?} needs fields"
        )));
    }
    let mut seen = BTreeSet::new();
    for field in fields {
        if !seen.insert(field.as_str()) {
            return Err(Error::InvalidInput(format!(
                "constraint {name:?} repeats field {field:?}"
            )));
        }
        let Some(field_spec) = spec.fields.get(field) else {
            return Err(Error::InvalidInput(format!(
                "constraint {name:?} references unknown field {field:?}"
            )));
        };
        if matches!(constraint, ConstraintSpec::Unique { .. })
            && !matches!(
                field_spec.field_type,
                FieldType::String | FieldType::Number | FieldType::Integer | FieldType::Boolean
            )
        {
            return Err(Error::InvalidInput(format!(
                "constraint {name:?} unique field {field:?} must be scalar"
            )));
        }
    }
    Ok(())
}

fn validate_field_spec(name: &str, field: &FieldSpec) -> Result<()> {
    if field.required && field.nullable {
        return Err(Error::InvalidInput(format!(
            "field {name:?} cannot be both required and nullable"
        )));
    }
    if let (Some(min), Some(max)) = (field.min_length, field.max_length) {
        if min > max {
            return Err(Error::InvalidInput(format!(
                "field {name:?} minLength exceeds maxLength"
            )));
        }
    }
    if let Some(pattern) = &field.pattern {
        Regex::new(pattern)
            .map_err(|e| Error::InvalidInput(format!("field {name:?} pattern is invalid: {e}")))?;
    }
    if let Some(format) = &field.format {
        match format.as_str() {
            "date-time" | "date" | "email" | "uri" | "uuid" => {}
            other => {
                return Err(Error::InvalidInput(format!(
                    "field {name:?} format {other:?} is unsupported"
                )))
            }
        }
    }
    if let Some(multiple) = field.multiple_of {
        if !multiple.is_finite() || multiple <= 0.0 {
            return Err(Error::InvalidInput(format!(
                "field {name:?} multipleOf must be positive"
            )));
        }
    }
    if let Some(default) = &field.default_value {
        validate_literal_type(name, field, default)?;
    }
    Ok(())
}

fn validate_literal_type(name: &str, field: &FieldSpec, value: &Value) -> Result<()> {
    match field.field_type {
        FieldType::String if !value.is_string() => Err(Error::InvalidInput(format!(
            "field {name:?} default must be string"
        ))),
        FieldType::Number if !value.is_number() => Err(Error::InvalidInput(format!(
            "field {name:?} default must be number"
        ))),
        FieldType::Integer if value.as_i64().is_none() && value.as_u64().is_none() => Err(
            Error::InvalidInput(format!("field {name:?} default must be integer")),
        ),
        FieldType::Boolean if !value.is_boolean() => Err(Error::InvalidInput(format!(
            "field {name:?} default must be boolean"
        ))),
        FieldType::Object if !value.is_object() => Err(Error::InvalidInput(format!(
            "field {name:?} default must be object"
        ))),
        FieldType::Array if !value.is_array() => Err(Error::InvalidInput(format!(
            "field {name:?} default must be array"
        ))),
        _ => Ok(()),
    }
}

fn validate_options(options: &TableOptions) -> Result<()> {
    match options.unknown_fields.as_str() {
        "preserve" | "reject" => {}
        other => {
            return Err(Error::InvalidInput(format!(
                "unknownFields {other:?} is unsupported"
            )))
        }
    }
    if options.max_row_bytes == 0 || options.max_row_bytes > 1024 * 1024 {
        return Err(Error::InvalidInput(
            "maxRowBytes must be between 1 and 1048576".into(),
        ));
    }
    if options.default_query_limit == 0
        || options.max_query_limit == 0
        || options.max_query_limit > 500
        || options.default_query_limit > options.max_query_limit
    {
        return Err(Error::InvalidInput(
            "query limits must be positive and maxQueryLimit <= 500".into(),
        ));
    }
    if !options.canonical_json {
        return Err(Error::InvalidInput(
            "canonicalJson must be true in v1".into(),
        ));
    }
    Ok(())
}

fn default_true() -> bool {
    true
}
fn asc() -> String {
    "asc".into()
}
fn reject_nulls() -> String {
    "reject".into()
}
fn active() -> String {
    "active".into()
}
fn preserve() -> String {
    "preserve".into()
}
fn default_max_row_bytes() -> usize {
    65_536
}
fn default_query_limit() -> usize {
    100
}
pub fn max_query_limit() -> usize {
    500
}
