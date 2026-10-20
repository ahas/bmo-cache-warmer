//! A keepalive scheduler. Consumers register the prompt-cache prefixes they want kept warm, with the
//! clocks each runs on, record every use, and serve a target-ping hook; the warmer runs the pass, judges
//! which targets are due, holds the switches and keeps the record of every ping (`docs/api.md`,
//! `docs/warming.md`, `docs/store.md`). Beside the service stand its client, its OpenAPI document and the
//! harness plugin's MCP server, and the schedule rule as a library of its own.

pub mod client;
pub mod clock;
pub mod mcp;
pub mod migration;
pub mod openapi;
pub mod pass;
pub mod policy;
pub mod routes;
pub mod schedule;
pub mod snapshot;
pub mod store;
pub mod ticker;
pub mod wire;
pub mod words;
