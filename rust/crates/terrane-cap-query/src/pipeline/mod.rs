use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{json, Map, Number, Value};
use terrane_cap_interface::{Error, Result};

pub const MAX_STAGES: usize = 32;
pub const MAX_SCANNED_DOCS: usize = 100_000;
pub const MAX_RESULT_DOCS: usize = 10_000;
pub const MAX_LOOKUP_FOREIGN_SCAN: usize = 100_000;

pub type LookupResolver<'a> = dyn FnMut(&Value) -> Result<Vec<Value>> + 'a;

pub fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&canonical(value))
        .map_err(|e| Error::Storage(format!("serialize canonical JSON: {e}")))
}

pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map.iter().collect::<BTreeMap<_, _>>() {
                out.insert(key.clone(), canonical(value));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub fn cmp_json(a: &Value, b: &Value) -> Ordering {
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Number(a), Value::Number(b)) => number(a).total_cmp(&number(b)),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Array(a), Value::Array(b)) => {
            for (a, b) in a.iter().zip(b) {
                let ord = cmp_json(a, b);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.len().cmp(&b.len())
        }
        (Value::Object(_), Value::Object(_)) => canonical_json(a)
            .unwrap_or_default()
            .cmp(&canonical_json(b).unwrap_or_default()),
        _ => Ordering::Equal,
    })
}

fn rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

pub fn execute_pipeline(
    docs: Vec<Value>,
    pipeline: &[Value],
    lookup: &mut LookupResolver<'_>,
) -> Result<Vec<Value>> {
    if pipeline.len() > MAX_STAGES {
        return Err(Error::InvalidInput(format!(
            "query pipeline has {} stages; limit is {MAX_STAGES}",
            pipeline.len()
        )));
    }
    if docs.len() > MAX_SCANNED_DOCS {
        return Err(Error::InvalidInput(format!(
            "query source scanned {} docs; limit is {MAX_SCANNED_DOCS}",
            docs.len()
        )));
    }
    let mut docs = docs;
    for stage in pipeline {
        docs = execute_stage(docs, stage, lookup)?;
        if docs.len() > MAX_RESULT_DOCS {
            return Err(Error::InvalidInput(format!(
                "query result has {} docs; limit is {MAX_RESULT_DOCS}",
                docs.len()
            )));
        }
    }
    Ok(docs)
}

fn execute_stage(
    docs: Vec<Value>,
    stage: &Value,
    lookup: &mut LookupResolver<'_>,
) -> Result<Vec<Value>> {
    let obj = stage.as_object().ok_or_else(|| {
        Error::InvalidInput("pipeline stage must be a one-key JSON object".into())
    })?;
    if obj.len() != 1 {
        return Err(Error::InvalidInput(
            "pipeline stage must contain exactly one stage operator".into(),
        ));
    }
    let (name, spec) = obj.iter().next().ok_or_else(|| {
        Error::InvalidInput("pipeline stage must contain a stage operator".into())
    })?;
    match name.as_str() {
        "$match" => docs
            .into_iter()
            .filter_map(|doc| match matches_doc(&doc, spec) {
                Ok(true) => Some(Ok(doc)),
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            })
            .collect(),
        "$project" => docs.into_iter().map(|doc| project_doc(&doc, spec)).collect(),
        "$addFields" => docs.into_iter().map(|doc| add_fields(&doc, spec)).collect(),
        "$unset" => docs.into_iter().map(|doc| unset_fields(doc, spec)).collect(),
        "$unwind" => unwind(docs, spec),
        "$group" => group(docs, spec),
        "$sort" => sort_docs(docs, spec),
        "$skip" => Ok(docs.into_iter().skip(as_usize(spec, "$skip")?).collect()),
        "$limit" => Ok(docs.into_iter().take(as_usize(spec, "$limit")?).collect()),
        "$count" => {
            let field = spec.as_str().ok_or_else(|| {
                Error::InvalidInput("$count requires an output field name".into())
            })?;
            Ok(vec![json!({ field: docs.len() as i64 })])
        }
        "$replaceRoot" => replace_root(docs, spec),
        "$lookup" => lookup_stage(docs, spec, lookup),
        other => Err(Error::InvalidInput(format!(
            "unsupported query pipeline stage {other}; supported stages: $match, $project, $addFields, $unset, $unwind, $group, $sort, $skip, $limit, $count, $replaceRoot, $lookup"
        ))),
    }
}

