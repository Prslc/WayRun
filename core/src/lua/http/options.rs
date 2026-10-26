use std::time::Duration;

use mlua::{Lua, LuaSerdeExt, Table, Value};

#[derive(Default)]
pub(super) struct Options {
    pub timeout_ms: Option<u64>,
    pub ttl: Option<u64>,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

#[derive(Default)]
pub(super) enum Body {
    #[default]
    None,
    Json(String),
    Form(Vec<(String, String)>),
    Raw(Vec<u8>),
}

/// The deadline: two seconds by default, held inside the host call's own
/// ceiling so an overlong timeout fails here rather than as a killed host.
pub(super) fn timeout_for(millis: Option<u64>) -> Duration {
    let ceiling =
        crate::provider::external::HOST_TIMEOUT.saturating_sub(Duration::from_millis(500));
    Duration::from_millis(millis.unwrap_or(2_000)).clamp(Duration::from_millis(100), ceiling)
}

/// The request URL with `params` appended, plus the headers those params may
/// carry under the reserved `headers` name the launcher's plugins already use.
pub(super) fn with_params(
    url: String,
    params: Option<Table>,
) -> mlua::Result<(String, Vec<(String, String)>)> {
    let mut url = url;
    let mut headers = Vec::new();
    let Some(params) = params else {
        return Ok((url, headers));
    };
    let mut query = Vec::new();
    for pair in params.pairs::<String, Value>() {
        let (name, value) = pair?;
        if name == "headers" {
            headers = string_pairs(&value, "headers")?;
            continue;
        }
        let value = match value {
            Value::Integer(number) => number.to_string(),
            Value::Number(number) => number.to_string(),
            Value::String(text) => text.to_string_lossy(),
            Value::Boolean(flag) => flag.to_string(),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "http params must be scalars, got {other:?}"
                )));
            }
        };
        query.push((name, value));
    }
    // A Lua table's iteration order is unspecified; sorting keeps the URL (and
    // so the ttl cache key) stable across processes.
    query.sort();
    let mut separator = if url.contains('?') { '&' } else { '?' };
    for (name, value) in query {
        url.push(separator);
        separator = '&';
        url.push_str(&urlencoding::encode(&name));
        url.push('=');
        url.push_str(&urlencoding::encode(&value));
    }
    Ok((url, headers))
}

pub(super) fn parse_options(
    lua: &Lua,
    value: Option<Value>,
    method: &str,
) -> mlua::Result<Options> {
    let mut options = Options::default();
    match value {
        None | Some(Value::Nil) => {}
        Some(Value::Integer(millis)) => options.timeout_ms = Some(millis.max(0) as u64),
        Some(Value::Number(millis)) => options.timeout_ms = Some(millis.max(0.0) as u64),
        Some(Value::Table(table)) => {
            let mut bodies = 0;
            for pair in table.pairs::<String, Value>() {
                let (name, value) = pair?;
                match name.as_str() {
                    "timeout_ms" => options.timeout_ms = Some(number(&value, "timeout_ms")?),
                    "ttl" => options.ttl = Some(number(&value, "ttl")?),
                    "headers" => options.headers = string_pairs(&value, "headers")?,
                    "json" => {
                        bodies += 1;
                        let json: serde_json::Value = lua.from_value(value).map_err(|error| {
                            mlua::Error::RuntimeError(format!("http json body: {error}"))
                        })?;
                        let text = serde_json::to_string(&json).map_err(|error| {
                            mlua::Error::RuntimeError(format!("http json body: {error}"))
                        })?;
                        options.body = Body::Json(text);
                    }
                    "form" => {
                        bodies += 1;
                        options.body = Body::Form(string_pairs(&value, "form")?);
                    }
                    "body" => {
                        bodies += 1;
                        let Value::String(text) = value else {
                            return Err(mlua::Error::RuntimeError(format!(
                                "http body must be a string, got {value:?}"
                            )));
                        };
                        options.body = Body::Raw(text.as_bytes().to_vec());
                    }
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "http options carry no {other:?}"
                        )));
                    }
                }
            }
            if bodies > 1 {
                return Err(mlua::Error::RuntimeError(
                    "http options: json, form and body are mutually exclusive".to_string(),
                ));
            }
            if bodies > 0 && method != "post" {
                return Err(mlua::Error::RuntimeError(
                    "http.get sends no body; use http.post".to_string(),
                ));
            }
            if options.ttl.is_some() && method == "post" {
                return Err(mlua::Error::RuntimeError(
                    "http.post is never cached".to_string(),
                ));
            }
        }
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "http options must be a table or a timeout in ms, got {other:?}"
            )));
        }
    }
    Ok(options)
}

fn number(value: &Value, name: &str) -> mlua::Result<u64> {
    match value {
        Value::Integer(number) => Ok((*number).max(0) as u64),
        Value::Number(number) => Ok(number.max(0.0) as u64),
        other => Err(mlua::Error::RuntimeError(format!(
            "http {name} must be a number, got {other:?}"
        ))),
    }
}

fn string_pairs(value: &Value, name: &str) -> mlua::Result<Vec<(String, String)>> {
    let Value::Table(table) = value else {
        return Err(mlua::Error::RuntimeError(format!(
            "http {name} must be a table, got {value:?}"
        )));
    };
    let mut pairs = Vec::new();
    for pair in table.pairs::<String, String>() {
        pairs.push(pair?);
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_are_clamped_inside_the_host_ceiling() {
        let ceiling = crate::provider::external::HOST_TIMEOUT - Duration::from_millis(500);
        assert_eq!(timeout_for(None), Duration::from_millis(2_000));
        assert_eq!(timeout_for(Some(50)), Duration::from_millis(100));
        assert_eq!(timeout_for(Some(3_000)), Duration::from_millis(3_000));
        assert_eq!(timeout_for(Some(60_000)), ceiling);
    }

    #[test]
    fn query_parameters_are_sorted() {
        let lua = Lua::new();
        let params: Table = lua
            .load(r#"return { b = 2, a = 1, c = "x y" }"#)
            .eval()
            .unwrap();
        let (url, _) = with_params("https://x/a".to_string(), Some(params)).unwrap();
        assert_eq!(url, "https://x/a?a=1&b=2&c=x%20y");
    }
}
