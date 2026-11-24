//! The store (docs/store.md): what each write keeps and refuses, its pages and its event window, and how
//! its shape moves: from the 1.0.0 baseline, an install from before it taken as it stands.

use crate::fixtures;

use bmo_cache_warmer::migration::{BASELINE, BEFORE, Migrator, MigratorTrait, up};
use bmo_cache_warmer::store::{
  Attempt, Outcome, Paging, Pause, RoundChange, Store, StoreError, SubjectChange, Target,
};
use fixtures::{IDLE, MINUTE, NOW, line, place, store};
use sea_orm::{ConnectionTrait, DatabaseConnection, SqlxSqliteConnector};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

const HOOK: &str = "http://[::1]:1/ping";

fn attempt(at: i64, outcome: Outcome) -> Attempt {
  Attempt {
    at,
    outcome,
    code: None,
    reason: None,
  }
}

fn all() -> Paging {
  Paging {
    after: 0,
    limit: 1000,
  }
}

async fn one(store: &Store, consumer: &str, target: &str) -> Target {
  store.target(consumer, target).await.expect("the target reads").expect("it stands")
}

#[tokio::test]
async fn a_new_store_has_its_round_running_and_keeps_what_it_holds_across_opening() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let path = dir.path().join("warmer.sqlite");

  let store = Store::open(&path).await.expect("the store opens");
  let round = store.round().await.expect("the round reads");
  assert_eq!((round.stopped, round.interval_ms), (false, None));
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  let change = RoundChange {
    stopped: Some(true),
    interval_ms: Some(Some(5_000)),
  };
  store.change_round(change).await.expect("the round changes");
  drop(store);

  let again = Store::open(&path).await.expect("the store opens again");
  let round = again.round().await.expect("the round reads");
  assert_eq!((round.stopped, round.interval_ms), (true, Some(5_000)));
  assert_eq!(again.hook_url("alpha").await.expect("a hook reads").as_deref(), Some(HOOK));
}

#[tokio::test]
async fn the_migrations_run_down_and_up_again() {
  let (_dir, store) = store().await;
  let db = store.connection();

  Migrator::down(db, None).await.expect("the shape goes down");
  assert!(store.round().await.is_err());
  Migrator::up(db, None).await.expect("the shape comes up");
  assert!(!store.round_stopped().await.expect("the round reads"));
}

#[tokio::test]
async fn a_consumer_is_new_once_keeps_its_id_and_first_time_and_its_hook_moves() {
  let (_dir, store) = store().await;

  let (first, new) = store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  assert!(new);
  let (again, new) = store.put_consumer("alpha", "http://[::1]:2/ping", NOW + 1).await.expect("a consumer");
  assert!(!new);
  assert_eq!((again.id, again.created_at, again.url.as_str()), (first.id, NOW, "http://[::1]:2/ping"));
  assert_eq!(store.hook_url("nobody").await.expect("no hook"), None);
}

#[tokio::test]
async fn pages_go_by_id_and_say_whether_more_stand() {
  let (_dir, store) = store().await;
  for name in ["c", "a", "b"] {
    store.put_consumer(name, HOOK, NOW).await.expect("a consumer");
  }

  let first = store.consumers(Paging { after: 0, limit: 2 }).await.expect("a page");
  assert_eq!(
    first.items.iter().map(|consumer| consumer.name.as_str()).collect::<Vec<_>>(),
    ["c", "a"]
  );
  assert!(first.more);
  let after = first.items[1].id;
  let rest = store.consumers(Paging { after, limit: 2 }).await.expect("a page");
  assert_eq!(rest.items.iter().map(|consumer| consumer.name.as_str()).collect::<Vec<_>>(), ["b"]);
  assert!(!rest.more);
}

#[tokio::test]
async fn a_group_needs_its_consumer_lists_its_targets_and_is_not_retired_while_named() {
  let (_dir, store) = store().await;

  assert!(matches!(store.put_group("alpha", "large", 2).await, Err(StoreError::UnknownConsumer)));
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  let (group, new) = store.put_group("alpha", "large", 2).await.expect("a group");
  assert!(new && group.target_names.is_empty());
  store.put_target("alpha", "seat-b", &place("holder", "large")).await.expect("a seat");
  store.put_target("alpha", "seat-a", &place("holder", "large")).await.expect("a seat");
  let (group, new) = store.put_group("alpha", "large", 3).await.expect("a group's limit");
  assert!(!new);
  assert_eq!((group.limit, group.target_names), (3, vec!["seat-a".to_owned(), "seat-b".to_owned()]));

  assert_eq!(one(&store, "alpha", "seat-a").await.group.map(|group| group.limit), Some(3));
  assert!(matches!(store.delete_group("alpha", "large").await, Err(StoreError::GroupInUse)));
  store.delete_target("alpha", "seat-a").await.expect("the seat retires");
  store.delete_target("alpha", "seat-b").await.expect("the seat retires");
  store.delete_group("alpha", "large").await.expect("the group retires");
  assert!(matches!(store.delete_group("alpha", "large").await, Err(StoreError::UnknownGroup)));
}