fn matches_doc(doc: &Value, spec: &Value) -> Result<bool> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$match requires an object".into()))?;
    for (key, expected) in obj {
        if key == "$expr" {
            if !truthy(&eval_expr(expected, doc, None)?) {
                return Ok(false);
            }
            continue;
        }
        let actual = field_path(doc, key).unwrap_or(&Value::Null);
        if expected
            .as_object()
            .is_some_and(|m| m.keys().any(|k| k.starts_with('$')))
        {
            if !match_operators(actual, expected, doc)? {
                return Ok(false);
            }
        } else if actual != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn match_operators(actual: &Value, expected: &Value, root: &Value) -> Result<bool> {
    let obj = expected
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$match operator spec must be an object".into()))?;
    for (op, rhs) in obj {
        let rhs = eval_expr(rhs, root, None)?;
        let pass = match op.as_str() {
            "$eq" => actual == &rhs,
            "$ne" => actual != &rhs,
            "$gt" => cmp_json(actual, &rhs) == Ordering::Greater,
            "$gte" => !matches!(cmp_json(actual, &rhs), Ordering::Less),
            "$lt" => cmp_json(actual, &rhs) == Ordering::Less,
            "$lte" => !matches!(cmp_json(actual, &rhs), Ordering::Greater),
            "$in" => rhs
                .as_array()
                .is_some_and(|items| items.iter().any(|v| v == actual)),
            other => return Err(unsupported_operator(other)),
        };
        if !pass {
            return Ok(false);
        }
    }
    Ok(true)
}

fn project_doc(doc: &Value, spec: &Value) -> Result<Value> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$project requires an object".into()))?;
    let mut out = Map::new();
    let include_mode = obj
        .values()
        .any(|v| matches!(v, Value::Bool(true)) || v.as_i64() == Some(1));
    if !include_mode {
        out = doc.as_object().cloned().unwrap_or_default();
    }
    for (field, expr) in obj {
        if matches!(expr, Value::Bool(false)) || expr.as_i64() == Some(0) {
            out.remove(field);
        } else if matches!(expr, Value::Bool(true)) || expr.as_i64() == Some(1) {
            out.insert(
                field.clone(),
                field_path(doc, field).cloned().unwrap_or(Value::Null),
            );
        } else {
            out.insert(field.clone(), eval_expr(expr, doc, None)?);
        }
    }
    Ok(Value::Object(out))
}

fn add_fields(doc: &Value, spec: &Value) -> Result<Value> {
    let mut out = doc.as_object().cloned().unwrap_or_default();
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$addFields requires an object".into()))?;
    for (field, expr) in obj {
        out.insert(field.clone(), eval_expr(expr, doc, None)?);
    }
    Ok(Value::Object(out))
}

fn unset_fields(doc: Value, spec: &Value) -> Result<Value> {
    let mut out = doc.as_object().cloned().unwrap_or_default();
    match spec {
        Value::String(field) => {
            out.remove(field);
        }
        Value::Array(fields) => {
            for field in fields {
                let Some(field) = field.as_str() else {
                    return Err(Error::InvalidInput(
                        "$unset array values must be strings".into(),
                    ));
                };
                out.remove(field);
            }
        }
        _ => {
            return Err(Error::InvalidInput(
                "$unset requires a string or string array".into(),
            ))
        }
    }
    Ok(Value::Object(out))
}

fn unwind(docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let path = match spec {
        Value::String(path) => path.trim_start_matches('$'),
        Value::Object(map) => map
            .get("path")
            .and_then(Value::as_str)
            .map(|s| s.trim_start_matches('$'))
            .ok_or_else(|| Error::InvalidInput("$unwind.path must be a field path".into()))?,
        _ => return Err(Error::InvalidInput("$unwind requires a field path".into())),
    };
    let mut out = Vec::new();
    for doc in docs {
        let Some(items) = field_path(&doc, path).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let mut next = doc.as_object().cloned().unwrap_or_default();
            next.insert(path.to_string(), item.clone());
            out.push(Value::Object(next));
        }
    }
    Ok(out)
}

#[derive(Clone)]
enum Acc {
    Sum(f64),
    Avg { sum: f64, count: usize },
    Min(Option<Value>),
    Max(Option<Value>),
    Count(usize),
    First(Option<Value>),
    Last(Option<Value>),
    Push(Vec<Value>),
    AddToSet(BTreeMap<String, Value>),
}

