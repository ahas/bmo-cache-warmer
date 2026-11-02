//! Dependencies: no path out of the repository, no other unit's crate, and nothing by git. The warmer calls
//! no other unit and serves no screen, so not even the base libraries are allowed. The schedule crate has
//! no dependency at all, so a consumer that takes it builds the rule alone.

use crate::support;

use std::path::{Component, Path, PathBuf};

use support::{ROOT, files_under, manifest, shown};
use toml::{Table, Value};

/// The crates of other units this repository may depend on: none.
const BASE_LIBRARIES: [&str; 0] = [];

/// The schedule crate's directory under the root.
const SCHEDULE: &str = "schedule";

/// The dependency tables a manifest or a target section may carry.
const DEPENDENCY_TABLES: [&str; 5] = [
  "dependencies",
  "dev-dependencies",
  "build-dependencies",
  "dev_dependencies",
  "build_dependencies",
];

/// `path` with `.` and `..` resolved by reading alone, so a path that does not exist still compares.
fn lexical(path: &Path) -> PathBuf {
  let mut out = PathBuf::new();
  for component in path.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        out.pop();
      }
      other => out.push(other),
    }
  }
  out
}

/// The dependency tables standing directly in one section of a manifest.
fn own(section: &Table) -> impl Iterator<Item = &Table> {
  DEPENDENCY_TABLES.iter().filter_map(|name| section.get(*name).and_then(Value::as_table))
}

/// Every dependency table in a manifest: the package's own, each target's, a workspace's shared one,
/// and each patch source's.
fn dependency_tables(manifest: &Table) -> Vec<&Table> {
  let mut tables: Vec<&Table> = own(manifest).collect();
  if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
    for target in targets.values().filter_map(Value::as_table) {
      tables.extend(own(target));
    }
  }
  if let Some(shared) = manifest
    .get("workspace")
    .and_then(|workspace| workspace.get("dependencies"))
    .and_then(Value::as_table)
  {
    tables.push(shared);
  }
  if let Some(patches) = manifest.get("patch").and_then(Value::as_table) {
    tables.extend(patches.values().filter_map(Value::as_table));
  }
  tables
}

/// What breaks the rules among one manifest's dependencies, read as standing in `manifest_dir` of a
/// repository rooted at `root`.
fn violations(root: &Path, manifest_dir: &Path, manifest: &Table, shown_as: &str) -> Vec<String> {
  let mut found = Vec::new();
  for table in dependency_tables(manifest) {
    for (key, spec) in table {
      let detail = spec.as_table();
      let name = detail.and_then(|d| d.get("package")).and_then(Value::as_str).unwrap_or(key);
      let path = detail.and_then(|d| d.get("path")).and_then(Value::as_str);
      let git = detail.and_then(|d| d.get("git")).and_then(Value::as_str);
      if let Some(git) = git {
        found.push(format!("{shown_as}: {key} comes from another repository through {git}"));
      }
      match path {
        Some(path) if !lexical(&manifest_dir.join(path)).starts_with(root) => {
          found.push(format!("{shown_as}: {key} reaches outside the repository through {path}"));
        }
        Some(_) => {}
        None if name.starts_with("bmo-") && !BASE_LIBRARIES.contains(&name) => {
          found.push(format!("{shown_as}: {key} is another unit's crate ({name})"));
        }
        None => {}
      }
    }
  }
  found
}

#[test]
fn dependencies_stay_in_the_repository_and_name_no_other_unit() {
  let root = lexical(Path::new(ROOT));
  let found: Vec<String> = files_under(&root)
    .into_iter()
    .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
    .flat_map(|path| {
      let dir = path.parent().expect("a manifest stands in a directory").to_path_buf();
      violations(&root, &dir, &manifest(&path), &shown(&path))
    })
    .collect();
  assert!(found.is_empty(), "these dependencies break the rules: {found:?}");
}

#[test]
fn the_schedule_crate_has_no_dependency() {
  let schedule = manifest(&Path::new(ROOT).join(SCHEDULE).join("Cargo.toml"));
  let tables = dependency_tables(&schedule);
  assert!(tables.is_empty(), "the schedule crate declares dependencies: {tables:?}");
}

#[test]
fn every_kind_of_dependency_table_is_read() {
  let manifest: Table = r#"
    [dependencies]
    [dev-dependencies]
    [build-dependencies]
    [target.'cfg(unix)'.dependencies]
    [workspace.dependencies]
    [patch.crates-io]
  "#
  .parse()
  .expect("the sample reads");
  assert_eq!(dependency_tables(&manifest).len(), 6);
}

#[test]
fn a_manifest_breaking_the_rules_is_named() {
  let manifest: Table = r#"
    [dependencies]
    serde = "1"
    bmo-chat = { version = "0.1" }
    bmo-agency = { git = "https://example.invalid/agency" }
    elsewhere = { git = "https://example.invalid/elsewhere", rev = "abc" }
    cache-warmer = { git = "https://example.invalid/agency", branch = "main" }
    schedule = { package = "cache-warmer", git = "https://example.invalid/agency", rev = "abc" }
    renamed = { package = "bmo-receiver", version = "0.1" }
    inside = { path = "src/inside" }
    outside = { path = "../../elsewhere" }

    [target.'cfg(unix)'.dev-dependencies]
    bmo-ui = "0.1"

    [workspace.dependencies]
    bmo-testbed = "0.1"
  "#
  .parse()
  .expect("the sample reads");
  let root = Path::new("/work/repository");
  let mut found = violations(root, root, &manifest, "sample");
  found.sort();
  assert_eq!(
    found,
    [
      "sample: bmo-agency comes from another repository through https://example.invalid/agency",
      "sample: bmo-agency is another unit's crate (bmo-agency)",
      "sample: bmo-chat is another unit's crate (bmo-chat)",
      "sample: bmo-testbed is another unit's crate (bmo-testbed)",
      "sample: bmo-ui is another unit's crate (bmo-ui)",
      "sample: cache-warmer comes from another repository through https://example.invalid/agency",
      "sample: elsewhere comes from another repository through https://example.invalid/elsewhere",
      "sample: outside reaches outside the repository through ../../elsewhere",
      "sample: renamed is another unit's crate (bmo-receiver)",
      "sample: schedule comes from another repository through https://example.invalid/agency",
    ]
  );
}
