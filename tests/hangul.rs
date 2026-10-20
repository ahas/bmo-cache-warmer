//! No Hangul in code or tests: the service answers in English and in codes.

mod support;

use std::fs;
use std::path::Path;

use support::{ROOT, files_under, members, shown};

/// The directories outside any package's code and tests: the examples, the plugin, policy and the deploy
/// unit.
const READ: [&str; 4] = ["examples", "plugin", "policy", "deploy"];

/// The directories holding a workspace member's code and its tests.
const MEMBER_READ: [&str; 2] = ["src", "tests"];

/// The Hangul syllables block, U+AC00 to U+D7A3, named by code point so this file holds none of it.
fn is_hangul(c: char) -> bool {
  ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

/// Each line of `text` holding Hangul, as `path:line`.
fn hangul_lines(path: &Path, text: &str) -> Vec<String> {
  text
    .lines()
    .enumerate()
    .filter(|(_, line)| line.chars().any(is_hangul))
    .map(|(number, _)| format!("{}:{}", shown(path), number + 1))
    .collect()
}

#[test]
fn no_hangul_in_code_policy_or_tests() {
  let fixed = READ.iter().map(|dir| Path::new(ROOT).join(dir));
  let member_dirs = members().into_iter().flat_map(|member| MEMBER_READ.map(|dir| member.join(dir)));
  let found: Vec<String> = fixed
    .chain(member_dirs)
    .flat_map(|dir| files_under(&dir))
    .flat_map(|path| {
      let bytes = fs::read(&path).expect("a file is readable");
      hangul_lines(&path, &String::from_utf8_lossy(&bytes))
    })
    .collect();
  assert!(found.is_empty(), "these lines hold Hangul: {found:?}");
}

#[test]
fn the_range_is_the_whole_syllables_block() {
  let first = char::from_u32(0xAC00).expect("a code point");
  let last = char::from_u32(0xD7A3).expect("a code point");
  let line = format!("ok\nsays {first}\nfine\nends {last}");
  assert_eq!(hangul_lines(Path::new("sample"), &line), ["sample:2", "sample:4"]);
  let outside = [char::from_u32(0xABFF), char::from_u32(0xD7A4)].map(|c| c.expect("a code point"));
  assert!(!outside.into_iter().any(is_hangul));
}
