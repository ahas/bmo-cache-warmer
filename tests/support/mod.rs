//! What the tests share: the root, the workspace's members and the walk over the files the repository
//! checks read.

use std::fs;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

pub const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// A manifest, read.
pub fn manifest(path: &Path) -> Table {
  let text =
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{} is unreadable: {error}", shown(path)));
  text.parse().unwrap_or_else(|error| panic!("{} reads: {error}", shown(path)))
}

/// The directory of each member the root manifest's workspace names, the root itself included.
pub fn members() -> Vec<PathBuf> {
  let root = manifest(&Path::new(ROOT).join("Cargo.toml"));
  let listed = root
    .get("workspace")
    .and_then(|workspace| workspace.get("members"))
    .and_then(Value::as_array)
    .expect("the root manifest names the workspace's members")
    .iter()
    .map(|member| Path::new(ROOT).join(member.as_str().expect("a member is a path")));
  // The root package is a member of its own workspace whether or not the manifest lists it.
  std::iter::once(Path::new(ROOT).to_path_buf()).chain(listed).collect()
}

/// Directories no check reads: build output and git's own.
const SKIPPED: [&str; 2] = ["target", ".git"];

pub fn is_skipped(name: &str) -> bool {
  SKIPPED.contains(&name)
}

/// Every file under `dir`, at any depth, skipping build output and git's directory. A directory that
/// does not exist holds no files.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  if dir.is_dir() {
    collect(dir, &mut files);
  }
  files.sort();
  files
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
  for entry in fs::read_dir(dir).expect("a directory is readable") {
    let entry = entry.expect("an entry is readable");
    let kind = entry.file_type().expect("an entry's type is readable");
    if kind.is_dir() {
      if !is_skipped(&entry.file_name().to_string_lossy()) {
        collect(&entry.path(), files);
      }
    } else if kind.is_file() {
      files.push(entry.path());
    }
  }
}

/// A path as it reads relative to the root, for a failure's message.
pub fn shown(path: &Path) -> String {
  path.strip_prefix(ROOT).unwrap_or(path).display().to_string()
}
