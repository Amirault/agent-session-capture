//! The Langfuse v2 observations endpoint, read-only, over HTTP.
//!
//! Mirrors the request the official JavaScript SDK sends
//! (`api.observations.getMany`): `GET <host>/api/public/v2/observations` with
//! Basic auth, `fields`, `limit=1000` and a cursor followed until a page
//! carries none; 408/429/5xx retried twice with backoff. Every API or network
//! failure becomes a source access error naming the HTTP status (or
//! "unreachable") and the host — never a key.

use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use super::langfuse_observation_lister::{LangfuseObservation, LangfuseObservationLister, ObservationQuery};
use crate::domain::source_errors::ReadError;
use crate::js::string::js_trim;
use crate::js::{Value, json, path};

const DEFAULT_HOST: &str = "https://cloud.langfuse.com";
const CONTENT_FIELDS: &str = "core,basic,io,metadata,trace_context";
const ACTIVITY_FIELDS: &str = "core,basic,time";
const PAGE_LIMIT: &str = "1000";
const MAX_RETRIES: u32 = 2;
const MISSING_CREDENTIALS: &str = "Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)";

/// Resolved credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LangfuseCredentials {
    pub host: String,
    pub public_key: String,
    pub secret_key: String,
}

/// Where credentials come from, in order.
#[derive(Clone, Debug, Default)]
pub struct LangfuseCredentialOptions {
    /// Environment read first (default: the process environment).
    pub env: Option<HashMap<String, String>>,
    /// `--langfuse-env <file>`: read after the environment.
    pub env_file: Option<String>,
    /// Home holding `.config/langfuse/capture.env`, read last (default `$HOME`).
    pub home_dir: Option<String>,
}

/// Resolve each `LANGFUSE_*` variable from the environment, else the
/// `--langfuse-env` file, else `~/.config/langfuse/capture.env`. The host
/// defaults to Langfuse Cloud; a missing key is a configuration error naming
/// the variables, never a value.
pub fn resolve_langfuse_credentials(options: &LangfuseCredentialOptions) -> Result<LangfuseCredentials, ReadError> {
    let env = options.env.clone().unwrap_or_else(|| std::env::vars().collect());
    let home = options.home_dir.clone().unwrap_or_else(path::home_dir);
    let layers = [
        env,
        options.env_file.as_deref().map(read_env_file).unwrap_or_default(),
        read_env_file(&path::join(&[&home, ".config", "langfuse", "capture.env"])),
    ];
    let value = |name: &str| {
        layers.iter().find_map(|layer| {
            layer
                .get(name)
                .map(|v| js_trim(v).to_string())
                .filter(|v| !v.is_empty())
        })
    };
    let (Some(public_key), Some(secret_key)) = (value("LANGFUSE_PUBLIC_KEY"), value("LANGFUSE_SECRET_KEY")) else {
        return Err(ReadError::SourceConfiguration(MISSING_CREDENTIALS.to_string()));
    };
    Ok(LangfuseCredentials {
        host: value("LANGFUSE_HOST").unwrap_or_else(|| DEFAULT_HOST.to_string()),
        public_key,
        secret_key,
    })
}

/// Dotenv-style `KEY=value` lines (optional `export `, quotes, `#` comments);
/// a missing file is empty.
fn read_env_file(file: &str) -> HashMap<String, String> {
    let mut entries = HashMap::new();
    let Ok(bytes) = fs::read(file) else { return entries };
    for raw_line in String::from_utf8_lossy(&bytes).split('\n') {
        let mut line = js_trim(raw_line);
        if let Some(rest) = line.strip_prefix("export")
            && rest.starts_with(|c: char| c.is_whitespace())
        {
            line = rest.trim_start();
        }
        let Some(separator) = line.find('=') else { continue };
        if line.starts_with('#') || separator == 0 {
            continue;
        }
        let key = js_trim(&line[..separator]).to_string();
        let mut value = js_trim(&line[separator + 1..]);
        for quote in ['\'', '"'] {
            if value.len() >= 2
                && value.starts_with(quote)
                && value.ends_with(quote)
                && !value[1..value.len() - 1].contains('\n')
            {
                value = &value[1..value.len() - 1];
                break;
            }
        }
        entries.insert(key, value.to_string());
    }
    entries
}

