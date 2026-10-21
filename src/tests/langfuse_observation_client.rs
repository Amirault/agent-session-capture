//! Port of `langfuseObservationClient.test.ts`: credentials and the HTTP
//! client against a loopback fake of the v2 endpoint.
//!
//! The TS fake recorded `query` as an object; the Rust fake records ordered
//! pairs, compared here as a map (order-insensitive, like `toEqual`).

use std::collections::{BTreeMap, HashMap};
use std::net::TcpListener;

use super::fixtures::TempDir;
use super::fixtures::fake_langfuse_server::{
    FakeServerOptions, RecordedRequest, TEST_PUBLIC_KEY, TEST_SECRET_KEY, start_fake_langfuse,
};
use super::fixtures::langfuse_observations::{any_observation, with};
use crate::adapters::langfuse_observation_client::{
    LangfuseCredentialOptions, LangfuseCredentials, LangfuseObservationClient, resolve_langfuse_credentials,
};
use crate::adapters::langfuse_observation_lister::{LangfuseObservationLister, ObservationQuery};
use crate::domain::source_errors::ReadError;
use crate::js::{Value, path};

const MISSING_CREDENTIALS: &str = "Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)";
const CONTENT_FIELDS: &str = "core,basic,io,metadata,trace_context";

fn write_env_file(file_path: &str, lines: &[&str]) -> String {
    std::fs::create_dir_all(path::dirname(file_path)).expect("mkdir");
    std::fs::write(file_path, format!("{}\n", lines.join("\n"))).expect("write env file");
    file_path.to_string()
}

fn default_env_file(home: &TempDir) -> String {
    path::join(&[&home.path(), ".config", "langfuse", "capture.env"])
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn resolve(home: &TempDir, vars: &[(&str, &str)], env_file: Option<String>) -> Result<LangfuseCredentials, ReadError> {
    resolve_langfuse_credentials(&LangfuseCredentialOptions {
        env: Some(env(vars)),
        env_file,
        home_dir: Some(home.path()),
    })
}

fn credentials(host: &str, public_key: &str, secret_key: &str) -> LangfuseCredentials {
    LangfuseCredentials {
        host: host.to_string(),
        public_key: public_key.to_string(),
        secret_key: secret_key.to_string(),
    }
}

fn client_for(url: &str, home: &TempDir) -> LangfuseObservationClient {
    LangfuseObservationClient::with_options(LangfuseCredentialOptions {
        env: Some(env(&[
            ("LANGFUSE_HOST", url),
            ("LANGFUSE_PUBLIC_KEY", TEST_PUBLIC_KEY),
            ("LANGFUSE_SECRET_KEY", TEST_SECRET_KEY),
        ])),
        env_file: None,
        home_dir: Some(home.path()),
    })
}

fn observations(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| with(any_observation(), &[("id", Value::from(format!("obs-{i}")))]).to_json())
        .collect()
}

fn ids(listed: &[crate::js::Object]) -> Vec<String> {
    listed
        .iter()
        .map(|o| o.get_str("id").expect("id").to_string())
        .collect()
}

fn cursors(requests: &[RecordedRequest]) -> Vec<Option<String>> {
    requests.iter().map(|r| r.param("cursor").map(str::to_string)).collect()
}

