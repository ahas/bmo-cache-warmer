//! The store's tables, made in one step (docs/store.md, "The tables").

use sea_orm_migration::async_trait::async_trait;
use sea_orm_migration::sea_orm::ConnectionTrait;
use sea_orm_migration::{DbErr, MigrationName, MigrationTrait, SchemaManager};

pub struct Migration;

impl MigrationName for Migration {
  fn name(&self) -> &str {
    "m20261017_000001_tables"
  }
}

#[async_trait]
impl MigrationTrait for Migration {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager.get_connection().execute_unprepared(include_str!("tables.sql")).await.map(drop)
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .get_connection()
      .execute_unprepared(include_str!("tables.down.sql"))
      .await
      .map(drop)
  }
}