/// `base64` of `user:password` for Basic auth.
fn basic_auth(user: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{user}:{password}").into_bytes();
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    format!("Basic {out}")
}

/// Reads observations through the v2 endpoint; credentials resolve on the
/// first call, so building the client needs none.
pub struct LangfuseObservationClient {
    credential_options: LangfuseCredentialOptions,
}

impl LangfuseObservationClient {
    pub fn new(env_file: Option<String>) -> Self {
        Self {
            credential_options: LangfuseCredentialOptions {
                env_file,
                ..LangfuseCredentialOptions::default()
            },
        }
    }

    pub fn with_options(credential_options: LangfuseCredentialOptions) -> Self {
        Self { credential_options }
    }

    fn all_pages(&self, fields: &str, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        let credentials = resolve_langfuse_credentials(&self.credential_options)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .proxy(None)
            .build()
            .into();
        let url = format!("{}/api/public/v2/observations", credentials.host.trim_end_matches('/'));
        let auth = basic_auth(&credentials.public_key, &credentials.secret_key);
        let mut observations = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = fetch_page(&agent, &url, &auth, fields, query, cursor.as_deref(), &credentials.host)?;
            let meta = page.get("meta").and_then(Value::as_object);
            if let Some(Value::Array(data)) = page.get("data") {
                observations.extend(data.iter().filter_map(|o| o.as_object().cloned()));
            }
            match meta.and_then(|m| m.get("cursor")) {
                Some(Value::String(next)) if !next.is_empty() => cursor = Some(next.clone()),
                _ => return Ok(observations),
            }
        }
    }
}

fn unreachable(host: &str, detail: &str) -> ReadError {
    ReadError::SourceAccess(format!("Langfuse API unreachable at {host} ({detail})"))
}

fn fetch_page(
    agent: &ureq::Agent,
    url: &str,
    auth: &str,
    fields: &str,
    query: &ObservationQuery,
    cursor: Option<&str>,
    host: &str,
) -> Result<crate::js::Object, ReadError> {
    let mut attempt = 0;
    loop {
        let mut request = agent
            .get(url)
            .header("Authorization", auth)
            .query("fields", fields)
            .query("limit", PAGE_LIMIT);
        if let Some(cursor) = cursor {
            request = request.query("cursor", cursor);
        }
        if let Some(session_id) = &query.session_id {
            request = request.query("sessionId", session_id);
        }
        if let Some(trace_id) = &query.trace_id {
            request = request.query("traceId", trace_id);
        }
        if let Some(from) = &query.from_start_time {
            request = request.query("fromStartTime", from);
        }
        let mut response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Timeout(_)) => {
                return Err(unreachable(
                    host,
                    "Timeout exceeded when calling GET /api/public/v2/observations.",
                ));
            }
            Err(_) => return Err(unreachable(host, "fetch failed")),
        };
        let status = response.status().as_u16();
        if (status == 408 || status == 429 || status >= 500) && attempt < MAX_RETRIES {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|s| *s > 0);
            let delay = retry_after.map_or(1000 * 2u64.pow(attempt), |s| (s * 1000).min(60_000));
            std::thread::sleep(Duration::from_millis(delay));
            attempt += 1;
            continue;
        }
        if !(200..400).contains(&status) {
            return Err(ReadError::SourceAccess(format!("Langfuse API {status} at {host}")));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_string()
            .map_err(|_| unreachable(host, "fetch failed"))?;
        return match json::parse(&body) {
            Ok(Value::Object(page)) => Ok(page),
            _ => Err(ReadError::SourceAccess(format!("Langfuse API {status} at {host}"))),
        };
    }
}

impl LangfuseObservationLister for LangfuseObservationClient {
    fn list_observations(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        self.all_pages(CONTENT_FIELDS, query)
    }

    fn list_activity(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        self.all_pages(ACTIVITY_FIELDS, query)
    }
}
