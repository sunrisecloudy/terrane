use regex::Regex;
use serde_json::{Map, Number, Value};
use terrane_cap_interface::{Error, Result};

use crate::spec::{ConstraintSpec, FieldSpec, FieldType, ProjectionSpec, TableSpec};

pub fn parse_and_validate_row(spec: &TableSpec, raw: &str) -> Result<Value> {
    if raw.len() > spec.options.max_row_bytes {
        return Err(Error::InvalidInput(format!(
            "row exceeds maxRowBytes {}",
            spec.options.max_row_bytes
        )));
    }
    let mut row: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("rowJson is invalid JSON: {e}")))?;
    validate_row_value(spec, &mut row)?;
    Ok(canonical_value(row))
}

pub fn parse_existing_row(raw: &str) -> Result<Value> {
    serde_json::from_str(raw)
        .map_err(|e| Error::Storage(format!("stored row JSON is invalid: {e}")))
}

pub fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&canonical_value(value.clone()))
        .map_err(|e| Error::Storage(format!("serialize canonical JSON: {e}")))
}

pub fn project_row(
    spec: &TableSpec,
    projection: &ProjectionSpec,
    row: &Value,
    pk_json: &str,
) -> Result<String> {
    match projection {
        ProjectionSpec::Keys => Ok(pk_json.to_string()),
        ProjectionSpec::All => canonical_json(row),
        ProjectionSpec::Include { fields } => {
            let obj = row
                .as_object()
                .ok_or_else(|| Error::InvalidInput("row must be object".into()))?;
            let mut out = Map::new();
            for part in spec
                .primary_key
                .partition
                .iter()
                .chain(spec.primary_key.sort.iter())
            {
                let field = crate::spec::key_part_field(part);
                if let Some(value) = obj.get(field) {
                    out.insert(field.to_string(), value.clone());
                }
            }
            for field in fields {
                if let Some(value) = obj.get(field) {
                    out.insert(field.clone(), value.clone());
                }
            }
            canonical_json(&Value::Object(out))
        }
    }
}

fn validate_row_value(spec: &TableSpec, row: &mut Value) -> Result<()> {
    let obj = row
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("row must be a JSON object".into()))?;
    if spec.options.unknown_fields == "reject" {
        for key in obj.keys() {
            if !spec.fields.contains_key(key) {
                return Err(Error::InvalidInput(format!("unknown row field {key:?}")));
            }
        }
    }
    for (name, field) in &spec.fields {
        if !obj.contains_key(name) {
            if let Some(default) = &field.default_value {
                obj.insert(name.clone(), default.clone());
            }
        }
        let Some(value) = obj.get(name) else {
            if field.required {
                return Err(Error::InvalidInput(format!(
                    "missing required field {name:?}"
                )));
            }
            continue;
        };
        validate_field_value(name, field, value)?;
    }
    for (name, constraint) in &spec.constraints {
        match constraint {
            ConstraintSpec::RequiredTogether { fields } => {
                let present = fields.iter().filter(|f| obj.contains_key(*f)).count();
                if present > 0 && present != fields.len() {
                    return Err(Error::InvalidInput(format!(
                        "constraint {name:?} requires fields together"
                    )));
                }
            }
            ConstraintSpec::Unique { .. } => {}
        }
    }
    Ok(())
}

