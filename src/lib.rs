//! A keepalive scheduler. Consumers register the prompt-cache prefixes they want kept warm, with the
//! clocks each runs on, report every use, and answer a ping hook; the warmer runs the pass, judges which
//! targets are due, holds the owner's switches and keeps the record of every ping (`docs/api.md`,
//! `docs/warming.md`, `docs/store.md`).

pub mod clock;
pub mod due;
pub mod migration;
pub mod pass;
pub mod policy;
pub mod routes;
pub mod snapshot;
pub mod store;
pub mod ticker;
pub mod words;