fn query_map(request: &RecordedRequest) -> BTreeMap<String, String> {
    request.query.iter().cloned().collect()
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn since(from: &str) -> ObservationQuery {
    ObservationQuery {
        from_start_time: Some(from.to_string()),
        ..Default::default()
    }
}

/// `path`, `query` and `authorized` of every recorded request.
fn summarized(requests: &[RecordedRequest]) -> Vec<(String, BTreeMap<String, String>, bool)> {
    requests
        .iter()
        .map(|r| (r.path.clone(), query_map(r), r.authorized))
        .collect()
}

// resolveLangfuseCredentials

#[test]
fn three_variables_in_the_environment_are_used_as_given() {
    // Given
    let home = TempDir::new("lf-client-");
    let vars = [
        ("LANGFUSE_HOST", "https://eu.example"),
        ("LANGFUSE_PUBLIC_KEY", "pk"),
        ("LANGFUSE_SECRET_KEY", "sk"),
    ];

    // When
    let resolved = resolve(&home, &vars, None);

    // Then
    assert_eq!(resolved, Ok(credentials("https://eu.example", "pk", "sk")));
}

#[test]
fn keys_without_host_default_the_host_to_langfuse_cloud() {
    // Given
    let home = TempDir::new("lf-client-");
    let vars = [("LANGFUSE_PUBLIC_KEY", "pk"), ("LANGFUSE_SECRET_KEY", "sk")];

    // When
    let resolved = resolve(&home, &vars, None).expect("credentials");

    // Then
    assert_eq!(resolved.host, "https://cloud.langfuse.com");
}

#[test]
fn keys_only_in_the_default_env_file_use_its_dotenv_lines() {
    // Given
    let home = TempDir::new("lf-client-");
    write_env_file(
        &default_env_file(&home),
        &[
            "# capture keys",
            "export LANGFUSE_PUBLIC_KEY=pk-file",
            "LANGFUSE_SECRET_KEY=\"sk-file\"",
            "",
            "LANGFUSE_HOST='https://file.example'",
        ],
    );

    // When
    let resolved = resolve(&home, &[], None);

    // Then
    assert_eq!(resolved, Ok(credentials("https://file.example", "pk-file", "sk-file")));
}

#[test]
fn environment_wins_then_langfuse_env_file_then_default_file() {
    // Given
    let home = TempDir::new("lf-client-");
    write_env_file(
        &default_env_file(&home),
        &[
            "LANGFUSE_HOST=https://default.example",
            "LANGFUSE_PUBLIC_KEY=pk-default",
            "LANGFUSE_SECRET_KEY=sk-default",
        ],
    );
    let env_file = write_env_file(
        &home.join("other.env"),
        &["LANGFUSE_PUBLIC_KEY=pk-explicit", "LANGFUSE_SECRET_KEY=sk-explicit"],
    );

    // When
    let resolved = resolve(&home, &[("LANGFUSE_SECRET_KEY", "sk-env")], Some(env_file));

    // Then
    assert_eq!(
        resolved,
        Ok(credentials("https://default.example", "pk-explicit", "sk-env"))
    );
}

fn assert_missing_credentials(vars: &[(&str, &str)]) {
    // Given
    let home = TempDir::new("lf-client-");

    // When
    let resolved = resolve(&home, vars, None);

    // Then
    assert_eq!(
        resolved,
        Err(ReadError::SourceConfiguration(MISSING_CREDENTIALS.to_string()))
    );
}

#[test]
fn no_key_anywhere_fails_as_a_configuration_error_naming_the_three_variables() {
    assert_missing_credentials(&[]);
}

#[test]
fn only_the_public_key_fails_as_a_configuration_error_naming_the_three_variables() {
    assert_missing_credentials(&[("LANGFUSE_PUBLIC_KEY", "pk-only")]);
}

#[test]
fn only_the_secret_key_fails_as_a_configuration_error_naming_the_three_variables() {
    assert_missing_credentials(&[("LANGFUSE_SECRET_KEY", "sk-only")]);
}

#[test]
fn empty_keys_fail_as_a_configuration_error_naming_the_three_variables() {
    assert_missing_credentials(&[("LANGFUSE_PUBLIC_KEY", ""), ("LANGFUSE_SECRET_KEY", " ")]);
}

// LangfuseObservationClient (against a loopback fake of the v2 endpoint)

#[test]
fn listing_2350_observations_follows_the_cursor_over_3_pages_in_order() {
    // Given
    let home = TempDir::new("lf-client-");
    let all = observations(2350);
    let server = start_fake_langfuse(FakeServerOptions {
        observations: all,
        ..Default::default()
    });

    // When
    let listed = client_for(&server.url, &home)
        .list_observations(&since("2026-09-19T12:00:00.000Z"))
        .expect("list");

    // Then
    assert_eq!(ids(&listed), (0..2350).map(|i| format!("obs-{i}")).collect::<Vec<_>>());
    assert_eq!(
        cursors(&server.requests()),
        vec![None, Some("page-1000".to_string()), Some("page-2000".to_string())]
    );
}

#[test]
fn listing_queries_the_v2_endpoint_authenticated_with_marker_mode_fields_page_size_and_cutoff() {
    // Given
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions::default());

    // When
    client_for(&server.url, &home)
        .list_observations(&since("2026-09-19T12:00:00.000Z"))
        .expect("list");

    // Then
    assert_eq!(
        summarized(&server.requests()),
        vec![(
            "/api/public/v2/observations".to_string(),
            map(&[
                ("fields", CONTENT_FIELDS),
                ("limit", "1000"),
                ("fromStartTime", "2026-09-19T12:00:00.000Z")
            ]),
            true
        )]
    );
}

fn assert_stops_after_last_page(last_page_meta: &str) {
    // Given
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions {
        observations: observations(3),
        page_size: 2,
        last_page_meta: last_page_meta.to_string(),
        max_requests: 5,
        ..Default::default()
    });

    // When
    let listed = client_for(&server.url, &home)
        .list_observations(&since("2026-09-19T12:00:00.000Z"))
        .expect("list");

    // Then
    assert_eq!(ids(&listed), vec!["obs-0", "obs-1", "obs-2"]);
    assert_eq!(cursors(&server.requests()), vec![None, Some("page-2".to_string())]);
}

