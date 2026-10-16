//! The snapshot (docs/store.md, "The snapshot"): a copy of a store in use holds to its manifest and
//! reopens with the same rows, and a copy or a manifest that changed, or is missing, is refused.

mod fixtures;

use std::fs;
use std::path::Path;

use bmo_cache_warmer::snapshot::{self, COPY, MANIFEST, Manifest, SnapshotError};
use bmo_cache_warmer::store::{Attempt, Outcome, Store};
use fixtures::{MINUTE, NOW, line, place, store};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

/// A store holding a consumer, a group, two targets with their use, a switch, a pause, attempts and a
/// stopped round, left open as the service holds it.
async fn store_in_use() -> (tempfile::TempDir, Store) {
  let (dir, store) = store().await;
  store.put_consumer("agency", "http://127.0.0.1:1/ping").await.unwrap();
  store.put_group("agency", "pool", 2).await.unwrap();
  store.put_target("agency", "line", &line("worker")).await.unwrap();
  store.put_target("agency", "place", &place("seat", "pool")).await.unwrap();
  store.report_use("agency", "line", NOW, NOW).await.unwrap();
  store.set_switch("agency", "worker", Some(false)).await.unwrap();
  store.pause("agency", "seat", NOW + MINUTE).await.unwrap();
  for (at, outcome) in [(NOW, Outcome::Warmed), (NOW + MINUTE, Outcome::Failed)] {
    let attempt = Attempt {
      at,
      outcome,
      code: None,
      reason: Some("a reason".into()),
    };
    store.record_attempt("agency", "place", &attempt).await.unwrap();
  }
  store.set_round_stopped(true).await.unwrap();

  (dir, store)
}

async fn taken() -> (tempfile::TempDir, tempfile::TempDir, Store) {
  let (dir, store) = store_in_use().await;
  let snapshot = tempfile::tempdir().unwrap();
  snapshot::take(&dir.path().join("warmer.sqlite"), snapshot.path()).await.unwrap();

  (dir, snapshot, store)
}

fn manifest(dir: &Path) -> Manifest {
  serde_json::from_str(&fs::read_to_string(dir.join(MANIFEST)).unwrap()).unwrap()
}

fn write_manifest(dir: &Path, manifest: &Manifest) {
  fs::write(dir.join(MANIFEST), serde_json::to_string(manifest).unwrap()).unwrap();
}

#[tokio::test]
async fn a_snapshot_of_a_store_in_use_verifies_and_reopens_with_the_same_rows() {
  let (_dir, snapshot, store) = taken().await;

  let written = manifest(snapshot.path());
  let held = written.store.as_ref().expect("the manifest holds the copy");
  assert_eq!(held.migration, "m20261017_000001_tables");
  for (table, rows) in [("consumers", 1), ("groups", 1), ("subjects", 2), ("targets", 2), ("attempts", 2)] {
    assert_eq!(held.tables.get(table), Some(&rows), "{table}");
  }
  assert_eq!(snapshot::verify(snapshot.path()).await.unwrap(), written);

  // The copy is reopened elsewhere, as it is put back, so the snapshot itself stays as taken.
  let restored = tempfile::tempdir().unwrap();
  fs::copy(snapshot.path().join(COPY), restored.path().join("warmer.sqlite")).unwrap();
  let reopened = Store::open(&restored.path().join("warmer.sqlite")).await.unwrap();
  assert_eq!(reopened.targets(None, None).await.unwrap(), store.targets(None, None).await.unwrap());
  assert_eq!(reopened.subjects("agency").await.unwrap(), store.subjects("agency").await.unwrap());
  assert_eq!(
    reopened.attempts("agency", "place").await.unwrap(),
    store.attempts("agency", "place").await.unwrap()
  );
  assert!(reopened.round_stopped().await.unwrap());
}