#[tokio::test]
async fn a_target_needs_its_consumer_and_its_group() {
  let (_dir, store) = store().await;

  assert!(matches!(
    store.put_target("alpha", "line", &line("a")).await,
    Err(StoreError::UnknownConsumer)
  ));
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  assert!(matches!(
    store.put_target("alpha", "seat", &place("a", "missing")).await,
    Err(StoreError::UnknownGroup)
  ));
  // A group of another consumer is not this one's.
  store.put_consumer("beta", HOOK, NOW).await.expect("a consumer");
  store.put_group("beta", "large", 1).await.expect("a group");
  assert!(matches!(
    store.put_target("alpha", "seat", &place("a", "large")).await,
    Err(StoreError::UnknownGroup)
  ));
}

#[tokio::test]
async fn restating_a_target_keeps_its_id_and_record_and_takes_only_a_later_use() {
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");

  let mut registration = line("session");
  registration.last_used_at = Some(NOW - IDLE);
  assert!(store.put_target("alpha", "line", &registration).await.expect("a target"));
  let id = one(&store, "alpha", "line").await.id;
  store
    .record_attempt("alpha", "line", &attempt(NOW, Outcome::Warmed))
    .await
    .expect("an attempt");

  registration.label = Some("restated".into());
  registration.kind = None;
  registration.rank = 7;
  registration.last_used_at = Some(NOW - 2 * IDLE);
  assert!(!store.put_target("alpha", "line", &registration).await.expect("a restatement"));
  let target = one(&store, "alpha", "line").await;
  assert_eq!(
    (target.id, target.label.as_deref(), target.kind, target.rank),
    (id, Some("restated"), None, 7)
  );
  assert_eq!(target.record.last_used_at, Some(NOW - IDLE));
  assert_eq!(target.record.last_warmed_at, Some(NOW));

  registration.last_used_at = None;
  store.put_target("alpha", "line", &registration).await.expect("a restatement");
  assert_eq!(one(&store, "alpha", "line").await.record.last_used_at, Some(NOW - IDLE));

  registration.last_used_at = Some(NOW - MINUTE);
  store.put_target("alpha", "line", &registration).await.expect("a restatement");
  assert_eq!(one(&store, "alpha", "line").await.record.last_used_at, Some(NOW - MINUTE));
}

#[tokio::test]
async fn retiring_a_target_takes_its_record_and_leaves_its_subject_switched() {
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  store.put_target("alpha", "line", &line("session")).await.expect("a target");
  let id = one(&store, "alpha", "line").await.id;
  let switch_on = SubjectChange {
    switch: Some(Some(true)),
    ..SubjectChange::default()
  };
  store.change_subject("alpha", "session", switch_on).await.expect("the switch");
  store
    .record_attempt("alpha", "line", &attempt(NOW, Outcome::Busy))
    .await
    .expect("an attempt");

  assert_eq!(store.delete_target("alpha", "line").await.expect("the target retires"), id);
  assert!(matches!(store.delete_target("alpha", "line").await, Err(StoreError::UnknownTarget)));
  assert!(store.attempts("alpha", "line").await.expect("the log").is_empty());

  let subjects = store.subjects("alpha", all()).await.expect("the subjects").items;
  assert_eq!(subjects.len(), 1);
  assert_eq!((subjects[0].switch, subjects[0].targets), (Some(true), 0));

  // Registered again, it starts with no record and a new id.
  store.put_target("alpha", "line", &line("session")).await.expect("a target");
  let target = one(&store, "alpha", "line").await;
  assert_eq!((target.last_attempt, target.switch), (None, Some(true)));
  assert!(target.id > id);
}

