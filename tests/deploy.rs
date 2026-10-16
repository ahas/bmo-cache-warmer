//! The deployed service (docs/api.md, "The deployed service"): the unit, the testbed declaration's
//! `[deploy]` table and the boundary document name one address, and the unit runs the release binary
//! from the declared deploy tree with its store in the declared data root. The backup a deployment
//! takes copies that store with that binary, and leaves the live store out of the raw pack.

mod support;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use bmo_cache_warmer::store::Store;
use support::ROOT;
use toml::Table;

/// The address the deployed service listens on.
const ADDRESS: &str = "127.0.0.1:9023";
const UNIT: &str = "deploy/systemd/bmo-cache-warmer.service";

fn read(path: &str) -> String {
  fs::read_to_string(Path::new(ROOT).join(path)).unwrap_or_else(|error| panic!("{path} reads: {error}"))
}

fn declaration() -> Table {
  read("testbed.toml").parse().expect("testbed.toml is TOML")
}

/// A string key of a table of the declaration.
fn key<'a>(declaration: &'a Table, table: &str, key: &str) -> &'a str {
  declaration[table][key]
    .as_str()
    .unwrap_or_else(|| panic!("[{table}] {key} is a string"))
}

/// A path the declaration roots at the home folder, as the unit names it.
fn in_home(path: &str) -> String {
  let under = path.strip_prefix("~/").unwrap_or_else(|| panic!("{path} stands under the home folder"));
  format!("%h/{under}")
}

#[test]
fn the_unit_the_declaration_and_the_document_name_one_address() {
  let declaration = declaration();
  let unit = read(UNIT);

  assert!(
    unit.contains(&format!("Environment=CACHE_WARMER_LISTEN={ADDRESS}\n")),
    "{UNIT} listens elsewhere"
  );
  let health = key(&declaration, "deploy", "health");
  assert!(
    health.contains(&format!("http://{ADDRESS}/health")),
    "the health asks elsewhere: {health}"
  );
  assert!(
    read("docs/api.md").contains(&format!("`{ADDRESS}`")),
    "docs/api.md names another address"
  );
}

#[test]
fn the_unit_runs_the_release_binary_with_its_store_in_the_data_root() {
  let declaration = declaration();
  let unit = read(UNIT);
  let root = in_home(key(&declaration, "deploy", "root"));
  let data_root = key(&declaration, "deploy", "data_root");
  let binaries = declaration["build"]["binaries"].as_array().expect("[build] binaries is a list");

  assert!(
    binaries.iter().any(|binary| binary.as_str() == Some("bmo-cache-warmer")),
    "{binaries:?}"
  );
  assert!(
    unit.contains(&format!("ExecStart={root}/deploy/bin/bmo-cache-warmer\n")),
    "{UNIT} runs another binary"
  );
  assert!(
    unit.contains(&format!("Environment=CACHE_WARMER_STORE={}/warmer.sqlite\n", in_home(data_root))),
    "{UNIT} keeps its store outside the data root"
  );
  assert!(
    unit.contains("\nX-BMO-Deploy=bot\n"),
    "{UNIT} is not marked as the repository's service"
  );
}

#[test]
fn the_declared_units_are_the_units_folder() {
  let declaration = declaration();
  let units = declaration["deploy"]["units"].as_array().expect("[deploy] units is a list");

  assert_eq!(
    units.iter().filter_map(|unit| unit.as_str()).collect::<Vec<_>>(),
    ["deploy/systemd/*.service"]
  );
  let found: Vec<String> = fs::read_dir(Path::new(ROOT).join("deploy/systemd"))
    .expect("deploy/systemd reads")
    .map(|entry| entry.expect("an entry reads").file_name().to_string_lossy().into_owned())
    .collect();
  assert_eq!(found, ["bmo-cache-warmer.service"]);
}

