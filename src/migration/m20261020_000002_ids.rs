//! Ids for every member, the pause's end, the round's interval and the event window (docs/store.md, "The
//! tables").

use sea_orm_migration::async_trait::async_trait;
use sea_orm_migration::sea_orm::ConnectionTrait;
use sea_orm_migration::{DbErr, MigrationName, MigrationTrait, SchemaManager};

pub struct Migration;

impl MigrationName for Migration {
  fn name(&self) -> &str {
    "m20261020_000002_ids"
  }
}

// Each way is one text run on one connection, since turning foreign keys off holds only on the connection
// that turns them off.
#[async_trait]
impl MigrationTrait for Migration {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager.get_connection().execute_unprepared(include_str!("ids.sql")).await.map(drop)
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .get_connection()
      .execute_unprepared(include_str!("ids.down.sql"))
      .await
      .map(drop)
  }
}
