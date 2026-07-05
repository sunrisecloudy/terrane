use serde_json::Value;
use terrane_cap_interface::{Error, Result};

pub fn eval(expr: &str, value: &Value) -> Result<String> {
    let json = serde_json::to_string(value)
        .map_err(|e| Error::Storage(format!("serialize JMESPath input: {e}")))?;
    let data = jmespath::Variable::from_json(&json)
        .map_err(|e| Error::InvalidInput(format!("JMESPath input is invalid: {e}")))?;
    let compiled = jmespath::compile(expr)
        .map_err(|e| Error::InvalidInput(format!("JMESPath expression is invalid: {e}")))?;
    Ok(compiled
        .search(data)
        .map_err(|e| Error::InvalidInput(format!("JMESPath evaluation failed: {e}")))?
        .to_string())
}
