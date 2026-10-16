//! A consistent copy of the store with a manifest beside it, and the check that holds a copy to its
//! manifest (docs/store.md, "The snapshot").

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

/// The copy's file name in a snapshot directory.
pub const COPY: &str = "warmer.sqlite";
/// The manifest's file name in a snapshot directory.
pub const MANIFEST: &str = "manifest.json";

/// What a snapshot holds, written beside the copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
  /// What the copy holds, or none where no store stood and no copy was written.
  pub store: Option<Held>,
}

/// What a copy of the store holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
  /// The latest migration recorded in the copy.
  pub migration: String,
  /// Each table's row count, by the table's name.
  pub tables: BTreeMap<String, i64>,
  /// The SHA-256 of the copy file, in hex.
  pub sha256: String,
}

/// Why a snapshot was not taken, or why a snapshot does not hold to its manifest.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
  #[error("The path {} is not UTF-8, so SQLite cannot be handed it.", .0.display())]
  NotUtf8(PathBuf),
  #[error("The file {} cannot be read or written: {error}.", path.display())]
  File { path: PathBuf, error: io::Error },
  #[error("The directory {} already holds a snapshot.", .0.display())]
  Occupied(PathBuf),
  #[error("The store at {} cannot be copied: {error}.", path.display())]
  Store { path: PathBuf, error: sqlx::Error },
  #[error("The copy at {} cannot be read: {error}.", path.display())]
  Copy { path: PathBuf, error: sqlx::Error },
  #[error("The file at {} holds no migration record, so it is not a store.", .0.display())]
  NoMigration(PathBuf),
  #[error("The directory {} holds no manifest.", .0.display())]
  ManifestMissing(PathBuf),
  #[error("The manifest at {} does not read: {error}.", path.display())]
  ManifestUnread {
    path: PathBuf,
    error: serde_json::Error,
  },
  #[error("The manifest holds a copy, but no copy stands at {}.", .0.display())]
  CopyMissing(PathBuf),
  #[error("The manifest says no store stood, but a copy stands at {}.", .0.display())]
  CopyWithoutStore(PathBuf),
  #[error("The copy hashes to {copy}, not to the manifest's {manifest}.")]
  Hash { manifest: String, copy: String },
  #[error("The copy fails SQLite's integrity check: {0}.")]
  Integrity(String),
  #[error("The copy records migration {copy}, not the manifest's {manifest}.")]
  Migration { manifest: String, copy: String },
  #[error("The table {table} holds {} in the copy and {} by the manifest.", rows(*.copy), rows(*.manifest))]
  Rows {
    table: String,
    manifest: Option<i64>,
    copy: Option<i64>,
  },
}

// A table's row count as a refusal says it, where the table may be missing on one side.
fn rows(count: Option<i64>) -> String {
  count.map_or_else(|| "no table".to_owned(), |count| format!("{count} rows"))
}

fn file_error(path: &Path) -> impl FnOnce(io::Error) -> SnapshotError + '_ {
  move |error| SnapshotError::File {
    path: path.to_path_buf(),
    error,
  }
}

// Whether a file stands at `path`, an error other than its absence being one.
fn stands(path: &Path) -> Result<bool, SnapshotError> {
  match fs::symlink_metadata(path) {
    Ok(_) => Ok(true),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
    Err(error) => Err(file_error(path)(error)),
  }
}

/// Copies the store at `store` into the directory `into`, made where missing, and writes the manifest
/// beside the copy. A store that does not stand is not created: the manifest then says so and no copy
/// is written. A directory already holding a copy or a manifest is refused with nothing written.
pub async fn take(store: &Path, into: &Path) -> Result<Manifest, SnapshotError> {
  let copy = into.join(COPY);
  let manifest_path = into.join(MANIFEST);
  fs::create_dir_all(into).map_err(file_error(into))?;
  if stands(&copy)? || stands(&manifest_path)? {
    return Err(SnapshotError::Occupied(into.to_path_buf()));
  }

  let manifest = if stands(store)? {
    vacuum_into(store, &copy).await?;
    match held(&copy).await {
      Ok(held) => Manifest { store: Some(held) },
      Err(error) => {
        // A copy no manifest describes is never left for a later run to take as a snapshot.
        let _ = fs::remove_file(&copy);
        return Err(match error {
          SnapshotError::NoMigration(_) => SnapshotError::NoMigration(store.to_path_buf()),
          error => error,
        });
      }
    }
  } else {
    Manifest { store: None }
  };

  let text = serde_json::to_string_pretty(&manifest).expect("a manifest is always JSON");
  fs::write(&manifest_path, text + "\n").map_err(file_error(&manifest_path))?;
  Ok(manifest)
}

