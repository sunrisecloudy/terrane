use serde::Deserialize;
use serde_json::Value;
use terrane_cap_interface::{Error, Result, StateStore};

use crate::key::{encode_scalar, encode_tuple, index_partition_prefix, primary_key_json, row_key};
use crate::row::{canonical_json, parse_existing_row};
use crate::spec::{key_part_field, IndexSpec, KeyPart, TableSpec};

const HIGH_KEY_SUFFIX: &str = "\u{10ffff}";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub partition: Option<Value>,
    #[serde(default)]
    pub sort_prefix: Option<Value>,
    #[serde(default)]
    pub sort_start: Option<Value>,
    #[serde(default)]
    pub sort_end: Option<Value>,
    #[serde(default)]
    pub start_exclusive: bool,
    #[serde(default)]
    pub end_inclusive: bool,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub select: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    pub pk: String,
    pub key: Value,
    pub projection: Value,
}

pub fn parse_query(raw: &str, spec: &TableSpec) -> Result<QueryRequest> {
    let raw = raw.trim();
    let query = if raw.is_empty() {
        QueryRequest::default()
    } else {
        serde_json::from_str::<QueryRequest>(raw)
            .map_err(|e| Error::InvalidInput(format!("queryJson is invalid JSON: {e}")))?
    };
    if query.limit == Some(0) {
        return Err(Error::InvalidInput("query limit must be positive".into()));
    }
    if let Some(limit) = query.limit {
        if limit > spec.options.max_query_limit {
            return Err(Error::InvalidInput(format!(
                "query limit {limit} exceeds maxQueryLimit {}",
                spec.options.max_query_limit
            )));
        }
    }
    match query.select.as_deref().unwrap_or("rows") {
        "rows" | "projection" | "keys" => {}
        other => {
            return Err(Error::InvalidInput(format!(
                "query select {other:?} is unsupported; use rows, projection, or keys"
            )))
        }
    }
    Ok(query)
}

pub fn query_table(
    state: &dyn StateStore,
    app: &str,
    table: &str,
    spec: &TableSpec,
    index: &str,
    raw_query: &str,
) -> Result<String> {
    let query = parse_query(raw_query, spec)?;
    let limit = query.limit.unwrap_or(spec.options.default_query_limit);
    let select = query.select.as_deref().unwrap_or("rows");
    let values = if index.is_empty() || index == "primary" || index == "$primary" {
        query_primary(state, app, table, spec, &query, limit, select)?
    } else {
        let index_spec = spec.indexes.get(index).ok_or_else(|| {
            Error::InvalidInput(format!("unknown index {index:?} on table {table:?}"))
        })?;
        query_secondary(
            state,
            app,
            SecondaryTarget {
                table,
                spec,
                index_name: index,
                index: index_spec,
            },
            &query,
            limit,
            select,
        )?
    };
    serde_json::to_string(&values).map_err(|e| Error::Storage(format!("serialize query rows: {e}")))
}

fn query_primary(
    state: &dyn StateStore,
    app: &str,
    table: &str,
    spec: &TableSpec,
    query: &QueryRequest,
    limit: usize,
    select: &str,
) -> Result<Vec<Value>> {
    let partition = required_values(
        query.partition.as_ref(),
        &spec.primary_key.partition,
        "partition",
        true,
    )?;
    let partition_key = encode_tuple(&partition)?;

    if spec.primary_key.sort.is_empty() {
        reject_sort_filters(query)?;
        let Some(raw) = terrane_cap_kv::get_value(state, app, &row_key(table, &partition_key))?
        else {
            return Ok(Vec::new());
        };
        let row = parse_existing_row(&raw)?;
        return Ok(match select {
            "keys" => vec![serde_json::from_str(&primary_key_json(spec, &row)?)
                .map_err(|e| Error::Storage(format!("parse primary key JSON: {e}")))?],
            "projection" | "rows" => vec![row],
            _ => unreachable!(),
        });
    }

    let base_prefix = format!("{}row/{}/{}/", crate::RDB_PREFIX, table, partition_key);
    let (start, end, start_exact, end_exact) =
        range_for_sort(&base_prefix, &spec.primary_key.sort, query)?;
    let mut rows = Vec::new();
    for (key, raw) in
        terrane_cap_kv::scan_range(state, app, &start, &end, spec.options.max_query_limit)?
    {
        if query.start_exclusive && key == start_exact {
            continue;
        }
        if !query.end_inclusive && !end_exact.is_empty() && key == end_exact {
            continue;
        }
        let row = parse_existing_row(&raw)?;
        let value = match select {
            "keys" => serde_json::from_str(&primary_key_json(spec, &row)?)
                .map_err(|e| Error::Storage(format!("parse primary key JSON: {e}")))?,
            "projection" | "rows" => row,
            _ => unreachable!(),
        };
        rows.push(value);
        if !query.reverse && rows.len() >= limit {
            break;
        }
    }
    if query.reverse {
        rows.reverse();
        rows.truncate(limit);
    }
    Ok(rows)
}

struct SecondaryTarget<'a> {
    table: &'a str,
    spec: &'a TableSpec,
    index_name: &'a str,
    index: &'a IndexSpec,
}