#[test]
fn the_backup_copies_the_units_store_with_the_release_binary_and_packs_no_live_store() {
  let declaration = declaration();
  let root = key(&declaration, "deploy", "root");
  let data_root = key(&declaration, "deploy", "data_root");
  let backup = declaration["deploy"]["backup"].as_table().expect("[deploy.backup] is a table");
  let binary = format!("{root}/deploy/bin/bmo-cache-warmer");
  let store = format!("{data_root}/warmer.sqlite");

  assert!(
    read(UNIT).contains(&format!("Environment=CACHE_WARMER_STORE={}\n", in_home(&store))),
    "{UNIT} keeps another store"
  );
  let dir = backup["dir"].as_str().expect("[deploy.backup] dir is a string");
  assert_eq!(dir, format!("{root}/backups"));
  assert!(!dir.starts_with(&format!("{data_root}/")), "the backups stand inside the data root");
  let snapshot = backup["snapshot"].as_str().expect("[deploy.backup] snapshot is a string");
  assert!(snapshot.contains(&format!("exec {binary} snapshot --store {store}\n")), "{snapshot}");
  let verify = backup["verify"].as_str().expect("[deploy.backup] verify is a string");
  assert!(verify.contains(&format!("exec {binary} verify\n")), "{verify}");
  let exclude = backup["exclude"].as_array().expect("[deploy.backup] exclude is a list");
  assert_eq!(
    exclude.iter().filter_map(|path| path.as_str()).collect::<Vec<_>>(),
    ["warmer.sqlite", "warmer.sqlite-journal", "warmer.sqlite-wal", "warmer.sqlite-shm"]
  );
}

/// The declared backup command `name` run as the testbed runs it, through `sh -c`, with `home` as the
/// home folder and `snapshot` as the snapshot directory.
fn backup_command(name: &str, home: &Path, snapshot: &Path) -> Output {
  let declaration = declaration();
  let line = declaration["deploy"]["backup"][name].as_str().expect("a declared command");
  Command::new("sh")
    .args(["-c", line])
    .current_dir(ROOT)
    .env("HOME", home)
    .env("BMO_BACKUP_SNAPSHOT_DIR", snapshot)
    .env_remove("CACHE_WARMER_STORE")
    .output()
    .expect("sh runs")
}

fn said(output: &Output) -> String {
  format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[tokio::test]
async fn the_declared_backup_runs_before_the_first_release_and_after_it() {
  let home = tempfile::tempdir().unwrap();
  let data_root = home.path().join(".local/share/bmo-cache-warmer/data");
  let empty = home.path().join("empty");
  fs::create_dir(&empty).unwrap();

  // Before the first release: no binary and no store, so nothing is copied and the backup passes.
  for name in ["snapshot", "verify"] {
    let ran = backup_command(name, home.path(), &empty);
    assert!(ran.status.success(), "{name}: {}", said(&ran));
  }
  assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);

  // A store with no release binary to copy it fails the backup.
  fs::create_dir_all(&data_root).unwrap();
  let store = Store::open(&data_root.join("warmer.sqlite")).await.unwrap();
  store.put_consumer("agency", "http://127.0.0.1:1/ping").await.unwrap();
  let ran = backup_command("snapshot", home.path(), &empty);
  assert!(!ran.status.success());
  assert_eq!(said(&ran), "The store stands, but no release binary stands to copy it.\n");

  // With the release binary in place, it copies the open store and holds the copy to its manifest.
  let bin = home.path().join(".local/share/bmo-cache-warmer/deploy/bin");
  fs::create_dir_all(&bin).unwrap();
  std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_bmo-cache-warmer"), bin.join("bmo-cache-warmer")).unwrap();
  let snapshot = home.path().join("snapshot");
  let ran = backup_command("snapshot", home.path(), &snapshot);
  assert!(ran.status.success(), "{}", said(&ran));
  let ran = backup_command("verify", home.path(), &snapshot);
  assert!(ran.status.success(), "{}", said(&ran));

  fs::remove_file(snapshot.join("manifest.json")).unwrap();
  let ran = backup_command("verify", home.path(), &snapshot);
  assert_eq!(ran.status.code(), Some(1));
  assert_eq!(said(&ran), format!("The directory {} holds no manifest.\n", snapshot.display()));
}
