//! A keepalive scheduler. Consumers register the prompt-cache prefixes they want kept warm, with the
//! clocks each runs on, report every use, and answer a ping hook; the warmer runs the pass, judges which
//! targets are due, holds the owner's switches and keeps the record of every ping (`docs/api.md`,
//! `docs/warming.md`).

pub mod due;
pub mod policy;
