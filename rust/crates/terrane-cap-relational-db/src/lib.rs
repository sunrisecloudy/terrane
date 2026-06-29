//! Relational table/index capability backed by `terrane-cap-kv` reserved keys.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use terrane_cap_interface::{
    arg, ensure_app_exists, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error,
    EventRecord, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod doc;
mod key;
mod query;
mod row;
pub mod spec;

use key::{
    encode_key_from_object, encode_primary_key, encode_tuple, index_key, primary_key_json, row_key,
    table_spec_key, table_summary_key, unique_key, values_from_row,
};
use row::{canonical_json, parse_and_validate_row, parse_existing_row, project_row};
use spec::{
    canonical_spec_json, parse_table_spec, validate_name, ConstraintSpec, IndexSpec, KeyPart,
    TableSpec,
};

/// Reserved app-local KV prefix for the relational capability.
pub const RDB_PREFIX: &str = "__terrane/rdb/v1/";

pub struct RelationalDbCapability;

impl Capability for RelationalDbCapability {
    fn namespace(&self) -> &'static str {
        "relational_db"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "relational_db.defineTable",
                },
                CommandSpec {
                    name: "relational_db.put",
                },
                CommandSpec {
                    name: "relational_db.delete",
                },
            ],
            events: Vec::new(),
            queries: Vec::new(),
            resources: resource_methods(),
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "relational_db.defineTable" => define_table(ctx, args),
            "relational_db.put" => put_row(ctx, args),
            "relational_db.delete" => delete_row(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "get" => read_get(ctx.state, ctx.app, args),
            "query" => read_query(ctx.state, ctx.app, args),
            "tables" => read_tables(ctx.state, ctx.app),
            "spec" => read_spec(ctx.state, ctx.app, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: relational_db.{other}"
            ))),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::relational_doc(include_internal)
    }
}

pub fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "defineTable",
            params: &["table", "specJson"],
        },
        ResourceMethod::Write {
            name: "put",
            params: &["table", "rowJson"],
        },
        ResourceMethod::Write {
            name: "delete",
            params: &["table", "keyJson"],
        },
        ResourceMethod::Read {
            name: "get",
            params: &["table", "keyJson"],
        },
        ResourceMethod::Read {
            name: "query",
            params: &["table", "index", "queryJson"],
        },
        ResourceMethod::Read {
            name: "tables",
            params: &[],
        },
        ResourceMethod::Read {
            name: "spec",
            params: &["table"],
        },
    ]
}

