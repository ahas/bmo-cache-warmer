//! The schedule crate's rule as its contract (docs/warming.md, "When a target is due" and "Groups"): the
//! due rule, groups, ranks, the switches and the reason cut, judged from what each test hands it.

use bmo_cache_warmer_schedule::{
  Candidate, Clocks, Group, Pass, Record, first_units, is_on, plan, usable, use_clears_pause,
};

const MINUTE: i64 = 60_000;
const IDLE: i64 = 40 * MINUTE;
const NOW: i64 = 10_000_000_000;

/// A session's clocks: idle 40 minutes, no lifetime, failures touch.
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

/// A shared seat's clocks: idle 40 minutes, lifetime 60 less a 5-minute margin, failures do not touch, the
/// idle override not honoured.
fn seat() -> Clocks {
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
    ..target("alpha", name, rank, seat(), record)
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
    target("alpha", "large", 80, session(), used(NOW - IDLE)),
    target("alpha", "small", 80, session(), used(NOW)),
  ];

  assert_eq!(pinged(&sessions, &pass()), vec!["large".to_owned()]);
}

#[test]
fn a_target_never_used_is_brought_up_once_its_last_ping_is_idle() {
  // A target registered with no last use has its own pings as its touch.
  let never = [target("alpha", "large", 80, session(), Record::default())];
  assert_eq!(pinged(&never, &pass()), vec!["large".to_owned()]);

  let just_warmed = [target("alpha", "large", 80, session(), warmed(NOW))];
  assert!(pinged(&just_warmed, &pass()).is_empty());
  let just_failed = [target("alpha", "large", 80, session(), failed(NOW))];
  assert!(pinged(&just_failed, &pass()).is_empty());

  // Both idle for the stretch: both are due, in target-name order.
  let both = [
    target("alpha", "small", 80, session(), used(NOW - IDLE)),
    target("alpha", "large", 80, session(), failed(NOW - IDLE)),
  ];
  assert_eq!(pinged(&both, &pass()), vec!["large".to_owned(), "small".to_owned()]);
}

#[test]
fn the_margin_is_taken_off_a_lifetime_and_nothing_is_left_of_a_short_one() {
  assert_eq!(usable(60 * MINUTE, 5 * MINUTE), Some(55 * MINUTE));
  assert_eq!(usable(5 * MINUTE, 5 * MINUTE), None);

  let clocks = seat();
  assert!(clocks.due(&warmed(NOW - 54 * MINUTE), NOW, None));
  assert!(!clocks.due(&warmed(NOW - 55 * MINUTE), NOW, None));

  let short = Clocks {
    lifetime_ms: Some(5 * MINUTE),
    ..seat()
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
  assert_eq!(seat().touch(&record), Some(NOW - 45 * MINUTE));
  assert_eq!(seat().touch(&Record::default()), None);

  // A seat is asked again after a failure; a line holds off for its idle stretch.
  let warm_then_failed = Record {
    last_warmed_at: Some(NOW - 45 * MINUTE),
    last_failed_at: Some(NOW - MINUTE),
    ..Record::default()
  };
  assert!(seat().due(&warm_then_failed, NOW, None));
  assert!(!line().due(&warm_then_failed, NOW, None));
}

#[test]
fn an_idle_override_applies_only_where_it_is_honoured() {
  let record = warmed(NOW - 2 * MINUTE);

  assert!(!line().due(&record, NOW, None));
  assert!(line().due(&record, NOW, Some(MINUTE)));
  assert!(!seat().due(&record, NOW, Some(MINUTE)));
}

#[test]
fn targets_go_by_rank_then_consumer_then_name() {
  let targets = [
    target("gamma", "touch", 90, line(), Record::default()),
    target("alpha", "session-b", 80, session(), Record::default()),
    target("alpha", "session-a", 80, session(), Record::default()),
    target("beta", "first", 10, line(), Record::default()),
    target("alpha", "late", 10, line(), Record::default()),
  ];

  assert_eq!(pinged(&targets, &pass()), vec!["late", "first", "session-a", "session-b", "touch"]);
}

#[test]
fn a_group_pings_its_oldest_touch_first_up_to_its_limit() {
  let seats = [
    grouped("fresh", 10, "large", 2, warmed(NOW - 41 * MINUTE)),
    grouped("old", 10, "large", 2, warmed(NOW - 50 * MINUTE)),
    grouped("never", 10, "large", 2, Record::default()),
    grouped("cool", 10, "large", 2, warmed(NOW - MINUTE)),
  ];

  assert_eq!(pinged(&seats, &pass()), vec!["never", "old"]);

  let none = [grouped("old", 10, "large", 0, warmed(NOW - 50 * MINUTE))];
  assert!(pinged(&none, &pass()).is_empty());
}

#[test]
fn a_target_not_due_leaves_its_seat_to_the_next() {
  let seats = [
    grouped("cool", 10, "large", 1, warmed(NOW - MINUTE)),
    grouped("gone", 10, "large", 1, warmed(NOW - 58 * MINUTE)),
    grouped("due", 10, "large", 1, warmed(NOW - 42 * MINUTE)),
  ];

  assert_eq!(pinged(&seats, &pass()), vec!["due"]);
}

#[test]
fn a_group_goes_together_where_its_first_target_stands() {
  let targets = [
    target("alpha", "line", 50, line(), Record::default()),
    grouped("late-seat", 60, "large", 2, Record::default()),
    grouped("early-seat", 5, "large", 2, warmed(NOW - 45 * MINUTE)),
    target("alpha", "stand-in", 20, line(), Record::default()),
  ];

  assert_eq!(pinged(&targets, &pass()), vec!["late-seat", "early-seat", "stand-in", "line"]);
}

#[test]
fn the_same_group_name_under_two_consumers_is_two_groups() {
  let seats = [
    grouped("a", 10, "large", 1, Record::default()),
    Candidate {
      consumer: "beta".into(),
      ..grouped("b", 10, "large", 1, Record::default())
    },
  ];

  assert_eq!(pinged(&seats, &pass()), vec!["a", "b"]);
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
    ..target("alpha", "session", 80, session(), Record::default())
  };
  assert!(pinged(std::slice::from_ref(&off), &pass()).is_empty());
  assert_eq!(off.due_at(NOW), None);
}

#[test]
fn a_paused_subject_is_not_due_until_work_begun_after_the_pause_is_recorded() {
  let paused = Candidate {
    paused: true,
    ..target("alpha", "session", 80, session(), Record::default())
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
  let targets = [target("alpha", "line", 50, line(), Record::default())];
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
    ..seat()
  };
  assert_eq!(no_room.due_at(&Record::default(), NOW), Some(NOW));
  assert_eq!(no_room.due_at(&warmed(NOW - MINUTE), NOW), None);
}

#[test]
fn a_prefix_is_kept_below_its_usable_lifetime_and_always_where_it_declares_none() {
  assert!(seat().kept(55 * MINUTE - 1));
  assert!(!seat().kept(55 * MINUTE));
  assert!(session().kept(600 * MINUTE));
}
