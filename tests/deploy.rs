//! The deployed service (docs/api.md, "The deployed service"): the unit runs the release binary from the
//! declared deploy tree, and it and the health name no address of their own but read the release's
//! settings, which the install writes with the port it chose and a deployment writes where none stands,
//! both naming the store and the token file in the declared data root. The health reaches the address
//! its settings name. The backup a deployment takes copies that store with that binary, and leaves the
//! live store out of the raw pack.

use crate::fixtures;
use crate::support;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use bmo_cache_warmer::routes::router;
use bmo_cache_warmer::settings::SettingsFile;
use bmo_cache_warmer::store::Store;
use support::ROOT;
use toml::Table;

/// The address a deployment's settings name where it finds none.
const PREFERRED: &str = "127.0.0.1:9023";
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
fn the_unit_and_the_health_name_no_address_and_read_the_releases_settings() {
  let declaration = declaration();
  let unit = read(UNIT);
  let root = key(&declaration, "deploy", "root");

  assert!(!unit.contains("Environment="), "{UNIT} sets the service's settings itself");
  assert!(!unit.contains("127.0.0.1"), "{UNIT} names an address");
  assert_eq!(
    key(&declaration, "deploy", "health"),
    format!("{root}/deploy/bin/bmo-cache-warmer health")
  );
  assert!(
    read("docs/api.md").contains(&format!("`{PREFERRED}`")),
    "docs/api.md names another preferred address"
  );
}

/// The settings a file holds, read as the service reads them.
fn settings_in(file: &Path) -> SettingsFile {
  SettingsFile::read(file).unwrap_or_else(|sentence| panic!("{sentence}"))
}

/// The declared settings step run as the testbed runs it, from a release folder, with `home` as the home
/// folder.
fn run_settings_step(home: &Path, release: &Path) {
  let ran = Command::new("sh")
    .args(["-c", &step("settings")])
    .current_dir(release)
    .env("HOME", home)
    .output()
    .unwrap();
  assert!(ran.status.success(), "{}", said(&ran));
}

#[test]
fn a_deployment_writes_the_preferred_settings_where_none_stand_and_keeps_those_that_do() {
  let home = tempfile::tempdir().unwrap();
  let data_root = home.path().join(".local/share/bmo-cache-warmer/data");
  let release = home.path().join("release");
  fs::create_dir(&release).unwrap();

  run_settings_step(home.path(), &release);
  let written = settings_in(&data_root.join("settings.toml"));
  assert_eq!(
    written,
    SettingsFile {
      listen: Some(PREFERRED.parse().unwrap()),
      store: Some(data_root.join("warmer.sqlite")),
      token_file: Some(data_root.join("token")),
    }
  );
  assert_eq!(fs::read_link(release.join("settings.toml")).unwrap(), data_root.join("settings.toml"));

  let chosen = "listen = \"127.0.0.1:9123\"\n";
  fs::write(data_root.join("settings.toml"), chosen).unwrap();
  let again = home.path().join("again");
  fs::create_dir(&again).unwrap();
  run_settings_step(home.path(), &again);
  assert_eq!(fs::read_to_string(data_root.join("settings.toml")).unwrap(), chosen);
  assert_eq!(fs::read_to_string(again.join("settings.toml")).unwrap(), chosen);
}

