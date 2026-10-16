//! The deployed service (docs/api.md, "The deployed service"): the unit, the testbed declaration's
//! `[deploy]` table and the boundary document name one address, and the unit runs the release binary
//! from the declared deploy tree with its store in the declared data root.

mod support;

use std::fs;
use std::path::Path;

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