#[tokio::test]
async fn a_use_moves_only_forward_and_lifts_a_pause_only_for_work_begun_after_it() {
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  store.put_target("alpha", "line", &line("session")).await.expect("a target");
  assert!(matches!(
    store.report_use("alpha", "missing", NOW, NOW).await,
    Err(StoreError::UnknownTarget)
  ));

  store.report_use("alpha", "line", NOW, NOW).await.expect("a use");
  store
    .report_use("alpha", "line", NOW - MINUTE, NOW - MINUTE)
    .await
    .expect("an older use");
  assert_eq!(one(&store, "alpha", "line").await.record.last_used_at, Some(NOW));

  let pause = Pause {
    at: NOW + MINUTE,
    until: NOW + 60 * MINUTE,
  };
  let pausing = SubjectChange {
    pause: Some(Some(pause)),
    ..SubjectChange::default()
  };
  let paused = store.change_subject("alpha", "session", pausing).await.expect("a pause");
  assert_eq!(paused.pause, Some(pause));
  let (subject, lifted) = store
    .report_use("alpha", "line", NOW + 2 * MINUTE, NOW)
    .await
    .expect("work begun before");
  assert_eq!((subject.as_str(), lifted), ("session", false));
  let target = one(&store, "alpha", "line").await;
  assert_eq!(target.pause, Some(pause));
  assert_eq!(target.record.last_used_at, Some(NOW + 2 * MINUTE));

  let (_, lifted) = store
    .report_use("alpha", "line", NOW + 3 * MINUTE, NOW + 2 * MINUTE)
    .await
    .expect("work begun after");
  assert!(lifted);
  assert_eq!(one(&store, "alpha", "line").await.pause, None);
}

#[tokio::test]
async fn a_pause_stands_until_its_end() {
  let pause = Pause {
    at: NOW,
    until: NOW + MINUTE,
  };
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  store.put_target("alpha", "line", &line("session")).await.expect("a target");
  let pausing = SubjectChange {
    pause: Some(Some(pause)),
    ..SubjectChange::default()
  };
  store.change_subject("alpha", "session", pausing).await.expect("a pause");
  let target = one(&store, "alpha", "line").await;

  assert_eq!(target.pause_at(NOW + MINUTE - 1), Some(pause));
  assert_eq!(target.pause_at(NOW + MINUTE), None);
  assert!(target.candidate(false, NOW).paused);
  assert!(!target.candidate(false, NOW + MINUTE).paused);
}

#[tokio::test]
async fn a_subject_is_made_before_any_target_and_each_part_changes_alone() {
  let (_dir, store) = store().await;
  let switch = |on| SubjectChange {
    switch: Some(on),
    ..SubjectChange::default()
  };
  assert!(matches!(
    store.change_subject("alpha", "session", switch(Some(true))).await,
    Err(StoreError::UnknownConsumer)
  ));
  assert!(matches!(store.subjects("alpha", all()).await, Err(StoreError::UnknownConsumer)));
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");

  let switched = store.change_subject("alpha", "session", switch(Some(false))).await.expect("the switch");
  assert_eq!((switched.name.as_str(), switched.switch, switched.targets), ("session", Some(false), 0));

  let mut registration = line("session");
  registration.warm_by_default = Some(true);
  store.put_target("alpha", "line", &registration).await.expect("a target");
  assert!(!one(&store, "alpha", "line").await.on(false));

  // A pause leaves the switch where it stands, and lifting it leaves it too.
  let pause = Pause {
    at: NOW,
    until: NOW + MINUTE,
  };
  let paused = store
    .change_subject(
      "alpha",
      "session",
      SubjectChange {
        pause: Some(Some(pause)),
        ..SubjectChange::default()
      },
    )
    .await
    .expect("a pause");
  assert_eq!((paused.switch, paused.pause), (Some(false), Some(pause)));
  let lifted = store
    .change_subject(
      "alpha",
      "session",
      SubjectChange {
        pause: Some(None),
        ..SubjectChange::default()
      },
    )
    .await
    .expect("the pause lifted");
  assert_eq!((lifted.switch, lifted.pause), (Some(false), None));

  let unset = store.change_subject("alpha", "session", switch(None)).await.expect("the switch unset");
  assert_eq!((unset.switch, unset.targets), (None, 1));
  assert!(one(&store, "alpha", "line").await.on(false));
  assert_eq!(store.subject("alpha", "nobody").await.expect("a read"), None);
}

