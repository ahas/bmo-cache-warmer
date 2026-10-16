//! The service's own word shape (docs/api.md, "Conventions").

/// Whether a word is snake_case: lower-case ASCII letters, digits and underscores, beginning with a
/// letter, at most 64 bytes.
pub fn is_snake_case(word: &str) -> bool {
  let mut bytes = word.bytes();

  word.len() <= 64
    && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
    && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}
