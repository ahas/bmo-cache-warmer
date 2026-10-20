//! No file of the checkout holds personal data (docs/PERSONAL-DATA.md): no home path, mail address,
//! telephone number, token or private network address. The check names nobody: it matches shapes, and
//! reports the file, the line and the kind, never the value it found.

mod support;

use std::fs;
use std::path::Path;

use regex::Regex;

use support::{ROOT, files_under, shown};

/// One form the checkout may not hold, and what a failure calls it.
struct Sought {
  kind: &'static str,
  pattern: Regex,
}

/// A home directory as an account's is written: the folder followed by a name, on a Unix-like path or
/// a Windows one.
fn home_path() -> Sought {
  Sought {
    kind: "a home path",
    pattern: Regex::new(r"(?:/home|/Users|/root)/[\w.-]|[A-Za-z]:\\Users\\[\w.-]")
      .expect("the home path builds"),
  }
}

/// A mail address.
fn mail_address() -> Sought {
  Sought {
    kind: "a mail address",
    pattern: Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
      .expect("the mail address builds"),
  }
}

/// A telephone number written in groups, with an optional country or area code.
fn telephone_number() -> Sought {
  Sought {
    kind: "a telephone number",
    pattern: Regex::new(r"(?:\+\d{1,3}[ .-]?)?(?:\(\d{1,4}\)[ .-]?)?\d{2,4}[ .-]\d{3,4}[ .-]\d{4}\b")
      .expect("the telephone number builds"),
  }
}

/// A token: a long value assigned to a secret's name, or the shape a vendor gives its keys.
fn token() -> Sought {
  let pattern = concat!(
    r"(?i)(?:\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|token|secret|key|",
    r#"client[_-]?secret|password|passwd|bearer)\b\s*[:=]\s*["']?[A-Za-z0-9_+/=.-]{16,}"#,
    r"|\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}",
    r"|\bsk-[A-Za-z0-9_-]{20,}|\bxox[abprs]-[A-Za-z0-9-]{10,}",
    r"|\bAKIA[0-9A-Z]{16}\b|\bAIza[0-9A-Za-z_-]{35}\b",
    r"|\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})"
  );
  Sought {
    kind: "a token",
    pattern: Regex::new(pattern).expect("the token builds"),
  }
}

/// A private or link-local network address. A loopback address is neither and passes.
fn private_address() -> Sought {
  let pattern = concat!(
    r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}",
    r"|172\.(?:1[6-9]|2[0-9]|3[01])\.\d{1,3}\.\d{1,3}",
    r"|192\.168\.\d{1,3}\.\d{1,3}",
    r"|169\.254\.\d{1,3}\.\d{1,3})\b"
  );
  Sought {
    kind: "a private network address",
    pattern: Regex::new(pattern).expect("the private address builds"),
  }
}

/// The forms no file of the checkout may hold (docs/PERSONAL-DATA.md).
fn forms() -> Vec<Sought> {
  vec![home_path(), mail_address(), telephone_number(), token(), private_address()]
}

/// Each line of `text` holding one of `forms`, as `path:line: kind`.
fn found_lines(path: &Path, text: &str, forms: &[Sought]) -> Vec<String> {
  text
    .lines()
    .enumerate()
    .flat_map(|(number, line)| {
      forms
        .iter()
        .filter(|form| form.pattern.is_match(line))
        .map(move |form| format!("{}:{}: {}", shown(path), number + 1, form.kind))
    })
    .collect()
}

#[test]
fn no_file_of_the_checkout_holds_personal_data() {
  let forms = forms();
  let found: Vec<String> = files_under(Path::new(ROOT))
    .into_iter()
    .flat_map(|path| {
      let bytes = fs::read(&path).expect("a file is readable");
      found_lines(&path, &String::from_utf8_lossy(&bytes), &forms)
    })
    .collect();
  assert!(found.is_empty(), "these lines hold personal data (docs/PERSONAL-DATA.md): {found:#?}");
}

#[test]
fn every_form_finds_its_own_and_passes_the_ordinary() {
  // Each sample is put together here, so this file holds no value of its own.
  let home = format!("/{}/", "home");
  let mail = format!("{}{}", "a.person", "@example.org");
  let telephone = format!("{}-{}-{}", "010", "1234", "5678");
  let private = format!("{}.{}.{}.{}", 192, 168, 0, 10);
  let key_line = format!("token = \"{}\"", "abcdefghijklmnop1234");
  let forms = forms();
  let text = [
    format!("cd {home}someone/work"),
    format!("mail {mail}"),
    format!("call {telephone}"),
    key_line,
    format!("serve on {private}"),
    "the loopback 127.0.0.1:7841 is ordinary".to_owned(),
    "version 0.1.0-rev.5".to_owned(),
  ]
  .join("\n");
  assert_eq!(
    found_lines(Path::new("sample"), &text, &forms),
    [
      "sample:1: a home path",
      "sample:2: a mail address",
      "sample:3: a telephone number",
      "sample:4: a token",
      "sample:5: a private network address",
    ]
  );
}
