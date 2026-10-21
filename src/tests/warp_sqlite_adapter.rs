//! §9.1 warpSqliteAdapter — snapshot & DB safety.
//!
//! The Rust adapter takes only an optional live DB path (no snapshot-dir or
//! keep-snapshot options), so snapshot cleanup is checked through observable
//! behaviour: the snapshot file is gone once `with_snapshot` returns.

use super::fixtures::TempDir;
use super::fixtures::fixture_db::{MarkerSeed, create_fixture, seed_marker};
use crate::adapters::warp_sqlite_adapter::WarpSqliteAdapter;
use crate::domain::ports::ConversationSource;
use crate::domain::source_errors::ReadError;

/// A content fingerprint of `path` (the bytes themselves), or `None` when missing.
fn hash_file(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn seed_smoke_marker(db: &rusqlite::Connection) {
    seed_marker(
        db,
        MarkerSeed {
            capture_id: "smoke",
            label: "specify",
            conversation_id: "c-seed",
            start_ts: "2026-06-30 19:34:14.579587",
        },
    );
}

#[test]
fn wal_snapshot_includes_committed_rows_and_never_modifies_live_files() {
    // Given — writer stays open so the committed row lives in the -wal (not checkpointed)
    let tmp = TempDir::new("wa-91-");
    let live_path = tmp.join("live.db");
    let writer = create_fixture(&live_path, true);
    seed_smoke_marker(&writer);
    let wal_path = format!("{live_path}-wal");
    let before_db = hash_file(&live_path);
    let before_wal = hash_file(&wal_path);

    // When
    let adapter = WarpSqliteAdapter::new(Some(live_path.clone()));
    let mut snapshot_path = None;
    let rows = adapter
        .with_snapshot(|db| {
            snapshot_path = db.connection().path().map(str::to_string);
            let rows = db.all("SELECT command FROM commands", &[])?;
            Ok(rows
                .iter()
                .filter_map(|r| r.get("command").text())
                .collect::<Vec<String>>())
        })
        .expect("snapshot");

    // Then
    assert!(rows.contains(&": CAPTURE_MARKER v=1 id=smoke label=specify".to_string()));
    assert_eq!(hash_file(&live_path), before_db);
    assert_eq!(hash_file(&wal_path), before_wal);
    // And — the ephemeral snapshot is removed once the run finishes
    let snapshot_path = snapshot_path.expect("snapshot path");
    assert_ne!(snapshot_path, live_path);
    assert!(std::fs::metadata(&snapshot_path).is_err());
    drop(writer);
}

#[test]
fn missing_live_db_path_fails_with_a_clear_error_listing_probed_paths() {
    // Given
    let tmp = TempDir::new("wa-91-");
    let missing = tmp.join("does-not-exist.db");
    let adapter = WarpSqliteAdapter::new(Some(missing.clone()));

    // When
    let error = adapter.resolve_live_db_path().expect_err("missing DB");

    // Then
    assert!(matches!(error, ReadError::Other(_)));
    assert!(error.message().contains("warp.sqlite not found"));
    assert!(error.message().contains(&missing));
}

#[test]
fn snapshot_is_read_only_and_any_write_attempt_fails() {
    // Given
    let tmp = TempDir::new("wa-91-");
    let live_path = tmp.join("live.db");
    let writer = create_fixture(&live_path, false);
    seed_smoke_marker(&writer);
    let adapter = WarpSqliteAdapter::new(Some(live_path));

    // When
    let write = adapter
        .with_snapshot(|db| Ok(db.all("CREATE TABLE should_fail(id INTEGER)", &[])))
        .expect("snapshot");

    // Then
    assert!(write.is_err());
    drop(writer);
}