#[tokio::test]
async fn targets_page_by_id_and_narrow() {
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  store.put_consumer("beta", HOOK, NOW).await.expect("a consumer");
  store.put_group("alpha", "large", 1).await.expect("a group");

  store.put_target("beta", "touch", &line("touch")).await.expect("a target");
  store.put_target("alpha", "b", &line("session")).await.expect("a target");
  store.put_target("alpha", "a", &line("other")).await.expect("a target");
  store.put_target("alpha", "seat", &place("holder", "large")).await.expect("a target");

  let names = |targets: Vec<Target>| -> Vec<String> {
    targets
      .into_iter()
      .map(|target| format!("{}/{}", target.consumer, target.name))
      .collect()
  };
  assert_eq!(
    names(store.targets(None, None, all()).await.expect("the list").items),
    ["beta/touch", "alpha/b", "alpha/a", "alpha/seat"]
  );
  assert_eq!(
    names(store.targets(Some("beta"), None, all()).await.expect("the list").items),
    ["beta/touch"]
  );
  assert_eq!(
    names(store.targets(None, Some("session"), all()).await.expect("the list").items),
    ["alpha/b"]
  );
  assert_eq!(store.all_targets().await.expect("every target").len(), 4);
}

#[tokio::test]
async fn an_attempt_moves_only_the_times_its_outcome_moves() {
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");
  store.put_target("alpha", "line", &line("session")).await.expect("a target");

  let failed = Attempt {
    at: NOW,
    outcome: Outcome::Failed,
    code: Some("hook-status".into()),
    reason: Some("The hook answered status 500.".into()),
  };
  store.record_attempt("alpha", "line", &failed).await.expect("an attempt");
  store
    .record_attempt("alpha", "line", &attempt(NOW + 1, Outcome::Skipped))
    .await
    .expect("an attempt");
  store
    .record_attempt("alpha", "line", &attempt(NOW + 2, Outcome::Busy))
    .await
    .expect("an attempt");

  let target = one(&store, "alpha", "line").await;
  assert_eq!(target.record.last_failed_at, Some(NOW));
  assert_eq!(target.record.last_warmed_at, None);
  assert_eq!(target.last_attempt, Some(attempt(NOW + 2, Outcome::Busy)));

  store
    .record_attempt("alpha", "line", &attempt(NOW + 3, Outcome::Warmed))
    .await
    .expect("an attempt");
  let target = one(&store, "alpha", "line").await;
  assert_eq!(target.record.last_warmed_at, Some(NOW + 3));
  assert_eq!(target.record.last_failed_at, Some(NOW));

  let log = store.attempts("alpha", "line").await.expect("the log");
  assert_eq!(log.len(), 4);
  assert_eq!(log[0], failed);

  assert!(
    !store
      .record_attempt("alpha", "gone", &attempt(NOW, Outcome::Warmed))
      .await
      .expect("nothing")
  );
  assert_eq!(store.prune_attempts(NOW + 2).await.expect("the log pruned"), 2);
  assert_eq!(store.attempts("alpha", "line").await.expect("the log").len(), 2);
}