fn define_table(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let table = arg(args, 1, "table")?;
    let spec_json = joined_arg(args, 2, "specJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    let spec = parse_table_spec(&table, &spec_json)?;
    let canonical = canonical_spec_json(&spec)?;
    let spec_key = table_spec_key(&table);
    if let Some(existing) = terrane_cap_kv::get_value(ctx.state, &app, &spec_key)? {
        if existing == canonical {
            return Ok(Decision::Commit(Vec::new()));
        }
        if !table_is_empty(ctx.state, &app, &table)? {
            return Err(Error::InvalidInput(format!(
                "table {table:?} already has rows; in-place table migrations are not supported"
            )));
        }
    }
    let summary = canonical_json(&json!({
        "name": table,
        "schemaVersion": spec.schema_version,
        "specVersion": spec.spec_version,
        "fieldCount": spec.fields.len(),
        "indexCount": spec.indexes.len()
    }))?;
    Ok(Decision::Commit(vec![
        terrane_cap_kv::set_event(app.clone(), spec_key, canonical)?,
        terrane_cap_kv::set_event(app, table_summary_key(&table), summary)?,
    ]))
}

fn put_row(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let table = arg(args, 1, "table")?;
    let row_json = joined_arg(args, 2, "rowJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    let spec = load_table_spec(ctx.state, &app, &table)?;
    let row = parse_and_validate_row(&spec, &row_json)?;
    let row_json = canonical_json(&row)?;
    let pk = encode_primary_key(&spec, &row)?;
    let row_key = row_key(&table, &pk);
    let old_row = terrane_cap_kv::get_value(ctx.state, &app, &row_key)?
        .as_deref()
        .map(parse_existing_row)
        .transpose()?;

    let old_index = old_row
        .as_ref()
        .map(|old| index_entries(&table, &spec, old))
        .transpose()?
        .unwrap_or_default();
    let new_index = index_entries(&table, &spec, &row)?;
    check_unique_conflicts(ctx.state, &app, &new_index.unique, &pk)?;

    let mut records = Vec::new();
    records.extend(delete_changed(
        &app,
        &old_index.secondary,
        &new_index.secondary,
    )?);
    records.extend(delete_changed(&app, &old_index.unique, &new_index.unique)?);
    records.push(terrane_cap_kv::set_event(app.clone(), row_key, row_json)?);
    records.extend(set_changed(&app, &old_index.unique, &new_index.unique)?);
    records.extend(set_changed(
        &app,
        &old_index.secondary,
        &new_index.secondary,
    )?);
    Ok(Decision::Commit(records))
}

fn delete_row(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let table = arg(args, 1, "table")?;
    let key_json = joined_arg(args, 2, "keyJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    let spec = load_table_spec(ctx.state, &app, &table)?;
    let key_value = serde_json::from_str::<Value>(&key_json)
        .map_err(|e| Error::InvalidInput(format!("keyJson is invalid JSON: {e}")))?;
    let pk = encode_key_from_object(&spec, &key_value)?;
    let row_key = row_key(&table, &pk);
    let Some(old_raw) = terrane_cap_kv::get_value(ctx.state, &app, &row_key)? else {
        return Ok(Decision::Commit(Vec::new()));
    };
    let old_row = parse_existing_row(&old_raw)?;
    let old_index = index_entries(&table, &spec, &old_row)?;
    let mut records = vec![terrane_cap_kv::delete_event(app.clone(), row_key)?];
    records.extend(
        old_index
            .secondary
            .keys()
            .map(|key| terrane_cap_kv::delete_event(app.clone(), key.clone()))
            .collect::<Result<Vec<_>>>()?,
    );
    records.extend(
        old_index
            .unique
            .keys()
            .map(|key| terrane_cap_kv::delete_event(app.clone(), key.clone()))
            .collect::<Result<Vec<_>>>()?,
    );
    Ok(Decision::Commit(records))
}

fn read_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let table = arg(args, 0, "table")?;
    let key_json = joined_arg(args, 1, "keyJson")?;
    let spec = load_table_spec(state, app, &table)?;
    let key_value = serde_json::from_str::<Value>(&key_json)
        .map_err(|e| Error::InvalidInput(format!("keyJson is invalid JSON: {e}")))?;
    let pk = encode_key_from_object(&spec, &key_value)?;
    Ok(ReadValue::OptString(terrane_cap_kv::get_value(
        state,
        app,
        &row_key(&table, &pk),
    )?))
}

fn read_query(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let table = arg(args, 0, "table")?;
    let index = arg(args, 1, "index")?;
    let query_json = joined_arg(args, 2, "queryJson")?;
    let spec = load_table_spec(state, app, &table)?;
    Ok(ReadValue::OptString(Some(query::query_table(
        state,
        app,
        &table,
        &spec,
        &index,
        &query_json,
    )?)))
}

fn read_tables(state: &dyn StateStore, app: &str) -> Result<ReadValue> {
    let mut tables = Vec::new();
    for (_, raw) in
        terrane_cap_kv::scan_prefix(state, app, &format!("{RDB_PREFIX}tables/"), usize::MAX)?
    {
        let value = serde_json::from_str::<Value>(&raw)
            .map_err(|e| Error::Storage(format!("stored table summary is invalid: {e}")))?;
        tables.push(value);
    }
    Ok(ReadValue::OptString(Some(
        serde_json::to_string(&tables)
            .map_err(|e| Error::Storage(format!("serialize tables: {e}")))?,
    )))
}

fn read_spec(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let table = arg(args, 0, "table")?;
    validate_name(&table, "table")?;
    Ok(ReadValue::OptString(terrane_cap_kv::get_value(
        state,
        app,
        &table_spec_key(&table),
    )?))
}

fn load_table_spec(state: &dyn StateStore, app: &str, table: &str) -> Result<TableSpec> {
    validate_name(table, "table")?;
    let Some(raw) = terrane_cap_kv::get_value(state, app, &table_spec_key(table))? else {
        return Err(Error::InvalidInput(format!(
            "table {table:?} is not defined"
        )));
    };
    parse_table_spec(table, &raw)
}

fn table_is_empty(state: &dyn StateStore, app: &str, table: &str) -> Result<bool> {
    Ok(
        terrane_cap_kv::scan_prefix(state, app, &format!("{RDB_PREFIX}row/{table}/"), 1)?
            .is_empty(),
    )
}

#[derive(Default)]
struct IndexEntries {
    secondary: BTreeMap<String, String>,
    unique: BTreeMap<String, String>,
}

fn index_entries(table: &str, spec: &TableSpec, row: &Value) -> Result<IndexEntries> {
    let mut out = IndexEntries::default();
    let pk = encode_primary_key(spec, row)?;
    let pk_json = primary_key_json(spec, row)?;
    for (name, index) in &spec.indexes {
        if let Some((partition, sort)) = index_key_values(row, index)? {
            let projection = project_row(spec, &index.projection, row, &pk_json)?;
            let value = query::index_entry_json(&pk, row, spec, &projection)?;
            out.secondary
                .insert(index_key(table, name, &partition, &sort, &pk), value);
            if index.unique {
                out.unique
                    .insert(unique_key(table, name, &partition), pk.clone());
            }
        }
    }
    for (name, constraint) in &spec.constraints {
        if let ConstraintSpec::Unique { fields, sparse } = constraint {
            if let Some(partition) = unique_constraint_key(row, fields, *sparse)? {
                out.unique.insert(
                    unique_key(table, &format!("constraint_{name}"), &partition),
                    pk.clone(),
                );
            }
        }
    }
    Ok(out)
}

fn index_key_values(row: &Value, index: &IndexSpec) -> Result<Option<(String, String)>> {
    let partition_values = values_from_row(row, &index.partition, index.sparse)?;
    if partition_values.is_empty() {
        return Ok(None);
    }
    let sort_values = values_from_row(row, &index.sort, index.sparse)?;
    if !index.sort.is_empty() && sort_values.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        encode_tuple(&partition_values)?,
        encode_tuple(&sort_values)?,
    )))
}

