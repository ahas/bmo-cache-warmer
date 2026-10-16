//! The due rule, groups, ranks, the owner's switches and the reason cut (docs/warming.md, "When a target
//! is due", "Groups", "The owner's switches" and "What an answer leaves behind").

use bmo_cache_warmer::due::{
  Candidate, Clocks, Group, Pass, Record, first_units, is_on, plan, usable, use_clears_pause,
};

const MINUTE: i64 = 60_000;
const IDLE: i64 = 40 * MINUTE;
const NOW: i64 = 10_000_000_000;

/// A worker session's clocks: idle 40 minutes, no lifetime, failures touch.
fn session() -> Clocks {
  Clocks {
    idle_ms: IDLE,
    lifetime_ms: None,
    margin_ms: 0,
    failure_touches: true,
    honours_idle_override: true,
  }
}

/// A launch line's clocks: idle 40 minutes, lifetime 60, no margin, failures touch.
fn line() -> Clocks {
  Clocks {
    lifetime_ms: Some(60 * MINUTE),
    ..session()
  }
}

/// A pool place's clocks: idle 40 minutes, lifetime 60 less a 5-minute margin, failures do not touch, the
/// idle override not honoured.
fn place() -> Clocks {
  Clocks {
    idle_ms: IDLE,
    lifetime_ms: Some(60 * MINUTE),
    margin_ms: 5 * MINUTE,
    failure_touches: false,
    honours_idle_override: false,
  }
}

fn used(at: i64) -> Record {
  Record {
    last_used_at: Some(at),
    ..Record::default()
  }
}

fn warmed(at: i64) -> Record {
  Record {
    last_warmed_at: Some(at),
    ..Record::default()
  }
}

fn failed(at: i64) -> Record {
  Record {
    last_failed_at: Some(at),
    ..Record::default()
  }
}

fn target(consumer: &str, name: &str, rank: i64, clocks: Clocks, record: Record) -> Candidate {
  Candidate {
    consumer: consumer.into(),
    target: name.into(),
    rank,
    group: None,
    clocks,
    record,
    on: true,
    paused: false,
  }
}

fn grouped(name: &str, rank: i64, group: &str, limit: usize, record: Record) -> Candidate {
  Candidate {
    group: Some(Group {
      name: group.into(),
      limit,
    }),
    ..target("agency", name, rank, place(), record)
  }
}

fn pass() -> Pass {
  Pass {
    now: NOW,
    idle_override_ms: None,
    stopped: false,
  }
}

/// The names of what a pass pings, in order.
fn pinged(candidates: &[Candidate], pass: &Pass) -> Vec<String> {
  plan(candidates, pass)
    .into_iter()
    .map(|index| candidates[index].target.clone())
    .collect()
}

#[test]
fn a_session_idle_past_the_mark_is_due_and_a_fresh_one_is_not() {
  let sessions = [
    target("agency", "opus", 80, session(), used(NOW - IDLE)),
    target("agency", "sonnet", 80, session(), used(NOW)),
  ];

  assert_eq!(pinged(&sessions, &pass()), vec!["opus".to_owned()]);
}

#[test]
fn the_default_model_with_no_session_is_brought_up_once_its_last_ping_is_idle() {
  // The default model with no session is registered with no last use, so its own pings are its touch.
  let never = [target("agency", "opus", 80, session(), Record::default())];
  assert_eq!(pinged(&never, &pass()), vec!["opus".to_owned()]);

  let just_warmed = [target("agency", "opus", 80, session(), warmed(NOW))];
  assert!(pinged(&just_warmed, &pass()).is_empty());
  let just_failed = [target("agency", "opus", 80, session(), failed(NOW))];
  assert!(pinged(&just_failed, &pass()).is_empty());

  // Both idle for the stretch: both are due, in target-name order.
  let both = [
    target("agency", "sonnet", 80, session(), used(NOW - IDLE)),
    target("agency", "opus", 80, session(), failed(NOW - IDLE)),
  ];
  assert_eq!(pinged(&both, &pass()), vec!["opus".to_owned(), "sonnet".to_owned()]);
}

