//! A loopback stand-in for the Langfuse v2 observations endpoint (never the
//! real API): serves `observations` in pages of `page_size` behind an opaque
//! cursor, filtered as the v2 endpoint does (`traceId` and `sessionId` exact,
//! `fromStartTime` on `startTime`), or answers every request with `fail_with`.
//! The last page's `meta` is `last_page_meta` (Langfuse Cloud answers `{}`).
//! Past `max_requests` requests it answers 418. Also included by the CLI
//! integration tests.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

pub const TEST_PUBLIC_KEY: &str = "test-public-key";
pub const TEST_SECRET_KEY: &str = "test-secret-key";
/// `Basic base64(test-public-key:test-secret-key)`.
pub const EXPECTED_AUTH: &str = "Basic dGVzdC1wdWJsaWMta2V5OnRlc3Qtc2VjcmV0LWtleQ==";

/// One request the fake received.
#[derive(Clone, Debug, Default)]
pub struct RecordedRequest {
    pub path: String,
    pub query: Vec<(String, String)>,
    pub authorized: bool,
}

impl RecordedRequest {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

pub struct FakeServerOptions {
    /// Observations as JSON object texts.
    pub observations: Vec<String>,
    pub page_size: usize,
    pub fail_with: Option<u16>,
    pub last_page_meta: String,
    pub max_requests: usize,
}

impl Default for FakeServerOptions {
    fn default() -> Self {
        Self {
            observations: vec![],
            page_size: 1000,
            fail_with: None,
            last_page_meta: "{}".to_string(),
            max_requests: 100,
        }
    }
}

pub struct FakeLangfuseServer {
    pub url: String,
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl FakeLangfuseServer {
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock").clone()
    }
}

fn decode(component: &str) -> String {
    let bytes = component.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(&component[i + 1..i + 3], 16).unwrap_or(b'?'));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn field(observation: &str, key: &str) -> Option<String> {
    let parsed = crate_json_parse(observation);
    parsed.into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The string fields of a flat observation JSON object (enough for filtering).
fn crate_json_parse(text: &str) -> Vec<(String, String)> {
    let value = crate::js::json::parse(text).expect("observation JSON");
    value
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

pub fn start_fake_langfuse(options: FakeServerOptions) -> FakeLangfuseServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::default();
    let recorded = requests.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut authorization = String::new();
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    authorization = value.trim().to_string();
                }
            }
            let target = request_line.split(' ').nth(1).unwrap_or("/").to_string();
            let (path, query_string) = target.split_once('?').unwrap_or((&target, ""));
            let query: Vec<(String, String)> = query_string
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    (decode(k), decode(v))
                })
                .collect();
            let request = RecordedRequest {
                path: path.to_string(),
                query,
                authorized: authorization == EXPECTED_AUTH,
            };
            let count = {
                let mut all = recorded.lock().expect("lock");
                all.push(request.clone());
                all.len()
            };
            let status = options
                .fail_with
                .or(if count > options.max_requests { Some(418) } else { None });
            let (status, body) = match status {
                Some(status) => (status, "{\"message\":\"Invalid credentials\"}".to_string()),
                None => {
                    let matching: Vec<&String> = options
                        .observations
                        .iter()
                        .filter(|o| {
                            request
                                .param("traceId")
                                .is_none_or(|t| field(o, "traceId").as_deref() == Some(t))
                                && request
                                    .param("sessionId")
                                    .is_none_or(|s| field(o, "sessionId").as_deref() == Some(s))
                                && request
                                    .param("fromStartTime")
                                    .is_none_or(|f| field(o, "startTime").is_some_and(|st| st.as_str() >= f))
                        })
                        .collect();
                    let offset: usize = request
                        .param("cursor")
                        .and_then(|c| c.strip_prefix("page-"))
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                    let next = offset + options.page_size;
                    let page: Vec<&str> = matching
                        .iter()
                        .skip(offset)
                        .take(options.page_size)
                        .map(|s| s.as_str())
                        .collect();
                    let meta = if next < matching.len() {
                        format!("{{\"cursor\":\"page-{next}\"}}")
                    } else {
                        options.last_page_meta.clone()
                    };
                    (200, format!("{{\"data\":[{}],\"meta\":{meta}}}", page.join(",")))
                }
            };
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    FakeLangfuseServer { url, requests }
}