fn validate_field_value(name: &str, field: &FieldSpec, value: &Value) -> Result<()> {
    if value.is_null() {
        if field.nullable {
            return Ok(());
        }
        return Err(Error::InvalidInput(format!(
            "field {name:?} must not be null"
        )));
    }
    match field.field_type {
        FieldType::String => {
            let Some(s) = value.as_str() else {
                return Err(type_err(name, "string"));
            };
            if let Some(min) = field.min_length {
                if s.chars().count() < min {
                    return Err(Error::InvalidInput(format!(
                        "field {name:?} is shorter than minLength"
                    )));
                }
            }
            if let Some(max) = field.max_length {
                if s.chars().count() > max {
                    return Err(Error::InvalidInput(format!(
                        "field {name:?} exceeds maxLength"
                    )));
                }
            }
            if let Some(pattern) = &field.pattern {
                let re = Regex::new(pattern).map_err(|e| {
                    Error::InvalidInput(format!("field {name:?} pattern invalid: {e}"))
                })?;
                if !re.is_match(s) {
                    return Err(Error::InvalidInput(format!(
                        "field {name:?} does not match pattern"
                    )));
                }
            }
            if let Some(format) = &field.format {
                validate_format(name, format, s)?;
            }
        }
        FieldType::Number => {
            let Some(n) = value.as_f64() else {
                return Err(type_err(name, "number"));
            };
            if !n.is_finite() {
                return Err(Error::InvalidInput(format!(
                    "field {name:?} must be finite"
                )));
            }
            validate_number_bounds(name, field, n)?;
        }
        FieldType::Integer => {
            let Some(n) = json_integer(value) else {
                return Err(type_err(name, "integer"));
            };
            validate_number_bounds(name, field, n as f64)?;
        }
        FieldType::Boolean if !value.is_boolean() => return Err(type_err(name, "boolean")),
        FieldType::Object if !value.is_object() => return Err(type_err(name, "object")),
        FieldType::Array => {
            let Some(items) = value.as_array() else {
                return Err(type_err(name, "array"));
            };
            if let Some(min) = field.min_items {
                if items.len() < min {
                    return Err(Error::InvalidInput(format!(
                        "field {name:?} has too few items"
                    )));
                }
            }
            if let Some(max) = field.max_items {
                if items.len() > max {
                    return Err(Error::InvalidInput(format!(
                        "field {name:?} has too many items"
                    )));
                }
            }
            if let Some(item_type) = field.item_type {
                for item in items {
                    let item_field = FieldSpec {
                        field_type: item_type,
                        description: None,
                        required: false,
                        nullable: false,
                        default_value: None,
                        enum_values: Vec::new(),
                        min_length: None,
                        max_length: None,
                        pattern: None,
                        format: None,
                        minimum: None,
                        maximum: None,
                        exclusive_minimum: None,
                        exclusive_maximum: None,
                        multiple_of: None,
                        min_items: None,
                        max_items: None,
                        item_type: None,
                    };
                    validate_field_value(name, &item_field, item)?;
                }
            }
        }
        FieldType::Json | FieldType::Boolean | FieldType::Object => {}
    }
    if !field.enum_values.is_empty() && !field.enum_values.iter().any(|v| v == value) {
        return Err(Error::InvalidInput(format!(
            "field {name:?} is not in enum"
        )));
    }
    Ok(())
}

fn validate_number_bounds(name: &str, field: &FieldSpec, n: f64) -> Result<()> {
    if let Some(min) = field.minimum {
        if n < min {
            return Err(Error::InvalidInput(format!(
                "field {name:?} is below minimum"
            )));
        }
    }
    if let Some(max) = field.maximum {
        if n > max {
            return Err(Error::InvalidInput(format!(
                "field {name:?} exceeds maximum"
            )));
        }
    }
    if let Some(min) = field.exclusive_minimum {
        if n <= min {
            return Err(Error::InvalidInput(format!(
                "field {name:?} is not above exclusiveMinimum"
            )));
        }
    }
    if let Some(max) = field.exclusive_maximum {
        if n >= max {
            return Err(Error::InvalidInput(format!(
                "field {name:?} is not below exclusiveMaximum"
            )));
        }
    }
    if let Some(multiple) = field.multiple_of {
        if (n / multiple).fract().abs() > f64::EPSILON {
            return Err(Error::InvalidInput(format!(
                "field {name:?} is not a multipleOf value"
            )));
        }
    }
    Ok(())
}

fn validate_format(name: &str, format: &str, value: &str) -> Result<()> {
    let ok = match format {
        "email" => value.contains('@') && !value.starts_with('@') && !value.ends_with('@'),
        "uuid" => value.len() == 36 && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "uri" => value.contains("://"),
        "date" => {
            value.len() == 10
                && value.chars().enumerate().all(|(i, c)| {
                    if matches!(i, 4 | 7) {
                        c == '-'
                    } else {
                        c.is_ascii_digit()
                    }
                })
        }
        "date-time" => value.contains('T'),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "field {name:?} is not valid {format}"
        )))
    }
}

fn json_integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .filter(|n| n.abs() <= 9_007_199_254_740_991)
}

fn type_err(name: &str, expected: &str) -> Error {
    Error::InvalidInput(format!("field {name:?} must be {expected}"))
}

pub fn canonical_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            let mut pairs: Vec<_> = map.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in pairs {
                out.insert(k, canonical_value(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical_value).collect()),
        Value::Number(n) => {
            Value::Number(Number::from_f64(n.as_f64().unwrap_or_default()).unwrap_or(n))
        }
        other => other,
    }
}
