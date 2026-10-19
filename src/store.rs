//! What the warmer keeps between passes and restarts (docs/store.md): consumers, groups, subjects,
//! targets with their records, the attempt log and the round, in one SQLite file. Every time it writes
//! is handed in by the caller.

use std::path::Path;

use cache_warmer::{Candidate, Clocks, Group, Record};
use sea_orm::{
  ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, QueryResult, RuntimeErr,
  SqlxSqliteConnector, Statement, TransactionTrait, Value,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::migration::{Migrator, MigratorTrait};

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

/// What a registration declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
  pub kind: String,
  pub label: String,
  pub subject: String,
  pub rank: i64,
  pub clocks: Clocks,
  pub ping_bound_ms: i64,
  pub group: Option<String>,
  pub warm_by_default: Option<bool>,
  /// The last use the consumer knows, taken only where later than the record's.
  pub last_used_at: Option<i64>,
}

/// A registered target with its record and its subject's switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
  pub consumer: String,
  pub name: String,
  pub kind: String,
  pub label: String,
  pub subject: String,
  pub rank: i64,
  pub clocks: Clocks,
  pub ping_bound_ms: i64,
  /// The group it is in, with that group's limit.
  pub group: Option<Group>,
  /// As registered; none where the registration said nothing.
  pub warm_by_default: Option<bool>,
  /// Its subject's switch; none while unset.
  pub switch: Option<bool>,
  /// When its subject was paused, while that stands.
  pub paused_at: Option<i64>,
  pub record: Record,
  pub last_attempt: Option<Attempt>,
}

/// A subject of a consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
  pub subject: String,
  pub switch: Option<bool>,
  pub paused_at: Option<i64>,
  /// How many targets name it.
  pub targets: i64,
}

impl Target {
  /// Whether it warms, given the policy's default for a registration that says nothing.
  pub fn on(&self, policy_default: bool) -> bool {
    cache_warmer::is_on(self.switch, self.warm_by_default, policy_default)
  }

  /// The target as a pass judges it.
  pub fn candidate(&self, policy_default: bool) -> Candidate {
    Candidate {
      consumer: self.consumer.clone(),
      target: self.name.clone(),
      rank: self.rank,
      group: self.group.clone(),
      clocks: self.clocks.clone(),
      record: self.record.clone(),
      on: self.on(policy_default),
      paused: self.paused_at.is_some(),
    }
  }
}

fn statement(sql: &str, values: impl IntoIterator<Item = Value>) -> Statement {
  Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
}

/// A target's columns, its group's limit and its subject's switches, in the order the pass walks them.
const TARGETS: &str = r#"
  SELECT t."consumer", t."name", t."kind", t."label", t."subject", t."rank", t."idle_ms", t."lifetime_ms",
         t."margin_ms", t."ping_bound_ms", t."group", g."limit" AS "group_limit", t."warm_by_default",
         t."failure_touches", t."honours_idle_override", t."last_used_at", t."last_warmed_at",
         t."last_failed_at", t."attempt_at", t."attempt_outcome", t."attempt_code", t."attempt_reason",
         s."switch", s."paused_at"
  FROM "targets" t
  JOIN "subjects" s ON s."consumer" = t."consumer" AND s."name" = t."subject"
  LEFT JOIN "groups" g ON g."consumer" = t."consumer" AND g."name" = t."group"
"#;

const ORDER: &str = r#" ORDER BY t."rank", t."consumer", t."name""#;

