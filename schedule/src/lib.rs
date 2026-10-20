//! The keepalive schedule's general rule (docs/warming.md, 「When a target is due」 and 「Groups」): when a kept
//! prompt-cache prefix is due a ping, in what order a pass pings its targets, and the switches as values.
//! Everything is judged from what it is handed, `now` included; the crate reads no clock, no store and no
//! policy, and has no dependency of any kind, so another repository may take the rule alone from this crate
//! (docs/api.md, 「The schedule crate」).

mod clocks;
mod plan;
mod reason;
mod switches;

pub use clocks::{Clocks, Record, usable};
pub use plan::{Candidate, Group, Pass, plan};
pub use reason::first_units;
pub use switches::{is_on, use_clears_pause};