fn query_secondary(
    state: &dyn StateStore,
    app: &str,
    target: SecondaryTarget<'_>,
    query: &QueryRequest,
    limit: usize,
    select: &str,
) -> Result<Vec<Value>> {
    let partition = required_values(
        query.partition.as_ref(),
        &target.index.partition,
        "partition",
        true,
    )?;
    let partition_key = encode_tuple(&partition)?;
    let base_prefix = index_partition_prefix(target.table, target.index_name, &partition_key);
    let (start, end, start_exact, end_exact) =
        range_for_sort(&base_prefix, &target.index.sort, query)?;
    let mut rows = Vec::new();
    for (key, raw) in terrane_cap_kv::scan_range(
        state,
        app,
        &start,
        &end,
        target.spec.options.max_query_limit,
    )? {
        if query.start_exclusive && key == start_exact {
            continue;
        }
        if !query.end_inclusive && !end_exact.is_empty() && key == end_exact {
            continue;
        }
        let entry = serde_json::from_str::<IndexEntry>(&raw)
            .map_err(|e| Error::Storage(format!("stored index entry is invalid: {e}")))?;
        let value = match select {
            "keys" => entry.key,
            "projection" => entry.projection,
            "rows" => {
                let Some(row_raw) =
                    terrane_cap_kv::get_value(state, app, &row_key(target.table, &entry.pk))?
                else {
                    continue;
                };
                parse_existing_row(&row_raw)?
            }
            _ => unreachable!(),
        };
        rows.push(value);
        if !query.reverse && rows.len() >= limit {
            break;
        }
    }
    if query.reverse {
        rows.reverse();
        rows.truncate(limit);
    }
    Ok(rows)
}

fn range_for_sort(
    base_prefix: &str,
    parts: &[KeyPart],
    query: &QueryRequest,
) -> Result<(String, String, String, String)> {
    if parts.is_empty() {
        reject_sort_filters(query)?;
        return Ok((
            base_prefix.to_string(),
            prefix_end(base_prefix),
            String::new(),
            String::new(),
        ));
    }
    if let Some(prefix) = &query.sort_prefix {
        if query.sort_start.is_some() || query.sort_end.is_some() {
            return Err(Error::InvalidInput(
                "queryJson cannot combine sortPrefix with sortStart/sortEnd".into(),
            ));
        }
        let values = values_from_query(prefix, parts, "sortPrefix", false)?;
        let encoded = encode_tuple(&values)?;
        let start = format!("{base_prefix}{}", sort_component(&encoded));
        return Ok((
            start.clone(),
            prefix_end(&start),
            String::new(),
            String::new(),
        ));
    }

    let start = if let Some(start) = &query.sort_start {
        let values = values_from_query(start, parts, "sortStart", true)?;
        format!("{base_prefix}{}", sort_component(&encode_tuple(&values)?))
    } else {
        base_prefix.to_string()
    };
    let end_exact = if let Some(end) = &query.sort_end {
        let values = values_from_query(end, parts, "sortEnd", true)?;
        format!("{base_prefix}{}", sort_component(&encode_tuple(&values)?))
    } else {
        String::new()
    };
    let end = if end_exact.is_empty() {
        prefix_end(base_prefix)
    } else if query.end_inclusive {
        prefix_end(&end_exact)
    } else {
        end_exact.clone()
    };
    Ok((start.clone(), end, start, end_exact))
}

fn reject_sort_filters(query: &QueryRequest) -> Result<()> {
    if query.sort_prefix.is_some() || query.sort_start.is_some() || query.sort_end.is_some() {
        return Err(Error::InvalidInput(
            "queryJson specifies sort filters but the target has no sort key".into(),
        ));
    }
    Ok(())
}

fn required_values(
    raw: Option<&Value>,
    parts: &[KeyPart],
    label: &str,
    exact: bool,
) -> Result<Vec<Value>> {
    let raw = raw.ok_or_else(|| Error::InvalidInput(format!("queryJson requires {label}")))?;
    values_from_query(raw, parts, label, exact)
}

fn values_from_query(
    raw: &Value,
    parts: &[KeyPart],
    label: &str,
    exact: bool,
) -> Result<Vec<Value>> {
    let values = match raw {
        Value::Array(items) => items.clone(),
        Value::Object(obj) => {
            let mut values = Vec::new();
            for part in parts {
                let field = key_part_field(part);
                match obj.get(field) {
                    Some(value) => values.push(value.clone()),
                    None if exact => {
                        return Err(Error::InvalidInput(format!(
                            "queryJson {label} missing field {field:?}"
                        )))
                    }
                    None => break,
                }
            }
            values
        }
        other if parts.len() == 1 => vec![other.clone()],
        _ => {
            return Err(Error::InvalidInput(format!(
                "queryJson {label} must be an array or object"
            )))
        }
    };
    if exact && values.len() != parts.len() {
        return Err(Error::InvalidInput(format!(
            "queryJson {label} needs {} value(s), got {}",
            parts.len(),
            values.len()
        )));
    }
    if !exact && values.len() > parts.len() {
        return Err(Error::InvalidInput(format!(
            "queryJson {label} has more values than the target sort key"
        )));
    }
    for value in &values {
        encode_scalar(value)?;
    }
    Ok(values)
}

fn sort_component(encoded: &str) -> &str {
    if encoded.is_empty() {
        "_"
    } else {
        encoded
    }
}

fn prefix_end(prefix: &str) -> String {
    format!("{prefix}{HIGH_KEY_SUFFIX}")
}

pub fn index_entry_json(
    pk: &str,
    row: &Value,
    spec: &TableSpec,
    projection_json: &str,
) -> Result<String> {
    let key = serde_json::from_str::<Value>(&primary_key_json(spec, row)?)
        .map_err(|e| Error::Storage(format!("parse primary key JSON: {e}")))?;
    let projection = serde_json::from_str::<Value>(projection_json)
        .map_err(|e| Error::Storage(format!("parse projection JSON: {e}")))?;
    canonical_json(&serde_json::json!({
        "pk": pk,
        "key": key,
        "projection": projection
    }))
}
