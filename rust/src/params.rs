//! Path, query and JSON body validation shared by the HTTP adapters.
use crate::{Query, api_error};
use anyhow::Result;
use serde_json::Value;

/// Existing endpoints expose different messages for out-of-range query values.
pub(crate) enum RangeError {
    Invalid,
    Bounds,
}

pub(crate) fn id(raw: &str) -> Result<i64> {
    positive_id(raw, || "Invalid identifier".to_owned())
}
pub(crate) fn query_integer(
    q: &Query,
    key: &str,
    default: i64,
    min: i64,
    max: i64,
    range_error: RangeError,
) -> Result<i64> {
    let n = match q.get(key) {
        Some(v) => v
            .parse()
            .map_err(|_| api_error(422, format!("Invalid {key}")))?,
        None => default,
    };
    if !(min..=max).contains(&n) {
        return Err(api_error(
            422,
            match range_error {
                RangeError::Invalid => format!("Invalid {key}"),
                RangeError::Bounds => format!("{key} must be between {min} and {max}"),
            },
        ));
    }
    Ok(n)
}
pub(crate) fn bool_value(value: &Value, key: &str, default: bool) -> Result<bool> {
    match value.get(key) {
        None => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(api_error(422, format!("{key} must be true or false"))),
    }
}
pub(crate) fn query_bool(q: &Query, key: &str) -> Result<Option<bool>> {
    q.get(key)
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(api_error(422, format!("Invalid {key}"))),
        })
        .transpose()
}
pub(crate) fn depth(body: &Value) -> Result<Option<i64>> {
    match body.get("depth") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .filter(|v| (6..=30).contains(v))
            .map(Some)
            .ok_or_else(|| api_error(422, "Depth must be between 6 and 30")),
    }
}
pub(crate) fn object(body: &Value) -> Result<()> {
    if !body.is_object() {
        return Err(api_error(422, "Expected a JSON object"));
    }
    Ok(())
}

pub(crate) fn optional_string<'a>(
    body: &'a Value,
    key: &str,
    limit: usize,
) -> anyhow::Result<Option<&'a str>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= limit => Ok(Some(value)),
        _ => Err(api_error(
            422,
            format!("{key} must be a string of at most {limit} characters"),
        )),
    }
}
pub(crate) fn required_string<'a>(
    body: &'a Value,
    key: &str,
    limit: usize,
) -> anyhow::Result<&'a str> {
    optional_string(body, key, limit)?.ok_or_else(|| api_error(422, format!("{key} is required")))
}
pub(crate) fn body_integer(
    body: &Value,
    key: &str,
    default: i64,
    min: i64,
    max: i64,
) -> anyhow::Result<i64> {
    let value = match body.get(key) {
        None | Some(Value::Null) => default,
        Some(value) => value
            .as_i64()
            .ok_or_else(|| api_error(422, format!("{key} must be an integer")))?,
    };
    if !(min..=max).contains(&value) {
        return Err(api_error(
            422,
            format!("{key} must be between {min} and {max}"),
        ));
    }
    Ok(value)
}

pub(crate) fn filter<'a>(q: &'a Query, key: &str, allowed: &[&str]) -> Result<Option<&'a str>> {
    match q.get(key).map(String::as_str) {
        None | Some("") | Some("all") => Ok(None),
        Some(value) if allowed.contains(&value) => Ok(Some(value)),
        Some(_) => Err(api_error(422, format!("Invalid {key}"))),
    }
}

pub(crate) fn named_id(raw: &str, name: &str) -> Result<i64> {
    positive_id(raw, || format!("{name} must be a positive integer"))
}

fn positive_id(raw: &str, message: impl FnOnce() -> String) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| api_error(422, message()))
}