fn unique_constraint_key(row: &Value, fields: &[String], sparse: bool) -> Result<Option<String>> {
    let parts = fields
        .iter()
        .map(|field| KeyPart::Field(field.clone()))
        .collect::<Vec<_>>();
    let values = values_from_row(row, &parts, sparse)?;
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(encode_tuple(&values)?))
}

fn check_unique_conflicts(
    state: &dyn StateStore,
    app: &str,
    unique: &BTreeMap<String, String>,
    pk: &str,
) -> Result<()> {
    for key in unique.keys() {
        if let Some(existing) = terrane_cap_kv::get_value(state, app, key)? {
            if existing != pk {
                return Err(Error::InvalidInput(format!(
                    "unique constraint conflict at {key}: existing primary key differs"
                )));
            }
        }
    }
    Ok(())
}

fn delete_changed(
    app: &str,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Result<Vec<EventRecord>> {
    old.iter()
        .filter(|(key, value)| new.get(*key) != Some(*value))
        .map(|(key, _)| terrane_cap_kv::delete_event(app.to_string(), key.clone()))
        .collect()
}

fn set_changed(
    app: &str,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Result<Vec<EventRecord>> {
    new.iter()
        .filter(|(key, value)| old.get(*key) != Some(*value))
        .map(|(key, value)| terrane_cap_kv::set_event(app.to_string(), key.clone(), value.clone()))
        .collect()
}

fn joined_arg(args: &[String], index: usize, name: &str) -> Result<String> {
    match args.get(index..) {
        Some(rest) if !rest.is_empty() => Ok(rest.join(" ")),
        _ => Err(Error::InvalidInput(format!("missing argument: {name}"))),
    }
}

#[cfg(test)]
mod tests;
