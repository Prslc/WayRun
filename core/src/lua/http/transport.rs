use std::collections::BTreeMap;

use super::Response;
use super::options::{Body, Options, timeout_for};

pub(super) fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .proxy(ureq::Proxy::try_from_env())
        .tls_config(tls)
        .build();
    ureq::Agent::new_with_config(config)
}

/// One blocking exchange; free so `get_many` can run several on a thread each.
pub(super) fn perform(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    options: &Options,
    post: bool,
) -> Result<Response, String> {
    let timeout = timeout_for(options.timeout_ms);
    let response = if post {
        let mut request = agent.post(url);
        for (name, value) in &options.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let request = request.config().timeout_global(Some(timeout)).build();
        match &options.body {
            Body::None => request.send_empty(),
            Body::Json(text) => request.content_type("application/json").send(text.clone()),
            Body::Form(pairs) => request.send_form(pairs.clone()),
            Body::Raw(bytes) => request.send(bytes.clone()),
        }
    } else {
        let mut request = agent.get(url);
        for (name, value) in &options.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        request
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call()
    };
    let mut response = response.map_err(|error| format!("http.{method} {url}: {error}"))?;
    let status = response.status().as_u16();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        headers
            .entry(name.as_str().to_string())
            .and_modify(|joined| {
                joined.push_str(", ");
                joined.push_str(&value);
            })
            .or_insert(value);
    }
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| format!("http.{method} {url}: {error}"))?;
    Ok(Response {
        status,
        headers: headers.into_iter().collect(),
        body,
    })
}