fn group(docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$group requires an object".into()))?;
    let id_expr = obj
        .get("_id")
        .ok_or_else(|| Error::InvalidInput("$group requires _id".into()))?;
    let acc_template = init_accs(obj)?;
    let mut groups: BTreeMap<String, (Value, BTreeMap<String, Acc>)> = BTreeMap::new();
    for doc in &docs {
        let id = eval_expr(id_expr, doc, None)?;
        let key = canonical_json(&id)?;
        let (_, accs) = groups
            .entry(key)
            .or_insert_with(|| (id, acc_template.clone()));
        for (field, acc) in accs {
            apply_acc(field, acc, obj, doc)?;
        }
    }
    let mut out = Vec::new();
    for (_, (id, accs)) in groups {
        let mut row = Map::new();
        row.insert("_id".to_string(), id);
        for (field, acc) in accs {
            row.insert(field, finish_acc(acc)?);
        }
        out.push(Value::Object(row));
    }
    Ok(out)
}

fn init_accs(obj: &Map<String, Value>) -> Result<BTreeMap<String, Acc>> {
    let mut out = BTreeMap::new();
    for (field, spec) in obj {
        if field == "_id" {
            continue;
        }
        let (op, _) = one_op(spec, "$group accumulator")?;
        let acc = match op {
            "$sum" => Acc::Sum(0.0),
            "$avg" => Acc::Avg { sum: 0.0, count: 0 },
            "$min" => Acc::Min(None),
            "$max" => Acc::Max(None),
            "$count" => Acc::Count(0),
            "$first" => Acc::First(None),
            "$last" => Acc::Last(None),
            "$push" => Acc::Push(Vec::new()),
            "$addToSet" => Acc::AddToSet(BTreeMap::new()),
            other => return Err(unsupported_operator(other)),
        };
        out.insert(field.clone(), acc);
    }
    Ok(out)
}

fn apply_acc(field: &str, acc: &mut Acc, obj: &Map<String, Value>, doc: &Value) -> Result<()> {
    let spec = obj
        .get(field)
        .ok_or_else(|| Error::Storage(format!("missing accumulator spec for {field}")))?;
    let (_, expr) = one_op(spec, "$group accumulator")?;
    match acc {
        Acc::Sum(sum) => *sum += numeric(&eval_expr(expr, doc, None)?, "$sum")?,
        Acc::Avg { sum, count } => {
            *sum += numeric(&eval_expr(expr, doc, None)?, "$avg")?;
            *count += 1;
        }
        Acc::Min(current) => {
            let value = eval_expr(expr, doc, None)?;
            if current
                .as_ref()
                .is_none_or(|old| cmp_json(&value, old) == Ordering::Less)
            {
                *current = Some(value);
            }
        }
        Acc::Max(current) => {
            let value = eval_expr(expr, doc, None)?;
            if current
                .as_ref()
                .is_none_or(|old| cmp_json(&value, old) == Ordering::Greater)
            {
                *current = Some(value);
            }
        }
        Acc::Count(count) => *count += 1,
        Acc::First(value) => {
            if value.is_none() {
                *value = Some(eval_expr(expr, doc, None)?);
            }
        }
        Acc::Last(value) => *value = Some(eval_expr(expr, doc, None)?),
        Acc::Push(values) => values.push(eval_expr(expr, doc, None)?),
        Acc::AddToSet(values) => {
            let value = eval_expr(expr, doc, None)?;
            values.insert(canonical_json(&value)?, value);
        }
    }
    Ok(())
}

fn finish_acc(acc: Acc) -> Result<Value> {
    match acc {
        Acc::Sum(sum) => number_value(sum),
        Acc::Avg { sum, count } => {
            if count == 0 {
                Ok(Value::Null)
            } else {
                number_value(sum / count as f64)
            }
        }
        Acc::Min(value) | Acc::Max(value) | Acc::First(value) | Acc::Last(value) => {
            Ok(value.unwrap_or(Value::Null))
        }
        Acc::Count(count) => Ok(json!(count as i64)),
        Acc::Push(values) => Ok(Value::Array(values)),
        Acc::AddToSet(values) => Ok(Value::Array(values.into_values().collect())),
    }
}

fn sort_docs(mut docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$sort requires an object".into()))?;
    let fields: Vec<(String, i64)> = obj
        .iter()
        .map(|(k, v)| Ok((k.clone(), v.as_i64().unwrap_or(1))))
        .collect::<Result<_>>()?;
    docs.sort_by(|a, b| {
        for (field, dir) in &fields {
            let ord = cmp_json(
                field_path(a, field).unwrap_or(&Value::Null),
                field_path(b, field).unwrap_or(&Value::Null),
            );
            if ord != Ordering::Equal {
                return if *dir < 0 { ord.reverse() } else { ord };
            }
        }
        Ordering::Equal
    });
    Ok(docs)
}