#[test]
fn last_page_answering_cursor_null_stops_after_that_page() {
    assert_stops_after_last_page("{\"cursor\":null}");
}

#[test]
fn last_page_answering_cursor_empty_string_stops_after_that_page() {
    assert_stops_after_last_page("{\"cursor\":\"\"}");
}

#[test]
fn listing_a_trace_queries_the_trace_id_filter_with_content_fields_without_cutoff() {
    // Given
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions {
        observations: vec![
            with(any_observation(), &[("id", "obs-a".into()), ("traceId", "tr-a".into())]).to_json(),
            with(any_observation(), &[("id", "obs-b".into()), ("traceId", "tr-b".into())]).to_json(),
        ],
        ..Default::default()
    });

    // When
    let listed = client_for(&server.url, &home)
        .list_observations(&ObservationQuery {
            trace_id: Some("tr-a".to_string()),
            ..Default::default()
        })
        .expect("list");

    // Then
    assert_eq!(ids(&listed), vec!["obs-a"]);
    assert_eq!(
        server.requests().iter().map(query_map).collect::<Vec<_>>(),
        vec![map(&[
            ("fields", CONTENT_FIELDS),
            ("limit", "1000"),
            ("traceId", "tr-a")
        ])]
    );
}

fn assert_activity_query(query: ObservationQuery, filter: &[(&str, &str)]) {
    // Given
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions::default());

    // When
    client_for(&server.url, &home)
        .list_activity(&query)
        .expect("list activity");

    // Then
    let mut expected = vec![("fields", "core,basic,time"), ("limit", "1000")];
    expected.extend_from_slice(filter);
    assert_eq!(
        summarized(&server.requests()),
        vec![("/api/public/v2/observations".to_string(), map(&expected), true)]
    );
}

#[test]
fn activity_query_from_start_time_uses_the_time_field_groups_only() {
    assert_activity_query(
        ObservationQuery {
            from_start_time: Some("2026-10-01T00:00:00.000Z".to_string()),
            ..Default::default()
        },
        &[("fromStartTime", "2026-10-01T00:00:00.000Z")],
    );
}

#[test]
fn activity_query_session_id_uses_the_time_field_groups_only() {
    assert_activity_query(
        ObservationQuery {
            session_id: Some("hermes-0419".to_string()),
            ..Default::default()
        },
        &[("sessionId", "hermes-0419")],
    );
}

#[test]
fn activity_query_without_filter_uses_the_time_field_groups_only() {
    assert_activity_query(ObservationQuery::default(), &[]);
}

#[test]
fn api_answering_401_fails_as_a_source_access_error_naming_status_and_host_without_any_key() {
    // Given
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions {
        fail_with: Some(401),
        ..Default::default()
    });

    // When
    let listing = client_for(&server.url, &home).list_observations(&since("2026-09-19T12:00:00.000Z"));

    // Then
    assert_eq!(
        listing,
        Err(ReadError::SourceAccess(format!("Langfuse API 401 at {}", server.url)))
    );
}

#[test]
fn api_answering_503_fails_as_a_source_access_error_naming_status_and_host() {
    // Given (retried twice with backoff: about 3 s)
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions {
        fail_with: Some(503),
        ..Default::default()
    });

    // When
    let listing = client_for(&server.url, &home).list_observations(&since("2026-09-19T12:00:00.000Z"));

    // Then
    assert_eq!(
        listing,
        Err(ReadError::SourceAccess(format!("Langfuse API 503 at {}", server.url)))
    );
}

#[test]
fn host_nothing_listens_on_fails_as_a_source_access_error_naming_the_host() {
    // Given
    let home = TempDir::new("lf-client-");
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port())
    };

    // When
    let listing = client_for(&closed, &home).list_observations(&since("2026-09-19T12:00:00.000Z"));

    // Then
    assert_eq!(
        listing,
        Err(ReadError::SourceAccess(format!(
            "Langfuse API unreachable at {closed} (fetch failed)"
        )))
    );
}

#[test]
fn no_credentials_fail_as_a_configuration_error_before_any_request() {
    // Given
    let home = TempDir::new("lf-client-");
    let server = start_fake_langfuse(FakeServerOptions::default());
    let client = LangfuseObservationClient::with_options(LangfuseCredentialOptions {
        env: Some(env(&[("LANGFUSE_HOST", &server.url)])),
        env_file: None,
        home_dir: Some(home.path()),
    });

    // When
    let listing = client.list_observations(&since("2026-09-19T12:00:00.000Z"));

    // Then
    assert_eq!(
        listing,
        Err(ReadError::SourceConfiguration(MISSING_CREDENTIALS.to_string()))
    );
    assert!(server.requests().is_empty());
}