#[test]
fn the_margin_is_taken_off_a_lifetime_and_nothing_is_left_of_a_short_one() {
  assert_eq!(usable(60 * MINUTE, 5 * MINUTE), Some(55 * MINUTE));
  assert_eq!(usable(5 * MINUTE, 5 * MINUTE), None);

  let clocks = place();
  assert!(clocks.due(&warmed(NOW - 54 * MINUTE), NOW, None));
  assert!(!clocks.due(&warmed(NOW - 55 * MINUTE), NOW, None));

  let short = Clocks {
    lifetime_ms: Some(5 * MINUTE),
    ..place()
  };
  assert!(!short.due(&Record::default(), NOW, None));
  assert_eq!(short.due_at(&Record::default(), NOW), None);
}

#[test]
fn a_reason_is_cut_by_the_units_a_script_counts() {
  assert_eq!(first_units("αβγ", 2), "αβ");
  assert_eq!(first_units("ab", 400), "ab");
  // A pair split by the cut ends as the replacement character.
  assert_eq!(first_units("a\u{1F600}", 2), "a\u{FFFD}");
}

#[test]
fn a_line_is_due_from_its_idle_stretch_until_its_cache_is_gone() {
  let clocks = line();

  assert!(clocks.due(&Record::default(), NOW, None));
  assert!(!clocks.due(&used(NOW - IDLE + 1), NOW, None));
  assert!(clocks.due(&used(NOW - IDLE), NOW, None));
  assert!(clocks.due(&used(NOW - 60 * MINUTE + 1), NOW, None));
  assert!(!clocks.due(&used(NOW - 60 * MINUTE), NOW, None));
}

#[test]
fn the_touch_is_the_latest_of_use_warmed_and_a_touching_failure() {
  let record = Record {
    last_used_at: Some(NOW - 50 * MINUTE),
    last_warmed_at: Some(NOW - 45 * MINUTE),
    last_failed_at: Some(NOW - MINUTE),
  };

  assert_eq!(line().touch(&record), Some(NOW - MINUTE));
  assert_eq!(place().touch(&record), Some(NOW - 45 * MINUTE));
  assert_eq!(place().touch(&Record::default()), None);

  // A pool place is asked again after a failure; a line holds off for its idle stretch.
  let warm_then_failed = Record {
    last_warmed_at: Some(NOW - 45 * MINUTE),
    last_failed_at: Some(NOW - MINUTE),
    ..Record::default()
  };
  assert!(place().due(&warm_then_failed, NOW, None));
  assert!(!line().due(&warm_then_failed, NOW, None));
}

#[test]
fn an_idle_override_applies_only_where_it_is_honoured() {
  let record = warmed(NOW - 2 * MINUTE);

  assert!(!line().due(&record, NOW, None));
  assert!(line().due(&record, NOW, Some(MINUTE)));
  assert!(!place().due(&record, NOW, Some(MINUTE)));
}

#[test]
fn targets_go_by_rank_then_consumer_then_name() {
  let targets = [
    target("receiver", "chat", 90, line(), Record::default()),
    target("agency", "worker-b", 80, session(), Record::default()),
    target("agency", "worker-a", 80, session(), Record::default()),
    target("other", "first", 10, line(), Record::default()),
    target("agency", "late", 10, line(), Record::default()),
  ];

  assert_eq!(pinged(&targets, &pass()), vec!["late", "first", "worker-a", "worker-b", "chat"]);
}

#[test]
fn a_group_pings_its_oldest_touch_first_up_to_its_limit() {
  let places = [
    grouped("fresh", 10, "opus", 2, warmed(NOW - 41 * MINUTE)),
    grouped("old", 10, "opus", 2, warmed(NOW - 50 * MINUTE)),
    grouped("never", 10, "opus", 2, Record::default()),
    grouped("cool", 10, "opus", 2, warmed(NOW - MINUTE)),
  ];

  assert_eq!(pinged(&places, &pass()), vec!["never", "old"]);

  let none = [grouped("old", 10, "opus", 0, warmed(NOW - 50 * MINUTE))];
  assert!(pinged(&none, &pass()).is_empty());
}