#[test]
fn the_install_writes_the_settings_with_the_port_it_chose_and_the_deployments_store_and_token() {
  let install: Table = read("install.toml").parse().expect("install.toml is TOML");
  let declaration = declaration();
  let data_root = key(&declaration, "deploy", "data_root");
  let under_prefix = data_root
    .strip_prefix("~/.local/share/")
    .unwrap_or_else(|| panic!("{data_root} stands under the install prefix"));
  let settings = install["settings"].as_array().expect("[[settings]] is a list");
  let [written] = settings.as_slice() else {
    panic!("one settings file is written: {settings:?}")
  };

  assert_eq!(written["file"].as_str(), Some(format!("prefix:{under_prefix}/settings.toml").as_str()));
  let filled = written["text"]
    .as_str()
    .expect("its text is a string")
    .replace("{{port.cache-warmer.server}}", "9123")
    .replace("{{prefix}}", "/prefix");
  let filled: SettingsFile = toml::from_str(&filled).expect("the filled text is the warmer's settings");
  assert_eq!(
    filled,
    SettingsFile {
      listen: Some("127.0.0.1:9123".parse().unwrap()),
      store: Some(Path::new("/prefix").join(under_prefix).join("warmer.sqlite")),
      token_file: Some(Path::new("/prefix").join(under_prefix).join("token")),
    }
  );
  let ports = install["port"].as_array().expect("[[port]] is a list");
  assert!(
    ports.iter().any(|port| port["name"].as_str() == Some("cache-warmer.server")
      && port["number"].as_integer() == Some(9023)),
    "{ports:?}"
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
    unit.contains(&format!("ExecStartPre=/usr/bin/mkdir -p {}\n", in_home(data_root))),
    "{UNIT} makes another data root"
  );
  assert!(
    unit.contains("\nX-BMO-Deploy=bot\n"),
    "{UNIT} is not marked as the repository's service"
  );
}

/// The command of the declared deploy step `id`.
fn step(id: &str) -> String {
  let declaration = declaration();
  let steps = declaration["deploy"]["step"].as_array().expect("[[deploy.step]] is a list");
  let step = steps
    .iter()
    .find(|step| step["id"].as_str() == Some(id))
    .unwrap_or_else(|| panic!("a deploy step {id} stands"));

  step["command"].as_str().expect("a step's command is a string").to_owned()
}

