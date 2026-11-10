//! The store (docs/store.md): what each write keeps and refuses, its pages and its event window, and how
//! its shape moves, rows carried across the migration that gives every member an id and the one that lets a
//! target declare no ping stretch.

use crate::fixtures;

use bmo_cache_warmer::migration::{Migrator, MigratorTrait};
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

#[tokio::test]
async fn the_id_migration_carries_every_row_it_can_and_goes_back() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let db = bare(&dir.path().join("warmer.sqlite")).await;
  Migrator::up(&db, Some(1)).await.expect("the first shape");
  db.execute_unprepared(
    r#"
    INSERT INTO "consumers" ("name", "hook_url") VALUES ('alpha', 'http://[::1]:1/ping');
    INSERT INTO "groups" ("consumer", "name", "limit") VALUES ('alpha', 'large', 2);
    INSERT INTO "subjects" ("consumer", "name", "switch", "paused_at") VALUES ('alpha', 'session', 1, 5);
    INSERT INTO "targets" ("consumer", "name", "kind", "label", "subject", "rank", "idle_ms", "lifetime_ms",
      "margin_ms", "ping_bound_ms", "group", "warm_by_default", "failure_touches", "honours_idle_override",
      "attempt_at", "attempt_outcome", "attempt_code")
    VALUES
      ('alpha', 'kept', 'chat_touch', 'a touch', 'session', 90, 2400000, 3600000, 0, 1800000, 'large', 1, 0, 1,
       7, 'failed', 'hook_timeout'),
      ('alpha', 'endless', 'worker_session', 'a session', 'session', 80, 2400000, NULL, 0, 90000, NULL, 1, 1, 1,
       NULL, NULL, NULL);
    INSERT INTO "attempts" ("consumer", "target", "at", "outcome", "code") VALUES
      ('alpha', 'kept', 7, 'failed', 'hook_timeout'),
      ('alpha', 'endless', 8, 'warmed', NULL);
    "#,
  )
  .await
  .expect("old-shape rows");

  Migrator::up(&db, Some(1)).await.expect("the shape comes up");
  assert_eq!(
    value::<String>(&db, r#"SELECT "url" FROM "consumers" WHERE "name" = 'alpha'"#).await,
    "http://[::1]:1/ping"
  );
  assert!(value::<i64>(&db, r#"SELECT "id" FROM "consumers" WHERE "name" = 'alpha'"#).await > 0);
  assert_eq!(value::<i64>(&db, r#"SELECT "paused_until" FROM "subjects""#).await, i64::MAX);
  assert_eq!(value::<i64>(&db, r#"SELECT COUNT(*) FROM "targets""#).await, 1);
  assert_eq!(value::<String>(&db, r#"SELECT "kind" FROM "targets""#).await, "chat-touch");
  assert_eq!(value::<String>(&db, r#"SELECT "attempt_code" FROM "targets""#).await, "hook-timeout");
  assert_eq!(
    value::<String>(&db, r#"SELECT group_concat("target" || ':' || "code") FROM "attempts""#).await,
    "kept:hook-timeout"
  );
  assert_eq!(value::<Option<i64>>(&db, r#"SELECT "interval_ms" FROM "round""#).await, None);
  assert_eq!(value::<i64>(&db, r#"SELECT COUNT(*) FROM "events""#).await, 0);
  assert_eq!(value::<i64>(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check").await, 0);

  Migrator::down(&db, Some(1)).await.expect("one step back");
  assert_eq!(
    value::<String>(&db, r#"SELECT "hook_url" FROM "consumers""#).await,
    "http://[::1]:1/ping"
  );
  assert_eq!(
    value::<String>(&db, r#"SELECT "kind" || '/' || "attempt_code" FROM "targets""#).await,
    "chat_touch/hook_timeout"
  );
  assert_eq!(value::<i64>(&db, r#"SELECT "paused_at" FROM "subjects""#).await, 5);
  assert_eq!(value::<i64>(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check").await, 0);
}

#[tokio::test]
async fn the_stretch_migration_carries_each_stretch_takes_none_and_goes_back() {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let db = bare(&dir.path().join("warmer.sqlite")).await;
  Migrator::up(&db, Some(2)).await.expect("the shape before the stretch");
  db.execute_unprepared(
    r#"
    INSERT INTO "consumers" ("name", "url", "created_at") VALUES ('alpha', 'http://[::1]:1/ping', 1);
    INSERT INTO "subjects" ("consumer", "name") VALUES ('alpha', 'session');
    INSERT INTO "targets" ("consumer", "name", "subject", "rank", "idle_ms", "lifetime_ms", "margin_ms",
      "ping_bound_ms", "failure_touches", "honours_idle_override")
    VALUES ('alpha', 'kept', 'session', 0, 2400000, 3600000, 0, 1800000, 0, 1);
    "#,
  )
  .await
  .expect("rows of the shape before");

  Migrator::up(&db, None).await.expect("the shape comes up");
  assert_eq!(value::<i64>(&db, r#"SELECT "ping_bound_ms" FROM "targets""#).await, 1_800_000);
  db.execute_unprepared(
    r#"INSERT INTO "targets" ("consumer", "name", "subject", "rank", "idle_ms", "lifetime_ms", "margin_ms",
         "failure_touches", "honours_idle_override")
       VALUES ('alpha', 'bare', 'session', 0, 2400000, 3600000, 0, 0, 1)"#,
  )
  .await
  .expect("a target with no stretch");
  assert!(
    db.execute_unprepared(r#"UPDATE "targets" SET "ping_bound_ms" = 0 WHERE "name" = 'kept'"#)
      .await
      .is_err()
  );

  Migrator::down(&db, Some(1)).await.expect("one step back");
  assert_eq!(
    value::<String>(&db, r#"SELECT group_concat("name" || ':' || "ping_bound_ms") FROM "targets""#).await,
    "kept:1800000,bare:120000"
  );
  assert_eq!(value::<i64>(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check").await, 0);
}
