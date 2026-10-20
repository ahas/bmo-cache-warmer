//! The service's own word and name shapes (docs/api.md, "Wire format").

/// Whether a name — a consumer, a target, a subject, a group — has its shape: not empty, at most 256
/// bytes.
pub fn is_name(name: &str) -> bool {
  !name.is_empty() && name.len() <= 256
}

/// Whether a word is a kind word: kebab-case, lower-case ASCII letters, digits and hyphens, beginning with
/// a letter, at most 64 bytes.
pub fn is_kind(word: &str) -> bool {
  let mut bytes = word.bytes();

  word.len() <= 64
    && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
    && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
