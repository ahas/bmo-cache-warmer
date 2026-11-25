//! A keepalive scheduler. Consumers register the prompt-cache prefixes they want kept warm, with the
//! clocks each runs on, record every use, and serve a target-ping hook; the warmer runs the pass, judges
//! which targets are due, holds the switches and keeps the record of every ping (`docs/api.md`,
//! `docs/warming.md`, `docs/store.md`). Beside the service stand its client, its OpenAPI document and the
//! harness plugin's MCP server. The schedule rule is the crate `bmo-cache-warmer-schedule`, re-exported
//! here as `schedule`.

pub use bmo_cache_warmer_schedule as schedule;

pub mod client;
pub mod clock;
pub mod mcp;
pub mod migration;
pub mod openapi;
pub mod pass;
pub mod policy;
pub mod routes;
pub mod service;
pub mod settings;
pub mod snapshot;
pub mod stop;
pub mod store;
pub mod ticker;
pub mod wire;
pub mod words;

/// The version the warmer's health, its MCP server and its command name: the set's version the build was
/// made at, which the testbed hands the build as `BMO_VERSION`, else the manifest's placeholder (bmo-chat's
/// `docs/conventions.md`, 「Versions」).
pub const VERSION: &str = match option_env!("BMO_VERSION") {
  Some(version) => version,
  None => env!("CARGO_PKG_VERSION"),
};
