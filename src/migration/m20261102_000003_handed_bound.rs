//! A target's ping stretch, the consumer's own and handed back in each ping, declared or not (docs/store.md,
//! "The tables").

use sea_orm_migration::async_trait::async_trait;
use sea_orm_migration::sea_orm::ConnectionTrait;
use sea_orm_migration::{DbErr, MigrationName, MigrationTrait, SchemaManager};

pub struct Migration;

impl MigrationName for Migration {
  fn name(&self) -> &str {
    "m20261102_000003_handed_bound"
  }
}

#[async_trait]
impl MigrationTrait for Migration {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .get_connection()
      .execute_unprepared(include_str!("handed_bound.sql"))
      .await
      .map(drop)
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .get_connection()
      .execute_unprepared(include_str!("handed_bound.down.sql"))
      .await
      .map(drop)
  }
}
