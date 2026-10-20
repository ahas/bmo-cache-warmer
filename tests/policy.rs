//! The policy file and its reader (docs/warming.md, "The policy").

use std::time::Duration;

use bmo_cache_warmer::policy::{Policy, PolicyError};

const SAMPLE: &str = "
start_delay_seconds = 30
tick_seconds = 60
warm_by_default = false
idle_percent = 66
ping_bound_seconds = 120
ping_slack_seconds = 15
reason_units = 400
log_days = 14
event_days = 2
";

#[test]
fn the_built_in_policy_reads_as_documented() {
  let policy = Policy::builtin().expect("the built-in policy reads");
  assert_eq!(policy, Policy::read(SAMPLE).expect("the sample reads"));
  assert_eq!(policy.start_delay(), Duration::from_secs(30));
  assert_eq!(policy.tick(), Duration::from_secs(60));
  assert_eq!(policy.tick_ms(), 60_000);
  assert_eq!(policy.ping_slack(), Duration::from_secs(15));
  assert_eq!(policy.ping_bound_ms(), 120_000);
  assert_eq!(policy.log_keep_ms(), 14 * 24 * 60 * 60 * 1000);
  assert_eq!(policy.event_keep_ms(), 2 * 24 * 60 * 60 * 1000);
}

#[test]
fn an_idle_stretch_left_out_is_the_share_of_the_usable_lifetime() {
  let policy = Policy::read(SAMPLE).unwrap();

  assert_eq!(policy.idle_ms(60 * 60_000, 0), 2_376_000);
  assert_eq!(policy.idle_ms(60 * 60_000, 10 * 60_000), 1_980_000);
  // Nothing usable still leaves a stretch above 0, and the target is never due by its lifetime.
  assert_eq!(policy.idle_ms(5, 5), 1);
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
  let text = SAMPLE.replace("event_days = 2\n", "");
  let refused = Policy::read(&text).expect_err("a missing key is refused");
  assert!(refused.to_string().contains("event_days"), "{refused}");
}

#[test]
fn a_zero_where_one_must_be_above_it_is_refused_and_so_is_a_share_over_the_whole() {
  for (stated, key) in [
    ("tick_seconds = 60", "tick_seconds"),
    ("idle_percent = 66", "idle_percent"),
    ("ping_bound_seconds = 120", "ping_bound_seconds"),
    ("reason_units = 400", "reason_units"),
    ("log_days = 14", "log_days"),
    ("event_days = 2", "event_days"),
  ] {
    let zero = format!("{key} = 0");
    let refused = Policy::read(&SAMPLE.replace(stated, &zero)).expect_err("a zero is refused");
    assert!(
      matches!(refused, PolicyError::Zero { key: refused_key } if refused_key == key),
      "{refused}"
    );
  }
  let over = Policy::read(&SAMPLE.replace("idle_percent = 66", "idle_percent = 101"));
  assert!(matches!(over, Err(PolicyError::OverWhole)));
}
