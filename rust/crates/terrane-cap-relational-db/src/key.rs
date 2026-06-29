use serde_json::{Map, Value};
use terrane_cap_interface::{Error, Result};

use crate::spec::{key_part_field, KeyPart, TableSpec};

pub fn encode_primary_key(spec: &TableSpec, row: &Value) -> Result<String> {
    let mut parts = Vec::new();
    parts.extend(values_from_row(row, &spec.primary_key.partition, false)?);
    parts.extend(values_from_row(row, &spec.primary_key.sort, false)?);
    encode_tuple(&parts)
}

pub fn encode_key_from_object(spec: &TableSpec, key: &Value) -> Result<String> {
    if let Some(obj) = key.as_object() {
        if obj.contains_key("partition") {
            return encode_key_from_tuple_json(key);
        }
        let mut values = Vec::new();
        for part in spec
            .primary_key
            .partition
            .iter()
            .chain(spec.primary_key.sort.iter())
        {
            let field = key_part_field(part);
            let value = obj.get(field).ok_or_else(|| {
                Error::InvalidInput(format!("keyJson missing primary key field {field:?}"))
            })?;
            values.push(value.clone());
        }
        return encode_tuple(&values);
    }
    Err(Error::InvalidInput("keyJson must be an object".into()))
}

pub fn encode_key_from_tuple_json(value: &Value) -> Result<String> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("key tuple JSON must be an object".into()))?;
    let mut values = Vec::new();
    let partition = obj
        .get("partition")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidInput("key tuple needs partition array".into()))?;
    values.extend(partition.iter().cloned());
    if let Some(sort) = obj.get("sort").and_then(Value::as_array) {
        values.extend(sort.iter().cloned());
    }
    encode_tuple(&values)
}

pub fn values_from_row(row: &Value, parts: &[KeyPart], sparse: bool) -> Result<Vec<Value>> {
    let obj = row
        .as_object()
        .ok_or_else(|| Error::InvalidInput("row must be a JSON object".into()))?;
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        let field = key_part_field(part);
        match obj.get(field) {
            Some(Value::Null) | None if sparse => return Ok(Vec::new()),
            Some(Value::Null) | None => {
                return Err(Error::InvalidInput(format!("missing key field {field:?}")))
            }
            Some(value) => values.push(value.clone()),
        }
    }
    Ok(values)
}

pub fn encode_tuple(values: &[Value]) -> Result<String> {
    values
        .iter()
        .map(encode_scalar)
        .collect::<Result<Vec<_>>>()
        .map(|v| v.join("/"))
}

pub fn encode_scalar(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(format!("S{}", percent_encode(s))),
        Value::Bool(false) => Ok("B0".into()),
        Value::Bool(true) => Ok("B1".into()),
        Value::Number(n) => {
            let f = n
                .as_f64()
                .ok_or_else(|| Error::InvalidInput("number key is not finite".into()))?;
            if !f.is_finite() {
                return Err(Error::InvalidInput("number key is not finite".into()));
            }
            let mut bits = f.to_bits();
            if bits & (1u64 << 63) != 0 {
                bits = !bits;
            } else {
                bits ^= 1u64 << 63;
            }
            Ok(format!("N{bits:016x}"))
        }
        Value::Null => Err(Error::InvalidInput("null cannot be used in a key".into())),
        _ => Err(Error::InvalidInput(
            "only scalar values can be used in keys".into(),
        )),
    }
}

pub fn primary_key_json(spec: &TableSpec, row: &Value) -> Result<String> {
    let obj = row
        .as_object()
        .ok_or_else(|| Error::InvalidInput("row must be a JSON object".into()))?;
    let mut out = Map::new();
    for part in spec
        .primary_key
        .partition
        .iter()
        .chain(spec.primary_key.sort.iter())
    {
        let field = key_part_field(part);
        let value = obj.get(field).ok_or_else(|| {
            Error::InvalidInput(format!("row missing primary key field {field:?}"))
        })?;
        out.insert(field.to_string(), value.clone());
    }
    serde_json::to_string(&Value::Object(out))
        .map_err(|e| Error::Storage(format!("serialize primary key JSON: {e}")))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn row_key(table: &str, pk: &str) -> String {
    format!("{}row/{}/{}", crate::RDB_PREFIX, table, pk)
}

pub fn table_spec_key(table: &str) -> String {
    format!("{}table/{}/spec", crate::RDB_PREFIX, table)
}

pub fn table_summary_key(table: &str) -> String {
    format!("{}tables/{}", crate::RDB_PREFIX, table)
}

pub fn index_key(table: &str, index: &str, partition: &str, sort: &str, pk: &str) -> String {
    format!(
        "{}idx/{}/{}/{}/{}/{}",
        crate::RDB_PREFIX,
        table,
        index,
        partition,
        sort_key_component(sort),
        pk
    )
}

pub fn index_partition_prefix(table: &str, index: &str, partition: &str) -> String {
    format!(
        "{}idx/{}/{}/{}/",
        crate::RDB_PREFIX,
        table,
        index,
        partition
    )
}

pub fn unique_key(table: &str, index: &str, partition: &str) -> String {
    format!(
        "{}uniq/{}/{}/{}",
        crate::RDB_PREFIX,
        table,
        index,
        partition
    )
}

fn sort_key_component(sort: &str) -> &str {
    if sort.is_empty() {
        "_"
    } else {
        sort
    }
}
