//! The store (docs/store.md): what each write keeps and refuses, and how its shape moves.

mod fixtures;

use bmo_cache_warmer::migration::{Migrator, MigratorTrait};
use bmo_cache_warmer::store::{Attempt, Outcome, Store, StoreError};
use fixtures::{IDLE, MINUTE, NOW, line, place, store};

const HOOK: &str = "http://127.0.0.1:1/ping";

fn attempt(at: i64, outcome: Outcome) -> Attempt {
  Attempt {
    at,
    outcome,
    code: None,
    reason: None,
  }
}

#[tokio::test]
async fn a_new_store_has_its_round_running_and_keeps_what_it_holds_across_opening() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let path = dir.path().join("warmer.sqlite");

  let store = Store::open(&path).await.expect("the store opens");
  assert!(!store.round_stopped().await.expect("the round reads"));
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  store.set_round_stopped(true).await.expect("the round stops");
  drop(store);

  let again = Store::open(&path).await.expect("the store opens again");
  assert!(again.round_stopped().await.expect("the round reads"));
  assert_eq!(again.hook_url("agency").await.expect("a hook reads").as_deref(), Some(HOOK));
}

#[tokio::test]
async fn the_migration_runs_down_and_up_again() {
  let (_dir, store) = store().await;
  let db = store.connection();

  Migrator::down(db, None).await.expect("the shape goes down");
  assert!(store.round_stopped().await.is_err());
  Migrator::up(db, None).await.expect("the shape comes up");
  assert!(!store.round_stopped().await.expect("the round reads"));
}

#[tokio::test]
async fn a_consumer_is_new_once_and_its_hook_moves() {
  let (_dir, store) = store().await;

  assert!(store.put_consumer("agency", HOOK).await.expect("a consumer"));
  assert!(!store.put_consumer("agency", "http://127.0.0.1:2/ping").await.expect("a consumer"));
  assert_eq!(
    store.hook_url("agency").await.expect("a hook").as_deref(),
    Some("http://127.0.0.1:2/ping")
  );
  assert_eq!(store.hook_url("nobody").await.expect("no hook"), None);
}

#[tokio::test]
async fn a_group_needs_its_consumer_and_is_not_retired_while_named() {
  let (_dir, store) = store().await;

  assert!(matches!(store.put_group("agency", "opus", 2).await, Err(StoreError::UnknownConsumer)));
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  assert!(store.put_group("agency", "opus", 2).await.expect("a group"));
  assert!(!store.put_group("agency", "opus", 3).await.expect("a group's limit"));
  store.put_target("agency", "place", &place("owner", "opus")).await.expect("a place");

  let targets = store.targets(None, None).await.expect("the list");
  assert_eq!(targets[0].group.as_ref().map(|group| group.limit), Some(3));
  assert!(matches!(store.delete_group("agency", "opus").await, Err(StoreError::GroupInUse)));
  store.delete_target("agency", "place").await.expect("the place retires");
  store.delete_group("agency", "opus").await.expect("the group retires");
  assert!(matches!(store.delete_group("agency", "opus").await, Err(StoreError::UnknownGroup)));
}

#[tokio::test]
async fn a_target_needs_its_consumer_and_its_group() {
  let (_dir, store) = store().await;

  assert!(matches!(
    store.put_target("agency", "line", &line("a")).await,
    Err(StoreError::UnknownConsumer)
  ));
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  assert!(matches!(
    store.put_target("agency", "place", &place("a", "missing")).await,
    Err(StoreError::UnknownGroup)
  ));
  // A group of another consumer is not this one's.
  store.put_consumer("other", HOOK).await.expect("a consumer");
  store.put_group("other", "opus", 1).await.expect("a group");
  assert!(matches!(
    store.put_target("agency", "place", &place("a", "opus")).await,
    Err(StoreError::UnknownGroup)
  ));
}