#[tokio::test]
async fn verify_refuses_a_changed_copy() {
  let (_dir, snapshot, _store) = taken().await;
  let options = SqliteConnectOptions::new().filename(snapshot.path().join(COPY));
  let mut copy = SqliteConnection::connect_with(&options).await.unwrap();
  sqlx::query(r#"UPDATE "consumers" SET "hook_url" = 'http://127.0.0.1:2/ping'"#)
    .execute(&mut copy)
    .await
    .unwrap();
  copy.close().await.unwrap();

  let refused = snapshot::verify(snapshot.path()).await.unwrap_err();
  assert!(matches!(refused, SnapshotError::Hash { .. }), "{refused}");
}

#[tokio::test]
async fn verify_refuses_a_copy_whose_rows_differ_from_the_manifest() {
  let (_dir, snapshot, _store) = taken().await;
  let mut written = manifest(snapshot.path());
  written.store.as_mut().unwrap().tables.insert("targets".into(), 3);
  write_manifest(snapshot.path(), &written);

  let refused = snapshot::verify(snapshot.path()).await.unwrap_err();
  assert_eq!(
    refused.to_string(),
    "The table targets holds 2 rows in the copy and 3 rows by the manifest."
  );
}

#[tokio::test]
async fn verify_refuses_a_missing_manifest_and_a_missing_copy() {
  let (_dir, snapshot, _store) = taken().await;
  fs::rename(snapshot.path().join(MANIFEST), snapshot.path().join("kept.json")).unwrap();
  let refused = snapshot::verify(snapshot.path()).await.unwrap_err();
  assert!(matches!(refused, SnapshotError::ManifestMissing(_)), "{refused}");

  fs::rename(snapshot.path().join("kept.json"), snapshot.path().join(MANIFEST)).unwrap();
  fs::remove_file(snapshot.path().join(COPY)).unwrap();
  let refused = snapshot::verify(snapshot.path()).await.unwrap_err();
  assert!(matches!(refused, SnapshotError::CopyMissing(_)), "{refused}");
}

#[tokio::test]
async fn a_directory_already_holding_a_snapshot_is_refused() {
  let (dir, snapshot, _store) = taken().await;
  let before = fs::read(snapshot.path().join(MANIFEST)).unwrap();

  let refused = snapshot::take(&dir.path().join("warmer.sqlite"), snapshot.path()).await.unwrap_err();
  assert!(matches!(refused, SnapshotError::Occupied(_)), "{refused}");
  assert_eq!(fs::read(snapshot.path().join(MANIFEST)).unwrap(), before);
}

#[tokio::test]
async fn a_store_that_does_not_stand_is_not_created_and_its_manifest_says_so() {
  let dir = tempfile::tempdir().unwrap();
  let store = dir.path().join("warmer.sqlite");
  let snapshot = dir.path().join("snapshot");

  let written = snapshot::take(&store, &snapshot).await.unwrap();
  assert_eq!(written, Manifest { store: None });
  assert!(!store.exists() && !snapshot.join(COPY).exists());
  assert_eq!(snapshot::verify(&snapshot).await.unwrap(), written);

  fs::write(snapshot.join(COPY), b"").unwrap();
  let refused = snapshot::verify(&snapshot).await.unwrap_err();
  assert!(matches!(refused, SnapshotError::CopyWithoutStore(_)), "{refused}");
}

#[tokio::test]
async fn a_file_with_no_migration_record_is_not_a_store() {
  let dir = tempfile::tempdir().unwrap();
  let other = dir.path().join("other.sqlite");
  let options = SqliteConnectOptions::new().filename(&other).create_if_missing(true);
  let mut db = SqliteConnection::connect_with(&options).await.unwrap();
  sqlx::query("CREATE TABLE notes (body TEXT)").execute(&mut db).await.unwrap();
  db.close().await.unwrap();

  let snapshot = dir.path().join("snapshot");
  let refused = snapshot::take(&other, &snapshot).await.unwrap_err();
  assert!(matches!(&refused, SnapshotError::NoMigration(path) if *path == other), "{refused}");
  assert!(!snapshot.join(COPY).exists() && !snapshot.join(MANIFEST).exists());
}