#[tokio::test]
async fn the_event_window_numbers_only_grow_narrows_by_kind_and_prunes_by_time() {
  let (_dir, store) = store().await;
  assert_eq!(store.next_event_id().await.expect("the next number"), 1);
  assert_eq!(store.first_kept_event().await.expect("the first kept"), 1);

  let first = store.append_event("target.changed", NOW, "{}").await.expect("an event");
  let second = store.append_event("round.changed", NOW + 1, "{}").await.expect("an event");
  let third = store.append_event("target.changed", NOW + 2, r#"{"id":1}"#).await.expect("an event");
  assert!(first < second && second < third);

  let narrowed = store.events(0, &["target.changed".to_owned()], 1).await.expect("a page");
  assert_eq!(narrowed.items.iter().map(|event| event.id).collect::<Vec<_>>(), [first]);
  assert!(narrowed.more);
  let rest = store.events(first, &["target.changed".to_owned()], 10).await.expect("a page");
  assert_eq!(
    (rest.items[0].id, rest.items[0].data.as_str(), rest.more),
    (third, r#"{"id":1}"#, false)
  );
  assert_eq!(store.events(0, &[], 10).await.expect("a page").items.len(), 3);

  assert_eq!(store.prune_events(NOW + 3).await.expect("the window pruned"), 3);
  assert_eq!(store.first_kept_event().await.expect("the first kept"), third + 1);
  let next = store.append_event("pass.created", NOW + 4, "{}").await.expect("an event");
  assert_eq!(next, third + 1);
}

#[tokio::test]
async fn a_clock_out_of_its_range_is_refused_by_the_store_itself() {
  let (_dir, store) = store().await;
  store.put_consumer("alpha", HOOK, NOW).await.expect("a consumer");

  let mut registration = line("session");
  registration.clocks.idle_ms = 0;
  assert!(matches!(
    store.put_target("alpha", "line", &registration).await,
    Err(StoreError::Database(_))
  ));
  assert!(store.targets(None, None, all()).await.expect("the list").items.is_empty());
}

/// A connection to a fresh SQLite file with foreign keys on, as the store opens one, with no migration run.
async fn bare(path: &std::path::Path) -> DatabaseConnection {
  let options = SqliteConnectOptions::new().filename(path).create_if_missing(true).foreign_keys(true);
  let pool = SqlitePoolOptions::new()
    .max_connections(1)
    .connect_with(options)
    .await
    .expect("the file opens");

  SqlxSqliteConnector::from_sqlx_sqlite_pool(pool)
}

/// The first value of the first row an SQL text answers, as an integer or a string.
async fn value<T: sea_orm::TryGetable>(db: &DatabaseConnection, sql: &str) -> T {
  let row = db
    .query_one(sea_orm::Statement::from_string(db.get_database_backend(), sql))
    .await
    .expect("the query runs")
    .expect("a row");

  row.try_get_by_index(0).expect("the value reads")
}

const BASELINE_SQL: &str = include_str!("../src/migration/baseline.sql");

/// Every object of the file but the record's, as SQLite keeps it, in one line.
async fn shape(db: &DatabaseConnection) -> String {
  value::<String>(
    db,
    "SELECT group_concat(name || ' ' || COALESCE(sql, ''), ' / ') FROM (SELECT name, sql FROM sqlite_master \
     WHERE name NOT LIKE 'sqlite_%' AND tbl_name <> 'seaql_migrations' ORDER BY type, name)",
  )
  .await
}

async fn recorded(db: &DatabaseConnection) -> String {
  value::<String>(db, r#"SELECT COALESCE(group_concat("version"), '') FROM "seaql_migrations""#).await
}

/// A fresh file holding the baseline's shape and a consumer, recorded as an install from before 1.0.0 that
/// ran the first `through` migrations of its last release.
async fn installed(path: &std::path::Path, through: usize) -> DatabaseConnection {
  let db = bare(path).await;
  db.execute_unprepared(BASELINE_SQL).await.expect("the shape");
  db.execute_unprepared(
    r#"CREATE TABLE "seaql_migrations" ("version" varchar NOT NULL PRIMARY KEY, "applied_at" bigint NOT NULL)"#,
  )
  .await
  .expect("the record");
  for version in &BEFORE[..through] {
    db.execute_unprepared(&format!(
      r#"INSERT INTO "seaql_migrations" ("version", "applied_at") VALUES ('{version}', 1)"#
    ))
    .await
    .expect("a record");
  }
  db.execute_unprepared(
    r#"INSERT INTO "consumers" ("name", "url", "created_at") VALUES ('alpha', 'http://[::1]:1/ping', 1)"#,
  )
  .await
  .expect("a consumer");
  db
}

#[tokio::test]
async fn a_fresh_file_takes_the_baseline_whole_with_the_rounds_row() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let migrated = bare(&dir.path().join("migrated.sqlite")).await;
  up(&migrated).await.expect("the shape comes up");
  let laid = bare(&dir.path().join("laid.sqlite")).await;
  laid.execute_unprepared(BASELINE_SQL).await.expect("the shape");

  assert_eq!(shape(&migrated).await, shape(&laid).await);
  assert_eq!(recorded(&migrated).await, BASELINE);
  assert_eq!(value::<i64>(&migrated, r#"SELECT COUNT(*) FROM "round""#).await, 1);
}

#[tokio::test]
async fn an_install_at_the_last_release_before_the_baseline_only_has_its_record_rewritten() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let db = installed(&dir.path().join("warmer.sqlite"), BEFORE.len()).await;
  let before = shape(&db).await;

  up(&db).await.expect("the store migrates");
  assert_eq!(recorded(&db).await, BASELINE);
  assert_eq!(shape(&db).await, before);
  assert_eq!(value::<String>(&db, r#"SELECT "name" FROM "consumers""#).await, "alpha");

  // A second start finds the baseline recorded and changes nothing.
  up(&db).await.expect("the store migrates again");
  assert_eq!(recorded(&db).await, BASELINE);
  assert_eq!(shape(&db).await, before);
}

#[tokio::test]
async fn an_install_short_of_the_last_release_before_the_baseline_is_refused() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let db = installed(&dir.path().join("warmer.sqlite"), 2).await;

  let refused = up(&db).await.expect_err("a store short of the last release is refused").to_string();
  assert!(refused.contains(BEFORE[1]), "{refused}");
  assert_eq!(recorded(&db).await, BEFORE[..2].join(","));
}