fn replace_root(docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let expr = spec
        .as_object()
        .and_then(|m| m.get("newRoot"))
        .unwrap_or(spec);
    docs.into_iter()
        .map(|doc| {
            let value = eval_expr(expr, &doc, None)?;
            if value.is_object() {
                Ok(value)
            } else {
                Err(Error::InvalidInput(
                    "$replaceRoot new root must evaluate to an object".into(),
                ))
            }
        })
        .collect()
}

fn lookup_stage(
    docs: Vec<Value>,
    spec: &Value,
    lookup: &mut LookupResolver<'_>,
) -> Result<Vec<Value>> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$lookup requires an object".into()))?;
    let from = obj
        .get("from")
        .ok_or_else(|| Error::InvalidInput("$lookup.from is required".into()))?;
    let local = obj
        .get("localField")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("$lookup.localField is required".into()))?;
    let foreign = obj
        .get("foreignField")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("$lookup.foreignField is required".into()))?;
    let as_field = obj
        .get("as")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("$lookup.as is required".into()))?;
    let foreign_docs = lookup(from)?;
    if foreign_docs.len() > MAX_LOOKUP_FOREIGN_SCAN {
        return Err(Error::InvalidInput(format!(
            "$lookup foreign scan has {} docs; limit is {MAX_LOOKUP_FOREIGN_SCAN}",
            foreign_docs.len()
        )));
    }
    let mut out = Vec::new();
    for doc in docs {
        let local_value = field_path(&doc, local).unwrap_or(&Value::Null);
        let matches: Vec<Value> = foreign_docs
            .iter()
            .filter(|candidate| {
                field_path(candidate, foreign).unwrap_or(&Value::Null) == local_value
            })
            .cloned()
            .collect();
        let mut obj = doc.as_object().cloned().unwrap_or_default();
        obj.insert(as_field.to_string(), Value::Array(matches));
        out.push(Value::Object(obj));
    }
    Ok(out)
}

