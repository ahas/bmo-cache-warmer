//! What the warmer keeps between passes and restarts (docs/store.md): consumers, groups, subjects,
//! targets with their records, the attempt log, the event window and the round, in one SQLite file.
//! Every time it writes is handed in by the caller.

use std::path::Path;
use std::time::Duration;

use sea_orm::{
  ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, QueryResult, RuntimeErr,
  SqlxSqliteConnector, Statement, TransactionTrait, Value,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::schedule::{Candidate, Clocks, Group as PlannedGroup, Record, is_on};

/// The store, open and migrated.
#[derive(Clone)]
pub struct Store {
  db: DatabaseConnection,
}

/// Why the store refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
  #[error("There is no consumer by that name.")]
  UnknownConsumer,
  #[error("There is no target by that name under that consumer.")]
  UnknownTarget,
  #[error("There is no group by that name under that consumer.")]
  UnknownGroup,
  #[error("A target still names the group.")]
  GroupInUse,
  #[error("The store could not be read or written: {0}")]
  Database(#[from] DbErr),
}

/// What one attempt answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
  Warmed,
  Busy,
  Skipped,
  Failed,
}

impl Outcome {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Warmed => "warmed",
      Self::Busy => "busy",
      Self::Skipped => "skipped",
      Self::Failed => "failed",
    }
  }

  pub fn parse(word: &str) -> Option<Self> {
    [Self::Warmed, Self::Busy, Self::Skipped, Self::Failed]
      .into_iter()
      .find(|outcome| outcome.as_str() == word)
  }
}

/// One attempt: when it ended, what it answered, and the code and reason it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
  pub at: i64,
  pub outcome: Outcome,
  pub code: Option<String>,
  pub reason: Option<String>,
}

/// A page of rows read by id: the rows and whether more stand past the last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows<T> {
  pub items: Vec<T>,
  pub more: bool,
}

/// Where a page starts and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paging {
  /// The id of the last row read; the page starts past it.
  pub after: i64,
  pub limit: u32,
}

/// A consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consumer {
  pub id: i64,
  pub name: String,
  pub url: String,
  pub created_at: i64,
}

/// A group, with the targets that name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
  pub id: i64,
  pub consumer: String,
  pub name: String,
  pub limit: i64,
  pub target_names: Vec<String>,
}

/// What a registration declares, every value filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
  pub kind: Option<String>,
  pub label: Option<String>,
  pub subject: String,
  pub rank: i64,
  /// Its clocks, a lifetime always declared.
  pub clocks: Clocks,
  /// The stretch the consumer keeps for its own reading of a ping, handed back in each; none where it
  /// declared none.
  pub ping_bound_ms: Option<i64>,
  pub group: Option<String>,
  pub warm_by_default: Option<bool>,
  /// The last use the consumer knows, taken only where later than the record's.
  pub last_used_at: Option<i64>,
}

/// A pause as it is kept: when it was set and when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pause {
  pub at: i64,
  pub until: i64,
}

impl Pause {
  /// Whether the pause still stands at `now`.
  pub fn stands(self, now: i64) -> bool {
    now < self.until
  }
}

/// A registered target with its record and its subject's switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
  pub id: i64,
  pub consumer: String,
  pub name: String,
  pub kind: Option<String>,
  pub label: Option<String>,
  pub subject: String,
  pub rank: i64,
  pub clocks: Clocks,
  pub ping_bound_ms: Option<i64>,
  /// The group it is in, with that group's limit.
  pub group: Option<PlannedGroup>,
  /// As registered; none where the registration said nothing.
  pub warm_by_default: Option<bool>,
  /// Its subject's switch; none while unset.
  pub switch: Option<bool>,
  /// Its subject's pause, as kept; it may have run out.
  pub pause: Option<Pause>,
  pub record: Record,
  pub last_attempt: Option<Attempt>,
}

/// A subject of a consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
  pub id: i64,
  pub consumer: String,
  pub name: String,
  pub switch: Option<bool>,
  /// Its pause, as kept; it may have run out.
  pub pause: Option<Pause>,
  /// How many targets name it.
  pub targets: i64,
}

