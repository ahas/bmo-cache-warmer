//! The store's shape, as migrations that run up and down (docs/store.md, "How the shape moves").

use sea_orm_migration::MigrationTrait;
pub use sea_orm_migration::MigratorTrait;

mod m20261017_000001_tables;
mod m20261020_000002_ids;

/// Every migration of the store, in the order they run.
pub struct Migrator;

impl MigratorTrait for Migrator {
  fn migrations() -> Vec<Box<dyn MigrationTrait>> {
    vec![Box::new(m20261017_000001_tables::Migration), Box::new(m20261020_000002_ids::Migration)]
  }
}