fn eval_expr(expr: &Value, root: &Value, this: Option<&Value>) -> Result<Value> {
    match expr {
        Value::String(s) if s == "$$ROOT" => Ok(root.clone()),
        Value::String(s) if s == "$$this" => Ok(this.unwrap_or(&Value::Null).clone()),
        Value::String(s) if s.starts_with('$') => {
            Ok(field_path(root, &s[1..]).cloned().unwrap_or(Value::Null))
        }
        Value::Array(items) => items
            .iter()
            .map(|v| eval_expr(v, root, this))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(map)
            if map.len() == 1 && map.keys().next().is_some_and(|k| k.starts_with('$')) =>
        {
            let (op, arg) = map
                .iter()
                .next()
                .ok_or_else(|| Error::InvalidInput("empty expression operator".into()))?;
            eval_op(op, arg, root, this)
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), eval_expr(v, root, this)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn eval_op(op: &str, arg: &Value, root: &Value, this: Option<&Value>) -> Result<Value> {
    let args = || -> Result<Vec<Value>> {
        arg.as_array()
            .ok_or_else(|| Error::InvalidInput(format!("{op} requires an array argument")))?
            .iter()
            .map(|v| eval_expr(v, root, this))
            .collect()
    };
    match op {
        "$eq" | "$ne" | "$gt" | "$gte" | "$lt" | "$lte" | "$in" => {
            let values = args()?;
            if values.len() != 2 {
                return Err(Error::InvalidInput(format!("{op} requires two arguments")));
            }
            let b = match op {
                "$eq" => values[0] == values[1],
                "$ne" => values[0] != values[1],
                "$gt" => cmp_json(&values[0], &values[1]) == Ordering::Greater,
                "$gte" => !matches!(cmp_json(&values[0], &values[1]), Ordering::Less),
                "$lt" => cmp_json(&values[0], &values[1]) == Ordering::Less,
                "$lte" => !matches!(cmp_json(&values[0], &values[1]), Ordering::Greater),
                "$in" => values[1].as_array().is_some_and(|a| a.contains(&values[0])),
                _ => false,
            };
            Ok(Value::Bool(b))
        }
        "$and" => Ok(Value::Bool(args()?.iter().all(truthy))),
        "$or" => Ok(Value::Bool(args()?.iter().any(truthy))),
        "$not" => Ok(Value::Bool(!truthy(&eval_expr(arg, root, this)?))),
        "$add" => number_value(
            args()?
                .iter()
                .map(|v| numeric(v, "$add"))
                .sum::<Result<f64>>()?,
        ),
        "$subtract" | "$multiply" | "$divide" | "$mod" => {
            let values = args()?;
            if values.len() != 2 {
                return Err(Error::InvalidInput(format!("{op} requires two arguments")));
            }
            let a = numeric(&values[0], op)?;
            let b = numeric(&values[1], op)?;
            let value = match op {
                "$subtract" => a - b,
                "$multiply" => a * b,
                "$divide" => a / b,
                "$mod" => a % b,
                _ => a,
            };
            number_value(value)
        }
        "$abs" => number_value(numeric(&eval_expr(arg, root, this)?, op)?.abs()),
        "$floor" => number_value(numeric(&eval_expr(arg, root, this)?, op)?.floor()),
        "$ceil" => number_value(numeric(&eval_expr(arg, root, this)?, op)?.ceil()),
        "$round" => number_value(numeric(&eval_expr(arg, root, this)?, op)?.round()),
        "$concat" => Ok(Value::String(args()?.iter().map(stringify).collect())),
        "$toLower" => Ok(Value::String(
            stringify(&eval_expr(arg, root, this)?).to_lowercase(),
        )),
        "$toUpper" => Ok(Value::String(
            stringify(&eval_expr(arg, root, this)?).to_uppercase(),
        )),
        "$substr" => {
            let values = args()?;
            if values.len() != 3 {
                return Err(Error::InvalidInput(
                    "$substr requires string, start, length".into(),
                ));
            }
            let s = stringify(&values[0]);
            let start = numeric(&values[1], "$substr")? as usize;
            let len = numeric(&values[2], "$substr")? as usize;
            Ok(Value::String(s.chars().skip(start).take(len).collect()))
        }
        "$split" => {
            let values = args()?;
            if values.len() != 2 {
                return Err(Error::InvalidInput(
                    "$split requires string and delimiter".into(),
                ));
            }
            Ok(Value::Array(
                stringify(&values[0])
                    .split(&stringify(&values[1]))
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ))
        }
        "$strLen" => Ok(json!(
            stringify(&eval_expr(arg, root, this)?).chars().count() as i64
        )),
        "$trim" => Ok(Value::String(
            stringify(&eval_expr(arg, root, this)?).trim().to_string(),
        )),
        "$size" => Ok(json!(
            eval_expr(arg, root, this)?.as_array().map_or(0, Vec::len) as i64
        )),
        "$arrayElemAt" => {
            let values = args()?;
            if values.len() != 2 {
                return Err(Error::InvalidInput(
                    "$arrayElemAt requires array and index".into(),
                ));
            }
            let idx = numeric(&values[1], "$arrayElemAt")? as usize;
            Ok(values[0]
                .as_array()
                .and_then(|a| a.get(idx))
                .cloned()
                .unwrap_or(Value::Null))
        }
        "$slice" => {
            let values = args()?;
            let arr = values
                .first()
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let start = values
                .get(1)
                .map(|v| numeric(v, "$slice"))
                .transpose()?
                .unwrap_or(0.0) as usize;
            let len = values
                .get(2)
                .map(|v| numeric(v, "$slice"))
                .transpose()?
                .unwrap_or(arr.len() as f64) as usize;
            Ok(Value::Array(
                arr.into_iter().skip(start).take(len).collect(),
            ))
        }
        "$filter" | "$map" => map_or_filter(op, arg, root),
        "$cond" => cond(arg, root, this),
        "$ifNull" => {
            let values = args()?;
            Ok(if values.first().is_none_or(Value::is_null) {
                values.get(1).cloned().unwrap_or(Value::Null)
            } else {
                values[0].clone()
            })
        }
        "$switch" => switch(arg, root, this),
        "$toString" => Ok(Value::String(stringify(&eval_expr(arg, root, this)?))),
        "$toInt" => Ok(json!(
            numeric(&eval_expr(arg, root, this)?, "$toInt")? as i64
        )),
        "$toDouble" => number_value(numeric(&eval_expr(arg, root, this)?, "$toDouble")?),
        "$toBool" => Ok(Value::Bool(truthy(&eval_expr(arg, root, this)?))),
        "$type" => Ok(Value::String(
            match eval_expr(arg, root, this)? {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            }
            .to_string(),
        )),
        other => Err(unsupported_operator(other)),
    }
}

fn map_or_filter(op: &str, arg: &Value, root: &Value) -> Result<Value> {
    let obj = arg
        .as_object()
        .ok_or_else(|| Error::InvalidInput(format!("{op} requires an object")))?;
    let input = eval_expr(obj.get("input").unwrap_or(&Value::Null), root, None)?;
    let items = input.as_array().cloned().unwrap_or_default();
    let expr = obj
        .get(if op == "$map" { "in" } else { "cond" })
        .ok_or_else(|| Error::InvalidInput(format!("{op} missing expression")))?;
    let mut out = Vec::new();
    for item in items {
        let value = eval_expr(expr, root, Some(&item))?;
        if op == "$map" {
            out.push(value);
        } else if truthy(&value) {
            out.push(item);
        }
    }
    Ok(Value::Array(out))
}

fn cond(arg: &Value, root: &Value, this: Option<&Value>) -> Result<Value> {
    if let Some(items) = arg.as_array() {
        if items.len() != 3 {
            return Err(Error::InvalidInput(
                "$cond array requires if, then, else".into(),
            ));
        }
        return if truthy(&eval_expr(&items[0], root, this)?) {
            eval_expr(&items[1], root, this)
        } else {
            eval_expr(&items[2], root, this)
        };
    }
    let obj = arg
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$cond requires array or object".into()))?;
    if truthy(&eval_expr(
        obj.get("if").unwrap_or(&Value::Null),
        root,
        this,
    )?) {
        eval_expr(obj.get("then").unwrap_or(&Value::Null), root, this)
    } else {
        eval_expr(obj.get("else").unwrap_or(&Value::Null), root, this)
    }
}

fn switch(arg: &Value, root: &Value, this: Option<&Value>) -> Result<Value> {
    let obj = arg
        .as_object()
        .ok_or_else(|| Error::InvalidInput("$switch requires an object".into()))?;
    for branch in obj
        .get("branches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let branch = branch
            .as_object()
            .ok_or_else(|| Error::InvalidInput("$switch branches must be objects".into()))?;
        if truthy(&eval_expr(
            branch.get("case").unwrap_or(&Value::Null),
            root,
            this,
        )?) {
            return eval_expr(branch.get("then").unwrap_or(&Value::Null), root, this);
        }
    }
    eval_expr(obj.get("default").unwrap_or(&Value::Null), root, this)
}

fn one_op<'a>(value: &'a Value, label: &str) -> Result<(&'a str, &'a Value)> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput(format!("{label} must be an object")))?;
    if obj.len() != 1 {
        return Err(Error::InvalidInput(format!(
            "{label} must contain exactly one operator"
        )));
    }
    obj.iter()
        .next()
        .map(|(k, v)| (k.as_str(), v))
        .ok_or_else(|| Error::InvalidInput(format!("{label} must contain an operator")))
}

