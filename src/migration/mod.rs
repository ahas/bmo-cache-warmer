//! The store's shape, as migrations that run up and down, from the 1.0.0 baseline (docs/store.md, "How the
//! shape moves").

use sea_orm_migration::MigrationTrait;
pub use sea_orm_migration::MigratorTrait;
use sea_orm_migration::sea_orm::{
  ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait,
};

mod m20261124_000001_baseline;

/// The migration that makes the whole shape.
pub const BASELINE: &str = m20261124_000001_baseline::NAME;

/// The record an install from before 1.0.0 holds once every migration of its last release ran. Its shape is
/// the baseline's.
pub const BEFORE: [&str; 3] =
  ["m20261017_000001_tables", "m20261020_000002_ids", "m20261102_000003_handed_bound"];

/// Every migration of the store, in the order they run.
pub struct Migrator;

impl MigratorTrait for Migrator {
  fn migrations() -> Vec<Box<dyn MigrationTrait>> {
    vec![Box::new(m20261124_000001_baseline::Migration)]
  }
}

/// The migrations run up, the baseline first. A store whose record names every migration of the last release
/// before 1.0.0, and nothing else, has that record rewritten to name the baseline alone, touching no table and
/// no row; one whose record names only part of them is refused.
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
  adopt(db).await?;
  Migrator::up(db, None).await
}

async fn adopt(db: &DatabaseConnection) -> Result<(), DbErr> {
  let tx = db.begin().await?;
  let statement = |sql: &str, values: Vec<sea_orm_migration::sea_orm::Value>| {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, values)
  };
  let recorded = tx
    .query_one(statement(
      "SELECT COUNT(*) AS held FROM sqlite_master WHERE type = 'table' AND name = 'seaql_migrations'",
      vec![],
    ))
    .await?
    .map(|row| row.try_get::<i64>("", "held"))
    .transpose()?
    .unwrap_or(0)
    > 0;
  if !recorded {
    return Ok(());
  }
  let held = tx
    .query_all(statement(r#"SELECT "version" FROM "seaql_migrations" ORDER BY "version""#, vec![]))
    .await?
    .into_iter()
    .map(|row| row.try_get::<String>("", "version"))
    .collect::<Result<Vec<_>, _>>()?;
  if held.is_empty() || !held.iter().all(|version| BEFORE.contains(&version.as_str())) {
    return Ok(());
  }
  if held.len() != BEFORE.len() {
    let reached = held.last().map_or("", String::as_str);
    return Err(DbErr::Migration(format!(
      "the store holds the migrations of a release before 1.0.0 only through {reached}; that release's last \
       migration runs first"
    )));
  }
  tx.execute_unprepared(r#"DELETE FROM "seaql_migrations""#).await?;
  tx.execute(statement(
    r#"INSERT INTO "seaql_migrations" ("version", "applied_at") VALUES (?, CAST(strftime('%s', 'now') AS INTEGER))"#,
    vec![BASELINE.into()],
  ))
  .await?;
  tx.commit().await
}