fn target_of(row: &QueryResult) -> Result<Target, DbErr> {
  let group = match row.try_get::<Option<String>>("", "group")? {
    Some(name) => Some(Group {
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
      outcome: Outcome::parse(&outcome)
        .ok_or_else(|| DbErr::Type(format!("an attempt's outcome reads {outcome}")))?,
      code: row.try_get("", "attempt_code")?,
      reason: row.try_get("", "attempt_reason")?,
    }),
    _ => None,
  };

  Ok(Target {
    consumer: row.try_get("", "consumer")?,
    name: row.try_get("", "name")?,
    kind: row.try_get("", "kind")?,
    label: row.try_get("", "label")?,
    subject: row.try_get("", "subject")?,
    rank: row.try_get("", "rank")?,
    clocks: Clocks {
      idle_ms: row.try_get("", "idle_ms")?,
      lifetime_ms: row.try_get("", "lifetime_ms")?,
      margin_ms: row.try_get("", "margin_ms")?,
      failure_touches: row.try_get("", "failure_touches")?,
      honours_idle_override: row.try_get("", "honours_idle_override")?,
    },
    ping_bound_ms: row.try_get("", "ping_bound_ms")?,
    group,
    warm_by_default: row.try_get("", "warm_by_default")?,
    switch: row.try_get("", "switch")?,
    paused_at: row.try_get("", "paused_at")?,
    record: Record {
      last_used_at: row.try_get("", "last_used_at")?,
      last_warmed_at: row.try_get("", "last_warmed_at")?,
      last_failed_at: row.try_get("", "last_failed_at")?,
    },
    last_attempt,
  })
}

async fn exists(db: &impl ConnectionTrait, sql: &str, values: Vec<Value>) -> Result<bool, DbErr> {
  Ok(db.query_one(statement(sql, values)).await?.is_some())
}