/// What a change of a subject sets: each field given sets its part, `None` inside unsetting it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubjectChange {
  pub switch: Option<Option<bool>>,
  pub pause: Option<Option<Pause>>,
}

/// The round as it is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Round {
  pub stopped: bool,
  /// The interval set; none while the policy's tick stands.
  pub interval_ms: Option<i64>,
}

/// What a change of the round sets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoundChange {
  pub stopped: Option<bool>,
  pub interval_ms: Option<Option<i64>>,
}

/// One event in the window, its member kept as JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
  pub id: i64,
  pub kind: String,
  pub at: i64,
  pub data: String,
}

impl Target {
  /// Whether it warms, given the policy's default for a registration that says nothing.
  pub fn on(&self, policy_default: bool) -> bool {
    is_on(self.switch, self.warm_by_default, policy_default)
  }

  /// Its subject's pause, where it stands at `now`.
  pub fn pause_at(&self, now: i64) -> Option<Pause> {
    self.pause.filter(|pause| pause.stands(now))
  }

  /// The target as a pass judges it at `now`.
  pub fn candidate(&self, policy_default: bool, now: i64) -> Candidate {
    Candidate {
      consumer: self.consumer.clone(),
      target: self.name.clone(),
      rank: self.rank,
      group: self.group.clone(),
      clocks: self.clocks.clone(),
      record: self.record.clone(),
      on: self.on(policy_default),
      paused: self.pause_at(now).is_some(),
    }
  }
}

