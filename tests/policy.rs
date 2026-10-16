//! The policy file and its reader (docs/warming.md, "The policy").

use std::time::Duration;

use bmo_cache_warmer::policy::{Policy, PolicyError};

const SAMPLE: &str = "
start_delay_seconds = 30
tick_seconds = 60
warm_by_default = false
ping_slack_seconds = 15
reason_units = 400
log_days = 14
";

#[test]
fn the_built_in_policy_reads_as_documented() {
  let policy = Policy::builtin().expect("the built-in policy reads");
  assert_eq!(
    policy,
    Policy {
      start_delay_seconds: 30,
      tick_seconds: 60,
      warm_by_default: false,
      ping_slack_seconds: 15,
      reason_units: 400,
      log_days: 14,
    }
  );
  assert_eq!(policy.start_delay(), Duration::from_secs(30));
  assert_eq!(policy.tick(), Duration::from_secs(60));
  assert_eq!(policy.ping_slack(), Duration::from_secs(15));
  assert_eq!(policy.log_keep_ms(), 14 * 24 * 60 * 60 * 1000);
}

#[test]
fn an_unknown_key_is_refused_by_name() {
  let text = format!("{SAMPLE}tick_minutes = 1\n");
  let refused = Policy::read(&text).expect_err("an unknown key is refused");
  assert!(matches!(refused, PolicyError::Malformed(_)));
  assert!(refused.to_string().contains("tick_minutes"), "{refused}");
}

#[test]
fn a_missing_key_is_refused_by_name() {
  let text = SAMPLE.replace("reason_units = 400\n", "");
  let refused = Policy::read(&text).expect_err("a missing key is refused");
  assert!(refused.to_string().contains("reason_units"), "{refused}");
}

#[test]
fn a_clock_of_zero_is_refused() {
  for (stated, key) in [
    ("tick_seconds = 60", "tick_seconds"),
    ("reason_units = 400", "reason_units"),
    ("log_days = 14", "log_days"),
  ] {
    let zero = format!("{key} = 0");
    let refused = Policy::read(&SAMPLE.replace(stated, &zero)).expect_err("a zero is refused");
    assert!(
      matches!(refused, PolicyError::Zero { key: refused_key } if refused_key == key),
      "{refused}"
    );
  }
}