#[tokio::test]
async fn restating_a_target_keeps_its_record_and_takes_only_a_later_use() {
  let (_dir, store) = store().await;
  store.put_consumer("agency", HOOK).await.expect("a consumer");

  let mut registration = line("worker");
  registration.last_used_at = Some(NOW - IDLE);
  assert!(store.put_target("agency", "line", &registration).await.expect("a target"));
  store
    .record_attempt("agency", "line", &attempt(NOW, Outcome::Warmed))
    .await
    .expect("an attempt");

  registration.label = "restated".into();
  registration.rank = 7;
  registration.last_used_at = Some(NOW - 2 * IDLE);
  assert!(!store.put_target("agency", "line", &registration).await.expect("a restatement"));
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!((target.label.as_str(), target.rank), ("restated", 7));
  assert_eq!(target.record.last_used_at, Some(NOW - IDLE));
  assert_eq!(target.record.last_warmed_at, Some(NOW));

  registration.last_used_at = None;
  store.put_target("agency", "line", &registration).await.expect("a restatement");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_used_at, Some(NOW - IDLE));

  registration.last_used_at = Some(NOW - MINUTE);
  store.put_target("agency", "line", &registration).await.expect("a restatement");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_used_at, Some(NOW - MINUTE));
}

#[tokio::test]
async fn retiring_a_target_takes_its_record_and_leaves_its_subject_switched() {
  let (_dir, store) = store().await;
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  store.put_target("agency", "line", &line("worker")).await.expect("a target");
  store.set_switch("agency", "worker", Some(true)).await.expect("the switch");
  store
    .record_attempt("agency", "line", &attempt(NOW, Outcome::Busy))
    .await
    .expect("an attempt");

  store.delete_target("agency", "line").await.expect("the target retires");
  assert!(matches!(store.delete_target("agency", "line").await, Err(StoreError::UnknownTarget)));
  assert!(store.attempts("agency", "line").await.expect("the log").is_empty());

  let subjects = store.subjects("agency").await.expect("the subjects");
  assert_eq!(subjects.len(), 1);
  assert_eq!((subjects[0].switch, subjects[0].targets), (Some(true), 0));

  // Registered again, it starts with no record.
  store.put_target("agency", "line", &line("worker")).await.expect("a target");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.last_attempt, None);
  assert_eq!(target.switch, Some(true));
}

#[tokio::test]
async fn a_use_moves_only_forward_and_clears_a_pause_only_for_work_begun_after_it() {
  let (_dir, store) = store().await;
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  store.put_target("agency", "line", &line("worker")).await.expect("a target");
  assert!(matches!(
    store.report_use("agency", "missing", NOW, NOW).await,
    Err(StoreError::UnknownTarget)
  ));

  store.report_use("agency", "line", NOW, NOW).await.expect("a use");
  store
    .report_use("agency", "line", NOW - MINUTE, NOW - MINUTE)
    .await
    .expect("an older use");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_used_at, Some(NOW));

  let paused = store.pause("agency", "worker", NOW + MINUTE).await.expect("a pause");
  assert_eq!(paused.paused_at, Some(NOW + MINUTE));
  store
    .report_use("agency", "line", NOW + 2 * MINUTE, NOW)
    .await
    .expect("work begun before the pause");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.paused_at, Some(NOW + MINUTE));
  assert_eq!(target.record.last_used_at, Some(NOW + 2 * MINUTE));

  store
    .report_use("agency", "line", NOW + 3 * MINUTE, NOW + 2 * MINUTE)
    .await
    .expect("work begun after");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.paused_at, None);
}

#[tokio::test]
async fn a_switch_is_made_before_any_target_and_unset_with_none() {
  let (_dir, store) = store().await;
  assert!(matches!(
    store.set_switch("agency", "worker", Some(true)).await,
    Err(StoreError::UnknownConsumer)
  ));
  assert!(matches!(store.pause("agency", "worker", NOW).await, Err(StoreError::UnknownConsumer)));
  assert!(matches!(store.subjects("agency").await, Err(StoreError::UnknownConsumer)));
  store.put_consumer("agency", HOOK).await.expect("a consumer");

  let switched = store.set_switch("agency", "worker", Some(false)).await.expect("the switch");
  assert_eq!(
    (switched.subject.as_str(), switched.switch, switched.targets),
    ("worker", Some(false), 0)
  );

  let mut registration = line("worker");
  registration.warm_by_default = Some(true);
  store.put_target("agency", "line", &registration).await.expect("a target");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert!(!target.on(false));

  let unset = store.set_switch("agency", "worker", None).await.expect("the switch unset");
  assert_eq!((unset.switch, unset.targets), (None, 1));
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert!(target.on(false));
}