#[test]
fn a_target_not_due_leaves_its_seat_to_the_next() {
  let places = [
    grouped("cool", 10, "opus", 1, warmed(NOW - MINUTE)),
    grouped("gone", 10, "opus", 1, warmed(NOW - 58 * MINUTE)),
    grouped("due", 10, "opus", 1, warmed(NOW - 42 * MINUTE)),
  ];

  assert_eq!(pinged(&places, &pass()), vec!["due"]);
}

#[test]
fn a_group_goes_together_where_its_first_target_stands() {
  let targets = [
    target("agency", "line", 50, line(), Record::default()),
    grouped("late-place", 60, "opus", 2, Record::default()),
    grouped("early-place", 5, "opus", 2, warmed(NOW - 45 * MINUTE)),
    target("agency", "stand-in", 20, line(), Record::default()),
  ];

  assert_eq!(pinged(&targets, &pass()), vec!["late-place", "early-place", "stand-in", "line"]);
}

#[test]
fn the_same_group_name_under_two_consumers_is_two_groups() {
  let places = [
    grouped("a", 10, "opus", 1, Record::default()),
    Candidate {
      consumer: "other".into(),
      ..grouped("b", 10, "opus", 1, Record::default())
    },
  ];

  assert_eq!(pinged(&places, &pass()), vec!["a", "b"]);
}

#[test]
fn a_switch_thrown_wins_over_the_registration_and_the_policy() {
  assert!(is_on(Some(true), Some(false), false));
  assert!(!is_on(Some(false), Some(true), true));
  assert!(is_on(None, Some(true), false));
  assert!(!is_on(None, Some(false), true));
  assert!(!is_on(None, None, false));
  assert!(is_on(None, None, true));

  let off = Candidate {
    on: false,
    ..target("agency", "worker", 80, session(), Record::default())
  };
  assert!(pinged(std::slice::from_ref(&off), &pass()).is_empty());
  assert_eq!(off.due_at(NOW), None);
}

#[test]
fn a_paused_subject_is_not_due_until_work_begun_after_the_pause_is_reported() {
  let paused = Candidate {
    paused: true,
    ..target("agency", "worker", 80, session(), Record::default())
  };
  assert!(!paused.is_due(&pass()));
  assert_eq!(paused.due_at(NOW), None);

  let paused_at = NOW - MINUTE;
  assert!(use_clears_pause(paused_at, paused_at + 1));
  assert!(!use_clears_pause(paused_at, paused_at));
  assert!(!use_clears_pause(paused_at, paused_at - 1));
}

#[test]
fn a_stopped_round_pings_nothing() {
  let targets = [target("agency", "line", 50, line(), Record::default())];
  let stopped = Pass {
    stopped: true,
    ..pass()
  };

  assert!(!targets[0].is_due(&stopped));
  assert!(plan(&targets, &stopped).is_empty());
}

#[test]
fn the_list_shows_when_a_target_is_next_due() {
  assert_eq!(line().due_at(&Record::default(), NOW), Some(NOW));
  assert_eq!(line().due_at(&used(NOW - MINUTE), NOW), Some(NOW - MINUTE + IDLE));
  assert_eq!(line().due_at(&used(NOW - 60 * MINUTE), NOW), None);
  assert_eq!(session().due_at(&used(NOW - 600 * MINUTE), NOW), Some(NOW - 600 * MINUTE + IDLE));

  let no_room = Clocks {
    idle_ms: 56 * MINUTE,
    ..place()
  };
  assert_eq!(no_room.due_at(&Record::default(), NOW), Some(NOW));
  assert_eq!(no_room.due_at(&warmed(NOW - MINUTE), NOW), None);
}