/// Holds the snapshot in the directory `from` to its manifest, and answers the manifest where it does.
pub async fn verify(from: &Path) -> Result<Manifest, SnapshotError> {
  let manifest_path = from.join(MANIFEST);
  let text = match fs::read_to_string(&manifest_path) {
    Ok(text) => text,
    Err(error) if error.kind() == ErrorKind::NotFound => {
      return Err(SnapshotError::ManifestMissing(from.to_path_buf()));
    }
    Err(error) => return Err(file_error(&manifest_path)(error)),
  };
  let manifest: Manifest = serde_json::from_str(&text).map_err(|error| SnapshotError::ManifestUnread {
    path: manifest_path.clone(),
    error,
  })?;

  let copy = from.join(COPY);
  let Some(expected) = &manifest.store else {
    if stands(&copy)? {
      return Err(SnapshotError::CopyWithoutStore(copy));
    }
    return Ok(manifest);
  };
  if !stands(&copy)? {
    return Err(SnapshotError::CopyMissing(copy));
  }

  let sha256 = sha256(&copy)?;
  if sha256 != expected.sha256 {
    return Err(SnapshotError::Hash {
      manifest: expected.sha256.clone(),
      copy: sha256,
    });
  }
  let mut db = open_copy(&copy).await?;
  let checked: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
    .fetch_all(&mut db)
    .await
    .map_err(copy_error(&copy))?;
  if checked != ["ok"] {
    return Err(SnapshotError::Integrity(checked.join("; ")));
  }
  let found = read_held(&mut db, &copy, sha256).await?;
  db.close().await.map_err(copy_error(&copy))?;

  if found.migration != expected.migration {
    return Err(SnapshotError::Migration {
      manifest: expected.migration.clone(),
      copy: found.migration,
    });
  }
  let mut names = expected.tables.keys().chain(found.tables.keys());
  if let Some(table) = names.find(|name| expected.tables.get(*name) != found.tables.get(*name)) {
    return Err(SnapshotError::Rows {
      table: table.clone(),
      manifest: expected.tables.get(table).copied(),
      copy: found.tables.get(table).copied(),
    });
  }
  Ok(manifest)
}

// SQLite's own copy of the store as one read transaction sees it, written to `copy`. The store is
// opened without creating it and without running a migration.
async fn vacuum_into(store: &Path, copy: &Path) -> Result<(), SnapshotError> {
  let into = copy.to_str().ok_or_else(|| SnapshotError::NotUtf8(copy.to_path_buf()))?;
  let store_error = |error| SnapshotError::Store {
    path: store.to_path_buf(),
    error,
  };
  let options = SqliteConnectOptions::new().filename(store).create_if_missing(false);
  let mut db = SqliteConnection::connect_with(&options).await.map_err(store_error)?;

  sqlx::query("VACUUM INTO ?").bind(into).execute(&mut db).await.map_err(store_error)?;
  db.close().await.map_err(store_error)
}

fn copy_error(copy: &Path) -> impl Fn(sqlx::Error) -> SnapshotError + '_ {
  move |error| SnapshotError::Copy {
    path: copy.to_path_buf(),
    error,
  }
}

async fn open_copy(copy: &Path) -> Result<SqliteConnection, SnapshotError> {
  let options = SqliteConnectOptions::new().filename(copy).read_only(true);
  SqliteConnection::connect_with(&options).await.map_err(copy_error(copy))
}

// What the copy at `copy` holds, read from the copy itself.
async fn held(copy: &Path) -> Result<Held, SnapshotError> {
  let sha256 = sha256(copy)?;
  let mut db = open_copy(copy).await?;
  let held = read_held(&mut db, copy, sha256).await?;
  db.close().await.map_err(copy_error(copy))?;
  Ok(held)
}

async fn read_held(db: &mut SqliteConnection, copy: &Path, sha256: String) -> Result<Held, SnapshotError> {
  let names: Vec<String> = sqlx::query_scalar(
    "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
  )
  .fetch_all(&mut *db)
  .await
  .map_err(copy_error(copy))?;
  if !names.iter().any(|name| name == "seaql_migrations") {
    return Err(SnapshotError::NoMigration(copy.to_path_buf()));
  }
  let migration: Option<String> = sqlx::query_scalar("SELECT max(version) FROM seaql_migrations")
    .fetch_one(&mut *db)
    .await
    .map_err(copy_error(copy))?;
  let migration = migration.ok_or_else(|| SnapshotError::NoMigration(copy.to_path_buf()))?;

  let mut tables = BTreeMap::new();
  for name in names {
    let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")))
      .fetch_one(&mut *db)
      .await
      .map_err(copy_error(copy))?;
    tables.insert(name, count);
  }
  Ok(Held {
    migration,
    tables,
    sha256,
  })
}

fn sha256(path: &Path) -> Result<String, SnapshotError> {
  let mut file = File::open(path).map_err(file_error(path))?;
  let mut hasher = Sha256::new();
  io::copy(&mut file, &mut hasher).map_err(file_error(path))?;
  Ok(format!("{:x}", hasher.finalize()))
}