#[tokio::test]
async fn targets_list_by_rank_then_consumer_then_name_and_narrow() {
  let (_dir, store) = store().await;
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  store.put_consumer("receiver", HOOK).await.expect("a consumer");
  store.put_group("agency", "opus", 1).await.expect("a group");

  store.put_target("receiver", "chat", &line("chat")).await.expect("a target");
  store.put_target("agency", "b", &line("worker")).await.expect("a target");
  store.put_target("agency", "a", &line("other")).await.expect("a target");
  store.put_target("agency", "place", &place("owner", "opus")).await.expect("a target");

  let names = |targets: Vec<bmo_cache_warmer::store::Target>| -> Vec<String> {
    targets
      .into_iter()
      .map(|target| format!("{}/{}", target.consumer, target.name))
      .collect()
  };
  assert_eq!(
    names(store.targets(None, None).await.expect("the list")),
    vec!["agency/place", "agency/a", "agency/b", "receiver/chat"]
  );
  assert_eq!(
    names(store.targets(Some("receiver"), None).await.expect("the list")),
    vec!["receiver/chat"]
  );
  assert_eq!(names(store.targets(None, Some("worker")).await.expect("the list")), vec!["agency/b"]);
}

#[tokio::test]
async fn an_attempt_moves_only_the_times_its_outcome_moves() {
  let (_dir, store) = store().await;
  store.put_consumer("agency", HOOK).await.expect("a consumer");
  store.put_target("agency", "line", &line("worker")).await.expect("a target");

  let failed = Attempt {
    at: NOW,
    outcome: Outcome::Failed,
    code: Some("hook_status".into()),
    reason: Some("The hook answered status 500.".into()),
  };
  store.record_attempt("agency", "line", &failed).await.expect("an attempt");
  store
    .record_attempt("agency", "line", &attempt(NOW + 1, Outcome::Skipped))
    .await
    .expect("an attempt");
  store
    .record_attempt("agency", "line", &attempt(NOW + 2, Outcome::Busy))
    .await
    .expect("an attempt");

  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_failed_at, Some(NOW));
  assert_eq!(target.record.last_warmed_at, None);
  assert_eq!(target.last_attempt, Some(attempt(NOW + 2, Outcome::Busy)));

  store
    .record_attempt("agency", "line", &attempt(NOW + 3, Outcome::Warmed))
    .await
    .expect("an attempt");
  let target = store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_warmed_at, Some(NOW + 3));
  assert_eq!(target.record.last_failed_at, Some(NOW));

  let log = store.attempts("agency", "line").await.expect("the log");
  assert_eq!(log.len(), 4);
  assert_eq!(log[0], failed);

  assert!(
    !store
      .record_attempt("agency", "gone", &attempt(NOW, Outcome::Warmed))
      .await
      .expect("nothing")
  );
  assert_eq!(store.prune_attempts(NOW + 2).await.expect("the log pruned"), 2);
  assert_eq!(store.attempts("agency", "line").await.expect("the log").len(), 2);
}

#[tokio::test]
async fn a_clock_out_of_its_range_is_refused_by_the_store_itself() {
  let (_dir, store) = store().await;
  store.put_consumer("agency", HOOK).await.expect("a consumer");

  let mut registration = line("worker");
  registration.clocks.idle_ms = 0;
  assert!(matches!(
    store.put_target("agency", "line", &registration).await,
    Err(StoreError::Database(_))
  ));
  assert!(store.targets(None, None).await.expect("the list").is_empty());
}
