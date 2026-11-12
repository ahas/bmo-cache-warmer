//! The crate's one test harness (bmo-chat's docs/conventions.md, 「Rust builds」): each file of tests a
//! module of it, sharing the process.

mod fixtures;
mod support;

mod dependencies;
mod deploy;
mod hangul;
mod layout;
mod mcp;
mod openapi;
mod pass;
mod personal_data;
mod policy;
mod routes;
mod service;
mod snapshot;
mod store;
mod ticker;