#[test]
fn the_token_is_made_once_and_kept_across_deployments() {
  let home = tempfile::tempdir().unwrap();
  let token = home.path().join(".local/share/bmo-cache-warmer/data/token");
  let make = || {
    let ran = Command::new("sh")
      .args(["-c", &step("make-token")])
      .env("HOME", home.path())
      .output()
      .unwrap();
    assert!(ran.status.success(), "{}", said(&ran));
    fs::read_to_string(&token).unwrap()
  };

  let first = make();
  assert_eq!(first.len(), 64, "{first}");
  assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()), "{first}");
  assert_eq!(make(), first);
  let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&token).unwrap().permissions());
  assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn the_binarys_health_ends_0_only_where_the_service_answers_ready_to_its_token() {
  let (_dir, warmer, _clock) = fixtures::warmer().await;
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = format!("http://{}", listener.local_addr().unwrap());
  tokio::spawn(async move { axum::serve(listener, router(warmer, "the-token")).await.unwrap() });
  let tokens = tempfile::tempdir().unwrap();
  let health = |token: &str| {
    let file = tokens.path().join(token);
    fs::write(&file, format!("{token}\n")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_bmo-cache-warmer"));
    command
      .args(["health", "--address", &address, "--token-file"])
      .arg(file)
      .env_remove("CACHE_WARMER_TOKEN")
      .env_remove("CACHE_WARMER_SETTINGS");
    tokio::task::spawn_blocking(move || command.output().unwrap())
  };

  let ready = health("the-token").await.unwrap();
  assert_eq!(ready.status.code(), Some(0), "{}", said(&ready));
  assert_eq!(
    said(&ready),
    format!("The service is ready, at version {}.\n", env!("CARGO_PKG_VERSION"))
  );
  let refused = health("another").await.unwrap();
  assert_eq!(refused.status.code(), Some(1));
  assert!(said(&refused).contains("401 unauthorized"), "{}", said(&refused));
}

/// The binary's `health` run with `settings` as its settings file and nothing else naming its address or
/// token.
fn health_by(settings: &Path) -> Output {
  Command::new(env!("CARGO_BIN_EXE_bmo-cache-warmer"))
    .arg("health")
    .env("CACHE_WARMER_SETTINGS", settings)
    .env_remove("CACHE_WARMER_ADDRESS")
    .env_remove("CACHE_WARMER_TOKEN")
    .env_remove("CACHE_WARMER_TOKEN_FILE")
    .output()
    .unwrap()
}

/// A settings file in `dir` naming `listen`, the store and the token file `dir` holds.
fn settings_file(dir: &Path, listen: &str) -> std::path::PathBuf {
  let file = dir.join(format!("settings-{}.toml", listen.replace([':', '.'], "-")));
  let text = format!(
    "listen = \"{listen}\"\nstore = \"{}\"\ntoken_file = \"{}\"\n",
    dir.join("warmer.sqlite").display(),
    dir.join("token").display()
  );
  fs::write(&file, text).unwrap();
  file
}

#[tokio::test]
async fn the_health_reaches_the_address_its_settings_name_with_their_token() {
  let (_dir, warmer, _clock) = fixtures::warmer().await;
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let serving = listener.local_addr().unwrap().to_string();
  tokio::spawn(async move { axum::serve(listener, router(warmer, "the-token")).await.unwrap() });
  let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let nobody = closed.local_addr().unwrap().to_string();
  drop(closed);
  let dir = tempfile::tempdir().unwrap();
  fs::write(dir.path().join("token"), "the-token\n").unwrap();

  let at_the_service = settings_file(dir.path(), &serving);
  let ready = tokio::task::spawn_blocking(move || health_by(&at_the_service)).await.unwrap();
  assert_eq!(ready.status.code(), Some(0), "{}", said(&ready));

  let elsewhere = settings_file(dir.path(), &nobody);
  let unreached = tokio::task::spawn_blocking(move || health_by(&elsewhere)).await.unwrap();
  assert_eq!(unreached.status.code(), Some(1));
  assert!(said(&unreached).starts_with("The service did not answer"), "{}", said(&unreached));

  let missing = dir.path().join("missing.toml");
  let unread = tokio::task::spawn_blocking(move || health_by(&missing)).await.unwrap();
  assert_eq!(unread.status.code(), Some(1));
  assert!(said(&unread).starts_with("The settings file "), "{}", said(&unread));
}

#[tokio::test]
async fn the_service_listens_where_its_settings_say_and_its_health_finds_it_there() {
  let dir = tempfile::tempdir().unwrap();
  fs::write(dir.path().join("token"), "the-token\n").unwrap();
  let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let listen = free.local_addr().unwrap().to_string();
  drop(free);
  let settings = settings_file(dir.path(), &listen);
  let mut service = tokio::process::Command::new(env!("CARGO_BIN_EXE_bmo-cache-warmer"))
    .env("CACHE_WARMER_SETTINGS", &settings)
    .env_remove("CACHE_WARMER_LISTEN")
    .env_remove("CACHE_WARMER_STORE")
    .env_remove("CACHE_WARMER_TOKEN_FILE")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true)
    .spawn()
    .unwrap();

  let mut answered = None;
  for _ in 0..600 {
    let asked = settings.clone();
    let health = tokio::task::spawn_blocking(move || health_by(&asked)).await.unwrap();
    if health.status.success() {
      answered = Some(health);
      break;
    }
    if let Some(ended) = service.try_wait().unwrap() {
      panic!("the service ended with {ended}");
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
  }
  let answered = answered.expect("the service answers ready at the address its settings name");
  assert!(said(&answered).starts_with("The service is ready"), "{}", said(&answered));
  assert!(
    dir.path().join("warmer.sqlite").exists(),
    "the service keeps the store its settings name"
  );
  service.kill().await.unwrap();
}

#[test]
fn the_service_does_not_start_without_a_listen_address() {
  let dir = tempfile::tempdir().unwrap();
  let settings = dir.path().join("settings.toml");
  fs::write(&settings, "store = \"warmer.sqlite\"\n").unwrap();
  let ran = Command::new(env!("CARGO_BIN_EXE_bmo-cache-warmer"))
    .env("CACHE_WARMER_SETTINGS", &settings)
    .env_remove("CACHE_WARMER_LISTEN")
    .env_remove("CACHE_WARMER_STORE")
    .env_remove("CACHE_WARMER_TOKEN_FILE")
    .output()
    .unwrap();

  assert_eq!(ran.status.code(), Some(1));
  assert!(said(&ran).starts_with("No listen address is given"), "{}", said(&ran));
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
    step("settings").contains("store = \"%s/warmer.sqlite\""),
    "the settings keep another store"
  );
  assert_eq!(format!("{data_root}/warmer.sqlite"), store);
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
  store.put_consumer("tester", "http://[::1]:1/ping", 0).await.unwrap();
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