fn field_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(doc);
    }
    let mut current = doc;
    for part in path.split('.') {
        current = current.as_object()?.get(part)?;
    }
    Some(current)
}

fn as_usize(value: &Value, label: &str) -> Result<usize> {
    value
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::InvalidInput(format!("{label} requires a positive integer")))
}

fn numeric(value: &Value, op: &str) -> Result<f64> {
    let n = match value {
        Value::Number(n) => number(n),
        Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| Error::InvalidInput(format!("{op} expected numeric value")))?,
        _ => return Err(Error::InvalidInput(format!("{op} expected numeric value"))),
    };
    if n.is_finite() {
        Ok(n)
    } else {
        Err(Error::InvalidInput(format!(
            "{op} produced non-finite number"
        )))
    }
}

fn number(n: &Number) -> f64 {
    n.as_f64().unwrap_or(0.0)
}

fn number_value(value: f64) -> Result<Value> {
    if !value.is_finite() {
        return Err(Error::InvalidInput(
            "query numeric operation produced NaN or infinity".into(),
        ));
    }
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        Ok(json!(value as i64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| Error::InvalidInput("query number is not representable".into()))
    }
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
        && !matches!(value, Value::Array(a) if a.is_empty())
        && !matches!(value, Value::Object(o) if o.is_empty())
}

fn stringify(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn unsupported_operator(op: &str) -> Error {
    Error::InvalidInput(format!(
        "unsupported query expression operator {op}; supported operators include comparison, boolean, arithmetic, string, array, conditional, type, field paths, $$ROOT, and $$this"
    ))
}