async fn consumer_exists(db: &impl ConnectionTrait, consumer: &str) -> Result<bool, DbErr> {
  exists(db, r#"SELECT 1 FROM "consumers" WHERE "name" = ?1"#, vec![consumer.into()]).await
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

async fn subject_of(db: &impl ConnectionTrait, consumer: &str, subject: &str) -> Result<Subject, DbErr> {
  let row = db
    .query_one(statement(
      r#"SELECT s."name", s."switch", s."paused_at",
                (SELECT COUNT(*) FROM "targets" t WHERE t."consumer" = s."consumer" AND t."subject" = s."name") AS "targets"
         FROM "subjects" s WHERE s."consumer" = ?1 AND s."name" = ?2"#,
      [consumer.into(), subject.into()],
    ))
    .await?
    .ok_or_else(|| DbErr::RecordNotFound(format!("the subject {subject}")))?;

  subject_row(&row)
}

fn subject_row(row: &QueryResult) -> Result<Subject, DbErr> {
  Ok(Subject {
    subject: row.try_get("", "name")?,
    switch: row.try_get("", "switch")?,
    paused_at: row.try_get("", "paused_at")?,
    targets: row.try_get("", "targets")?,
  })
}

impl Store {
  /// The store in the SQLite file at `path`, created where missing and migrated to the latest shape.
  pub async fn open(path: &Path) -> Result<Self, StoreError> {
    let options = SqliteConnectOptions::new().filename(path).create_if_missing(true).foreign_keys(true);
    let pool = SqlitePoolOptions::new()
      .max_connections(4)
      .connect_with(options)
      .await
      .map_err(|error| DbErr::Conn(RuntimeErr::SqlxError(error)))?;
    let db = SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);

    Migrator::up(&db, None).await?;
    Ok(Self { db })
  }

  /// The connection under the store, for the migrations' own tests.
  pub fn connection(&self) -> &DatabaseConnection {
    &self.db
  }

  async fn transaction(&self) -> Result<DatabaseTransaction, StoreError> {
    Ok(self.db.begin().await?)
  }

  /// Registers a consumer or moves its hook; true when it is new.
  pub async fn put_consumer(&self, consumer: &str, hook_url: &str) -> Result<bool, StoreError> {
    let txn = self.transaction().await?;
    let new = !consumer_exists(&txn, consumer).await?;

    txn
      .execute(statement(
        r#"INSERT INTO "consumers" ("name", "hook_url") VALUES (?1, ?2)
           ON CONFLICT ("name") DO UPDATE SET "hook_url" = excluded."hook_url""#,
        [consumer.into(), hook_url.into()],
      ))
      .await?;
    txn.commit().await?;
    Ok(new)
  }

  /// A consumer's hook URL; none when there is no such consumer.
  pub async fn hook_url(&self, consumer: &str) -> Result<Option<String>, StoreError> {
    let row = self
      .db
      .query_one(statement(r#"SELECT "hook_url" FROM "consumers" WHERE "name" = ?1"#, [consumer.into()]))
      .await?;

    Ok(row.map(|row| row.try_get("", "hook_url")).transpose()?)
  }

  /// Declares a group or changes its limit; true when it is new.
  pub async fn put_group(&self, consumer: &str, group: &str, limit: i64) -> Result<bool, StoreError> {
    let txn = self.transaction().await?;
    if !consumer_exists(&txn, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    let new = !exists(
      &txn,
      r#"SELECT 1 FROM "groups" WHERE "consumer" = ?1 AND "name" = ?2"#,
      vec![consumer.into(), group.into()],
    )
    .await?;

    txn
      .execute(statement(
        r#"INSERT INTO "groups" ("consumer", "name", "limit") VALUES (?1, ?2, ?3)
           ON CONFLICT ("consumer", "name") DO UPDATE SET "limit" = excluded."limit""#,
        [consumer.into(), group.into(), limit.into()],
      ))
      .await?;
    txn.commit().await?;
    Ok(new)
  }

  /// Retires a group no target names.
  pub async fn delete_group(&self, consumer: &str, group: &str) -> Result<(), StoreError> {
    let txn = self.transaction().await?;
    let key = || vec![consumer.into(), group.into()];

    if !exists(&txn, r#"SELECT 1 FROM "groups" WHERE "consumer" = ?1 AND "name" = ?2"#, key()).await? {
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

  /// Registers a target or restates one, keeping its record; true when it is new. Its subject is made
  /// where none stands.
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
    if let Some(group) = &registration.group {
      let named = vec![consumer.into(), group.as_str().into()];
      if !exists(&txn, r#"SELECT 1 FROM "groups" WHERE "consumer" = ?1 AND "name" = ?2"#, named).await? {
        return Err(StoreError::UnknownGroup);
      }
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
          registration.kind.as_str().into(),
          registration.label.as_str().into(),
          registration.subject.as_str().into(),
          registration.rank.into(),
          clocks.idle_ms.into(),
          clocks.lifetime_ms.into(),
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

  /// Retires a target with its record and its attempts; its subject stays.
  pub async fn delete_target(&self, consumer: &str, target: &str) -> Result<(), StoreError> {
    let gone = self
      .db
      .execute(statement(
        r#"DELETE FROM "targets" WHERE "consumer" = ?1 AND "name" = ?2"#,
        [consumer.into(), target.into()],
      ))
      .await?;

    if gone.rows_affected() == 0 {
      return Err(StoreError::UnknownTarget);
    }
    Ok(())
  }

  /// Reports a use: the last use moves to `at` where that is later, and a pause on the subject set before
  /// `started_at` is cleared.
  pub async fn report_use(
    &self,
    consumer: &str,
    target: &str,
    at: i64,
    started_at: i64,
  ) -> Result<(), StoreError> {
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
    txn
      .execute(statement(
        r#"UPDATE "subjects" SET "paused_at" = NULL WHERE "consumer" = ?1 AND "name" = ?2 AND "paused_at" < ?3"#,
        [consumer.into(), subject.into(), started_at.into()],
      ))
      .await?;
    txn.commit().await?;
    Ok(())
  }

  /// Every target, ordered by rank, then consumer, then name; `consumer` and `subject` narrow it.
  pub async fn targets(
    &self,
    consumer: Option<&str>,
    subject: Option<&str>,
  ) -> Result<Vec<Target>, StoreError> {
    let sql = format!(
      r#"{TARGETS} WHERE (?1 IS NULL OR t."consumer" = ?1) AND (?2 IS NULL OR t."subject" = ?2){ORDER}"#
    );
    let rows = self
      .db
      .query_all(statement(&sql, [consumer.map(str::to_owned).into(), subject.map(str::to_owned).into()]))
      .await?;

    Ok(rows.iter().map(target_of).collect::<Result<_, _>>()?)
  }

  /// One target; none when there is no such target.
  pub async fn target(&self, consumer: &str, target: &str) -> Result<Option<Target>, StoreError> {
    let sql = format!(r#"{TARGETS} WHERE t."consumer" = ?1 AND t."name" = ?2"#);
    let row = self.db.query_one(statement(&sql, [consumer.into(), target.into()])).await?;

    Ok(row.as_ref().map(target_of).transpose()?)
  }

  /// A consumer's subjects, by name.
  pub async fn subjects(&self, consumer: &str) -> Result<Vec<Subject>, StoreError> {
    if !consumer_exists(&self.db, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    let rows = self
      .db
      .query_all(statement(
        r#"SELECT s."name", s."switch", s."paused_at",
                  (SELECT COUNT(*) FROM "targets" t WHERE t."consumer" = s."consumer" AND t."subject" = s."name") AS "targets"
           FROM "subjects" s WHERE s."consumer" = ?1 ORDER BY s."name""#,
        [consumer.into()],
      ))
      .await?;

    Ok(rows.iter().map(subject_row).collect::<Result<_, _>>()?)
  }

  /// Throws a subject's switch, or unsets it with none; the subject is made where none stands.
  pub async fn set_switch(
    &self,
    consumer: &str,
    subject: &str,
    switch: Option<bool>,
  ) -> Result<Subject, StoreError> {
    self
      .on_subject(consumer, subject, r#"UPDATE "subjects" SET "switch" = ?3"#, switch.into())
      .await
  }

  /// Pauses a subject at `at`, moving a pause already standing; the subject is made where none stands.
  pub async fn pause(&self, consumer: &str, subject: &str, at: i64) -> Result<Subject, StoreError> {
    self
      .on_subject(consumer, subject, r#"UPDATE "subjects" SET "paused_at" = ?3"#, at.into())
      .await
  }

  async fn on_subject(
    &self,
    consumer: &str,
    subject: &str,
    update: &str,
    value: Value,
  ) -> Result<Subject, StoreError> {
    let txn = self.transaction().await?;
    if !consumer_exists(&txn, consumer).await? {
      return Err(StoreError::UnknownConsumer);
    }
    ensure_subject(&txn, consumer, subject).await?;
    txn
      .execute(statement(
        &format!(r#"{update} WHERE "consumer" = ?1 AND "name" = ?2"#),
        [consumer.into(), subject.into(), value],
      ))
      .await?;
    let answer = subject_of(&txn, consumer, subject).await?;
    txn.commit().await?;
    Ok(answer)
  }

  /// Whether the owner has stopped the round.
  pub async fn round_stopped(&self) -> Result<bool, StoreError> {
    let row = self
      .db
      .query_one(statement(r#"SELECT "stopped" FROM "round" WHERE "only" = 1"#, []))
      .await?
      .ok_or_else(|| DbErr::RecordNotFound("the round".into()))?;

    Ok(row.try_get("", "stopped")?)
  }

  /// Stops the round or lets it run.
  pub async fn set_round_stopped(&self, stopped: bool) -> Result<(), StoreError> {
    self
      .db
      .execute(statement(r#"UPDATE "round" SET "stopped" = ?1 WHERE "only" = 1"#, [stopped.into()]))
      .await?;
    Ok(())
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
        outcome: Outcome::parse(&outcome)
          .ok_or_else(|| DbErr::Type(format!("an attempt's outcome reads {outcome}")))?,
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
}