fn statement(sql: &str, values: impl IntoIterator<Item = Value>) -> Statement {
  Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

/// The rows of a page read with one row more than its limit, and whether that row came.
fn paged<T>(mut items: Vec<T>, limit: u32) -> Rows<T> {
  let limit = limit as usize;
  let more = items.len() > limit;
  items.truncate(limit);

  Rows { items, more }
}

/// A target's columns, its group's limit and its subject's switches.
const TARGETS: &str = r#"
  SELECT t."id", t."consumer", t."name", t."kind", t."label", t."subject", t."rank", t."idle_ms",
         t."lifetime_ms", t."margin_ms", t."ping_bound_ms", t."group", g."limit" AS "group_limit",
         t."warm_by_default", t."failure_touches", t."honours_idle_override", t."last_used_at",
         t."last_warmed_at", t."last_failed_at", t."attempt_at", t."attempt_outcome", t."attempt_code",
         t."attempt_reason", s."switch", s."paused_at", s."paused_until"
  FROM "targets" t
  JOIN "subjects" s ON s."consumer" = t."consumer" AND s."name" = t."subject"
  LEFT JOIN "groups" g ON g."consumer" = t."consumer" AND g."name" = t."group"
"#;

/// A subject's columns and how many targets name it.
const SUBJECTS: &str = r#"
  SELECT s."id", s."consumer", s."name", s."switch", s."paused_at", s."paused_until",
         (SELECT COUNT(*) FROM "targets" t WHERE t."consumer" = s."consumer" AND t."subject" = s."name") AS "targets"
  FROM "subjects" s
"#;

fn pause_of(row: &QueryResult) -> Result<Option<Pause>, DbErr> {
  Ok(
    match (
      row.try_get::<Option<i64>>("", "paused_at")?,
      row.try_get::<Option<i64>>("", "paused_until")?,
    ) {
      (Some(at), Some(until)) => Some(Pause { at, until }),
      _ => None,
    },
  )
}

fn outcome_of(word: &str) -> Result<Outcome, DbErr> {
  Outcome::parse(word).ok_or_else(|| DbErr::Type(format!("an attempt's outcome reads {word}")))
}

fn target_of(row: &QueryResult) -> Result<Target, DbErr> {
  let group = match row.try_get::<Option<String>>("", "group")? {
    Some(name) => Some(PlannedGroup {
      name,
      limit: usize::try_from(row.try_get::<i64>("", "group_limit")?).unwrap_or(0),
    }),
    None => None,
  };
  let last_attempt = match (
    row.try_get::<Option<i64>>("", "attempt_at")?,
    row.try_get::<Option<String>>("", "attempt_outcome")?,
  ) {
    (Some(at), Some(outcome)) => Some(Attempt {
      at,
      outcome: outcome_of(&outcome)?,
      code: row.try_get("", "attempt_code")?,
      reason: row.try_get("", "attempt_reason")?,
    }),
    _ => None,
  };

  Ok(Target {
    id: row.try_get("", "id")?,
    consumer: row.try_get("", "consumer")?,
    name: row.try_get("", "name")?,
    kind: row.try_get("", "kind")?,
    label: row.try_get("", "label")?,
    subject: row.try_get("", "subject")?,
    rank: row.try_get("", "rank")?,
    clocks: Clocks {
      idle_ms: row.try_get("", "idle_ms")?,
      lifetime_ms: Some(row.try_get("", "lifetime_ms")?),
      margin_ms: row.try_get("", "margin_ms")?,
      failure_touches: row.try_get("", "failure_touches")?,
      honours_idle_override: row.try_get("", "honours_idle_override")?,
    },
    ping_bound_ms: row.try_get("", "ping_bound_ms")?,
    group,
    warm_by_default: row.try_get("", "warm_by_default")?,
    switch: row.try_get("", "switch")?,
    pause: pause_of(row)?,
    record: Record {
      last_used_at: row.try_get("", "last_used_at")?,
      last_warmed_at: row.try_get("", "last_warmed_at")?,
      last_failed_at: row.try_get("", "last_failed_at")?,
    },
    last_attempt,
  })
}

fn subject_of(row: &QueryResult) -> Result<Subject, DbErr> {
  Ok(Subject {
    id: row.try_get("", "id")?,
    consumer: row.try_get("", "consumer")?,
    name: row.try_get("", "name")?,
    switch: row.try_get("", "switch")?,
    pause: pause_of(row)?,
    targets: row.try_get("", "targets")?,
  })
}

fn consumer_of(row: &QueryResult) -> Result<Consumer, DbErr> {
  Ok(Consumer {
    id: row.try_get("", "id")?,
    name: row.try_get("", "name")?,
    url: row.try_get("", "url")?,
    created_at: row.try_get("", "created_at")?,
  })
}

fn event_of(row: &QueryResult) -> Result<StoredEvent, DbErr> {
  Ok(StoredEvent {
    id: row.try_get("", "id")?,
    kind: row.try_get("", "kind")?,
    at: row.try_get("", "at")?,
    data: row.try_get("", "data")?,
  })
}

async fn exists(db: &impl ConnectionTrait, sql: &str, values: Vec<Value>) -> Result<bool, DbErr> {
  Ok(db.query_one(statement(sql, values)).await?.is_some())
}

async fn consumer_exists(db: &impl ConnectionTrait, consumer: &str) -> Result<bool, DbErr> {
  exists(db, r#"SELECT 1 FROM "consumers" WHERE "name" = ?1"#, vec![consumer.into()]).await
}

async fn group_exists(db: &impl ConnectionTrait, consumer: &str, group: &str) -> Result<bool, DbErr> {
  let key = vec![consumer.into(), group.into()];

  exists(db, r#"SELECT 1 FROM "groups" WHERE "consumer" = ?1 AND "name" = ?2"#, key).await
}

/// Makes a subject where none stands yet.
async fn ensure_subject(db: &impl ConnectionTrait, consumer: &str, subject: &str) -> Result<(), DbErr> {
  db.execute(statement(
    r#"INSERT INTO "subjects" ("consumer", "name") VALUES (?1, ?2) ON CONFLICT DO NOTHING"#,
    [consumer.into(), subject.into()],
  ))
  .await
  .map(drop)
}

async fn one_subject(db: &impl ConnectionTrait, consumer: &str, subject: &str) -> Result<Subject, DbErr> {
  let sql = format!(r#"{SUBJECTS} WHERE s."consumer" = ?1 AND s."name" = ?2"#);
  let row = db
    .query_one(statement(&sql, [consumer.into(), subject.into()]))
    .await?
    .ok_or_else(|| DbErr::RecordNotFound(format!("the subject {subject}")))?;

  subject_of(&row)
}

async fn one_group(db: &impl ConnectionTrait, consumer: &str, group: &str) -> Result<Group, DbErr> {
  let key = || [consumer.into(), group.into()];
  let row = db
    .query_one(statement(
      r#"SELECT "id", "limit" FROM "groups" WHERE "consumer" = ?1 AND "name" = ?2"#,
      key(),
    ))
    .await?
    .ok_or_else(|| DbErr::RecordNotFound(format!("the group {group}")))?;
  let names = db
    .query_all(statement(
      r#"SELECT "name" FROM "targets" WHERE "consumer" = ?1 AND "group" = ?2 ORDER BY "name""#,
      key(),
    ))
    .await?;

  Ok(Group {
    id: row.try_get("", "id")?,
    consumer: consumer.to_owned(),
    name: group.to_owned(),
    limit: row.try_get("", "limit")?,
    target_names: names.iter().map(|row| row.try_get("", "name")).collect::<Result<_, _>>()?,
  })
}

impl Store {
  /// The store in the SQLite file at `path`, created where missing and migrated to the latest shape.
  pub async fn open(path: &Path) -> Result<Self, StoreError> {
    // A statement waits for another connection's lock, and a call for a free connection, until the holder
    // gives it back: the lock's wait is the longest SQLite takes and the connection's a span no service
    // lives through, so no clock ends store work.
    let options = SqliteConnectOptions::new()
      .filename(path)
      .create_if_missing(true)
      .foreign_keys(true)
      .busy_timeout(Duration::from_millis(i32::MAX.unsigned_abs().into()));
    let pool = SqlitePoolOptions::new()
      .max_connections(4)
      .acquire_timeout(Duration::from_secs(u32::MAX.into()))
      .connect_with(options)
      .await
      .map_err(|error| DbErr::Conn(RuntimeErr::SqlxError(error)))?;
    let db = SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);

    crate::migration::up(&db).await?;
    Ok(Self { db })
  }

  /// The connection under the store, for the migrations' own tests.
  pub fn connection(&self) -> &DatabaseConnection {
    &self.db
  }

  async fn transaction(&self) -> Result<DatabaseTransaction, StoreError> {
    Ok(self.db.begin().await?)
  }

  /// Registers a consumer at `now` or moves its hook; the consumer, and true when it is new.
  pub async fn put_consumer(
    &self,
    name: &str,
    url: &str,
    now: i64,
  ) -> Result<(Consumer, bool), StoreError> {
    let txn = self.transaction().await?;
    let new = !consumer_exists(&txn, name).await?;

    txn
      .execute(statement(
        r#"INSERT INTO "consumers" ("name", "url", "created_at") VALUES (?1, ?2, ?3)
           ON CONFLICT ("name") DO UPDATE SET "url" = excluded."url""#,
        [name.into(), url.into(), now.into()],
      ))
      .await?;
    let row = txn
      .query_one(statement(r#"SELECT * FROM "consumers" WHERE "name" = ?1"#, [name.into()]))
      .await?
      .ok_or(StoreError::UnknownConsumer)?;
    let consumer = consumer_of(&row)?;
    txn.commit().await?;
    Ok((consumer, new))
  }

  /// A page of consumers by id.
  pub async fn consumers(&self, paging: Paging) -> Result<Rows<Consumer>, StoreError> {
    let rows = self
      .db
      .query_all(statement(
        r#"SELECT * FROM "consumers" WHERE "id" > ?1 ORDER BY "id" LIMIT ?2"#,
        [paging.after.into(), (i64::from(paging.limit) + 1).into()],
      ))
      .await?;
    let items = rows.iter().map(consumer_of).collect::<Result<_, _>>()?;

    Ok(paged(items, paging.limit))
  }

  /// A consumer's hook URL; none when there is no such consumer.
  pub async fn hook_url(&self, consumer: &str) -> Result<Option<String>, StoreError> {
    let row = self
      .db
      .query_one(statement(r#"SELECT "url" FROM "consumers" WHERE "name" = ?1"#, [consumer.into()]))
      .await?;

    Ok(row.map(|row| row.try_get("", "url")).transpose()?)
  }

  /// Declares a group or changes its limit; the group, and true when it is new.
  pub async fn put_group(
    &self,
    consumer: &str,
    group: &str,
    limit: i64,
  ) -> Result<(Group, bool), StoreError> {
    let txn = self.transaction().await?;
    if !consumer_exists(&txn, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    let new = !group_exists(&txn, consumer, group).await?;

    txn
      .execute(statement(
        r#"INSERT INTO "groups" ("consumer", "name", "limit") VALUES (?1, ?2, ?3)
           ON CONFLICT ("consumer", "name") DO UPDATE SET "limit" = excluded."limit""#,
        [consumer.into(), group.into(), limit.into()],
      ))
      .await?;
    let answer = one_group(&txn, consumer, group).await?;
    txn.commit().await?;
    Ok((answer, new))
  }

  /// Retires a group no target names.
  pub async fn delete_group(&self, consumer: &str, group: &str) -> Result<(), StoreError> {
    let txn = self.transaction().await?;
    let key = || vec![consumer.into(), group.into()];

    if !group_exists(&txn, consumer, group).await? {
      return Err(StoreError::UnknownGroup);
    }
    if exists(&txn, r#"SELECT 1 FROM "targets" WHERE "consumer" = ?1 AND "group" = ?2"#, key()).await? {
      return Err(StoreError::GroupInUse);
    }
    txn
      .execute(statement(r#"DELETE FROM "groups" WHERE "consumer" = ?1 AND "name" = ?2"#, key()))
      .await?;
    txn.commit().await?;
    Ok(())
  }

  /// Registers a target or restates one, keeping its record and its id; true when it is new. Its subject is
  /// made where none stands.
  pub async fn put_target(
    &self,
    consumer: &str,
    target: &str,
    registration: &Registration,
  ) -> Result<bool, StoreError> {
    let txn = self.transaction().await?;
    if !consumer_exists(&txn, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    if let Some(group) = &registration.group
      && !group_exists(&txn, consumer, group).await?
    {
      return Err(StoreError::UnknownGroup);
    }
    let new = !exists(
      &txn,
      r#"SELECT 1 FROM "targets" WHERE "consumer" = ?1 AND "name" = ?2"#,
      vec![consumer.into(), target.into()],
    )
    .await?;
    ensure_subject(&txn, consumer, &registration.subject).await?;

    let clocks = &registration.clocks;
    txn
      .execute(statement(
        r#"INSERT INTO "targets" ("consumer", "name", "kind", "label", "subject", "rank", "idle_ms",
             "lifetime_ms", "margin_ms", "ping_bound_ms", "group", "warm_by_default", "failure_touches",
             "honours_idle_override", "last_used_at")
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
           ON CONFLICT ("consumer", "name") DO UPDATE SET
             "kind" = excluded."kind", "label" = excluded."label", "subject" = excluded."subject",
             "rank" = excluded."rank", "idle_ms" = excluded."idle_ms", "lifetime_ms" = excluded."lifetime_ms",
             "margin_ms" = excluded."margin_ms", "ping_bound_ms" = excluded."ping_bound_ms",
             "group" = excluded."group", "warm_by_default" = excluded."warm_by_default",
             "failure_touches" = excluded."failure_touches",
             "honours_idle_override" = excluded."honours_idle_override",
             "last_used_at" = CASE
               WHEN "targets"."last_used_at" IS NULL OR excluded."last_used_at" > "targets"."last_used_at"
                 THEN COALESCE(excluded."last_used_at", "targets"."last_used_at")
               ELSE "targets"."last_used_at" END"#,
        [
          consumer.into(),
          target.into(),
          registration.kind.clone().into(),
          registration.label.clone().into(),
          registration.subject.as_str().into(),
          registration.rank.into(),
          clocks.idle_ms.into(),
          clocks.lifetime_ms.unwrap_or(0).into(),
          clocks.margin_ms.into(),
          registration.ping_bound_ms.into(),
          registration.group.clone().into(),
          registration.warm_by_default.into(),
          clocks.failure_touches.into(),
          clocks.honours_idle_override.into(),
          registration.last_used_at.into(),
        ],
      ))
      .await?;
    txn.commit().await?;
    Ok(new)
  }

  /// Retires a target with its record and its attempts; its id, and its subject stays.
  pub async fn delete_target(&self, consumer: &str, target: &str) -> Result<i64, StoreError> {
    let row = self
      .db
      .query_one(statement(
        r#"DELETE FROM "targets" WHERE "consumer" = ?1 AND "name" = ?2 RETURNING "id""#,
        [consumer.into(), target.into()],
      ))
      .await?
      .ok_or(StoreError::UnknownTarget)?;

    Ok(row.try_get("", "id")?)
  }

  /// Records a use: the last use moves to `at` where that is later, and a pause on the subject set before
  /// `started_at` is lifted. The target's subject, and whether its pause was lifted.
  pub async fn report_use(
    &self,
    consumer: &str,
    target: &str,
    at: i64,
    started_at: i64,
  ) -> Result<(String, bool), StoreError> {
    let txn = self.transaction().await?;
    let row = txn
      .query_one(statement(
        r#"SELECT "subject" FROM "targets" WHERE "consumer" = ?1 AND "name" = ?2"#,
        [consumer.into(), target.into()],
      ))
      .await?
      .ok_or(StoreError::UnknownTarget)?;
    let subject: String = row.try_get("", "subject")?;

    txn
      .execute(statement(
        r#"UPDATE "targets" SET "last_used_at" = ?3
           WHERE "consumer" = ?1 AND "name" = ?2 AND ("last_used_at" IS NULL OR "last_used_at" < ?3)"#,
        [consumer.into(), target.into(), at.into()],
      ))
      .await?;
    let lifted = txn
      .execute(statement(
        r#"UPDATE "subjects" SET "paused_at" = NULL, "paused_until" = NULL
           WHERE "consumer" = ?1 AND "name" = ?2 AND "paused_at" < ?3"#,
        [consumer.into(), subject.as_str().into(), started_at.into()],
      ))
      .await?
      .rows_affected()
      > 0;
    txn.commit().await?;
    Ok((subject, lifted))
  }

  /// Every target, in no order; the pass orders them by its plan.
  pub async fn all_targets(&self) -> Result<Vec<Target>, StoreError> {
    let rows = self.db.query_all(statement(TARGETS, [])).await?;

    Ok(rows.iter().map(target_of).collect::<Result<_, _>>()?)
  }

  /// A page of targets by id; `consumer` and `subject` narrow it.
  pub async fn targets(
    &self,
    consumer: Option<&str>,
    subject: Option<&str>,
    paging: Paging,
  ) -> Result<Rows<Target>, StoreError> {
    let sql = format!(
      r#"{TARGETS} WHERE (?1 IS NULL OR t."consumer" = ?1) AND (?2 IS NULL OR t."subject" = ?2) AND t."id" > ?3
         ORDER BY t."id" LIMIT ?4"#
    );
    let rows = self
      .db
      .query_all(statement(
        &sql,
        [
          consumer.map(str::to_owned).into(),
          subject.map(str::to_owned).into(),
          paging.after.into(),
          (i64::from(paging.limit) + 1).into(),
        ],
      ))
      .await?;
    let items = rows.iter().map(target_of).collect::<Result<_, _>>()?;

    Ok(paged(items, paging.limit))
  }

  /// One target; none when there is no such target.
  pub async fn target(&self, consumer: &str, target: &str) -> Result<Option<Target>, StoreError> {
    let sql = format!(r#"{TARGETS} WHERE t."consumer" = ?1 AND t."name" = ?2"#);
    let row = self.db.query_one(statement(&sql, [consumer.into(), target.into()])).await?;

    Ok(row.as_ref().map(target_of).transpose()?)
  }

  /// A page of a consumer's subjects by id.
  pub async fn subjects(&self, consumer: &str, paging: Paging) -> Result<Rows<Subject>, StoreError> {
    if !consumer_exists(&self.db, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    let sql = format!(r#"{SUBJECTS} WHERE s."consumer" = ?1 AND s."id" > ?2 ORDER BY s."id" LIMIT ?3"#);
    let rows = self
      .db
      .query_all(statement(
        &sql,
        [consumer.into(), paging.after.into(), (i64::from(paging.limit) + 1).into()],
      ))
      .await?;
    let items = rows.iter().map(subject_of).collect::<Result<_, _>>()?;

    Ok(paged(items, paging.limit))
  }

  /// One subject; none when there is no such subject.
  pub async fn subject(&self, consumer: &str, subject: &str) -> Result<Option<Subject>, StoreError> {
    let sql = format!(r#"{SUBJECTS} WHERE s."consumer" = ?1 AND s."name" = ?2"#);
    let row = self.db.query_one(statement(&sql, [consumer.into(), subject.into()])).await?;

    Ok(row.as_ref().map(subject_of).transpose()?)
  }

  /// Sets a subject's switch or pause as `change` says; the subject is made where none stands.
  pub async fn change_subject(
    &self,
    consumer: &str,
    subject: &str,
    change: SubjectChange,
  ) -> Result<Subject, StoreError> {
    let txn = self.transaction().await?;
    if !consumer_exists(&txn, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    ensure_subject(&txn, consumer, subject).await?;
    if let Some(switch) = change.switch {
      txn
        .execute(statement(
          r#"UPDATE "subjects" SET "switch" = ?3 WHERE "consumer" = ?1 AND "name" = ?2"#,
          [consumer.into(), subject.into(), switch.into()],
        ))
        .await?;
    }
    if let Some(pause) = change.pause {
      txn
        .execute(statement(
          r#"UPDATE "subjects" SET "paused_at" = ?3, "paused_until" = ?4 WHERE "consumer" = ?1 AND "name" = ?2"#,
          [
            consumer.into(),
            subject.into(),
            pause.map(|pause| pause.at).into(),
            pause.map(|pause| pause.until).into(),
          ],
        ))
        .await?;
    }
    let answer = one_subject(&txn, consumer, subject).await?;
    txn.commit().await?;
    Ok(answer)
  }

  /// The round as it is kept.
  pub async fn round(&self) -> Result<Round, StoreError> {
    let row = self
      .db
      .query_one(statement(r#"SELECT "stopped", "interval_ms" FROM "round" WHERE "only" = 1"#, []))
      .await?
      .ok_or_else(|| DbErr::RecordNotFound("the round".into()))?;

    Ok(Round {
      stopped: row.try_get("", "stopped")?,
      interval_ms: row.try_get("", "interval_ms")?,
    })
  }

  /// Whether the round is stopped.
  pub async fn round_stopped(&self) -> Result<bool, StoreError> {
    Ok(self.round().await?.stopped)
  }

  /// Changes the round as `change` says; the round as it then stands.
  pub async fn change_round(&self, change: RoundChange) -> Result<Round, StoreError> {
    let txn = self.transaction().await?;
    if let Some(stopped) = change.stopped {
      txn
        .execute(statement(r#"UPDATE "round" SET "stopped" = ?1 WHERE "only" = 1"#, [stopped.into()]))
        .await?;
    }
    if let Some(interval_ms) = change.interval_ms {
      txn
        .execute(statement(
          r#"UPDATE "round" SET "interval_ms" = ?1 WHERE "only" = 1"#,
          [interval_ms.into()],
        ))
        .await?;
    }
    txn.commit().await?;
    self.round().await
  }

  /// Records an attempt on a target: its last attempt, the log, and the last warmed or failed time its
  /// outcome moves. A target retired while it was pinged records nothing; false then.
  pub async fn record_attempt(
    &self,
    consumer: &str,
    target: &str,
    attempt: &Attempt,
  ) -> Result<bool, StoreError> {
    let txn = self.transaction().await?;
    let warmed = (attempt.outcome == Outcome::Warmed).then_some(attempt.at);
    let failed = (attempt.outcome == Outcome::Failed).then_some(attempt.at);
    let updated = txn
      .execute(statement(
        r#"UPDATE "targets" SET "attempt_at" = ?3, "attempt_outcome" = ?4, "attempt_code" = ?5,
             "attempt_reason" = ?6, "last_warmed_at" = COALESCE(?7, "last_warmed_at"),
             "last_failed_at" = COALESCE(?8, "last_failed_at")
           WHERE "consumer" = ?1 AND "name" = ?2"#,
        [
          consumer.into(),
          target.into(),
          attempt.at.into(),
          attempt.outcome.as_str().into(),
          attempt.code.clone().into(),
          attempt.reason.clone().into(),
          warmed.into(),
          failed.into(),
        ],
      ))
      .await?;
    if updated.rows_affected() == 0 {
      return Ok(false);
    }
    txn
      .execute(statement(
        r#"INSERT INTO "attempts" ("consumer", "target", "at", "outcome", "code", "reason")
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
        [
          consumer.into(),
          target.into(),
          attempt.at.into(),
          attempt.outcome.as_str().into(),
          attempt.code.clone().into(),
          attempt.reason.clone().into(),
        ],
      ))
      .await?;
    txn.commit().await?;
    Ok(true)
  }

  /// A target's attempts in the log, oldest first.
  pub async fn attempts(&self, consumer: &str, target: &str) -> Result<Vec<Attempt>, StoreError> {
    let rows = self
      .db
      .query_all(statement(
        r#"SELECT "at", "outcome", "code", "reason" FROM "attempts"
           WHERE "consumer" = ?1 AND "target" = ?2 ORDER BY "id""#,
        [consumer.into(), target.into()],
      ))
      .await?;

    let attempt = |row: &QueryResult| -> Result<Attempt, DbErr> {
      let outcome: String = row.try_get("", "outcome")?;
      Ok(Attempt {
        at: row.try_get("", "at")?,
        outcome: outcome_of(&outcome)?,
        code: row.try_get("", "code")?,
        reason: row.try_get("", "reason")?,
      })
    };
    Ok(rows.iter().map(attempt).collect::<Result<_, _>>()?)
  }

  /// Removes the log's attempts from before `before`; how many went.
  pub async fn prune_attempts(&self, before: i64) -> Result<u64, StoreError> {
    let gone = self
      .db
      .execute(statement(r#"DELETE FROM "attempts" WHERE "at" < ?1"#, [before.into()]))
      .await?;

    Ok(gone.rows_affected())
  }

  /// Keeps an event in the window; its number.
  pub async fn append_event(&self, kind: &str, at: i64, data: &str) -> Result<i64, StoreError> {
    let row = self
      .db
      .query_one(statement(
        r#"INSERT INTO "events" ("kind", "at", "data") VALUES (?1, ?2, ?3) RETURNING "id""#,
        [kind.into(), at.into(), data.into()],
      ))
      .await?
      .ok_or_else(|| DbErr::RecordNotFound("the event just kept".into()))?;

    Ok(row.try_get("", "id")?)
  }

  /// The number the next event will take: one past the highest ever given, whether or not it is still
  /// kept.
  pub async fn next_event_id(&self) -> Result<i64, StoreError> {
    let row = self
      .db
      .query_one(statement(
        r#"SELECT COALESCE((SELECT "seq" FROM "sqlite_sequence" WHERE "name" = 'events'), 0) + 1 AS "next""#,
        [],
      ))
      .await?
      .ok_or_else(|| DbErr::RecordNotFound("the event sequence".into()))?;

    Ok(row.try_get("", "next")?)
  }

  /// The first event still kept; the next number where none is.
  pub async fn first_kept_event(&self) -> Result<i64, StoreError> {
    let row = self.db.query_one(statement(r#"SELECT MIN("id") AS "first" FROM "events""#, [])).await?;

    match row.map(|row| row.try_get::<Option<i64>>("", "first")).transpose()?.flatten() {
      Some(first) => Ok(first),
      None => self.next_event_id().await,
    }
  }

  /// A page of the events past `after`, of the kinds named where any are.
  pub async fn events(
    &self,
    after: i64,
    kinds: &[String],
    limit: u32,
  ) -> Result<Rows<StoredEvent>, StoreError> {
    // The kinds go in as one comma-joined list, none of them holding a comma, and a kind matches where it
    // stands between two of its commas.
    let rows = self
      .db
      .query_all(statement(
        r#"SELECT "id", "kind", "at", "data" FROM "events"
           WHERE "id" > ?1 AND (?2 = '' OR instr(',' || ?2 || ',', ',' || "kind" || ',') > 0)
           ORDER BY "id" LIMIT ?3"#,
        [after.into(), kinds.join(",").into(), (i64::from(limit) + 1).into()],
      ))
      .await?;
    let items = rows.iter().map(event_of).collect::<Result<_, _>>()?;

    Ok(paged(items, limit))
  }

  /// Removes the window's events from before `before`; how many went.
  pub async fn prune_events(&self, before: i64) -> Result<u64, StoreError> {
    let gone = self
      .db
      .execute(statement(r#"DELETE FROM "events" WHERE "at" < ?1"#, [before.into()]))
      .await?;

    Ok(gone.rows_affected())
  }
}
