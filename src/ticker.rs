//! The ticker (docs/warming.md, "The pass"): the first pass a start delay after the service starts, then
//! one every tick, never two at once and never run twice to catch up.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::pass::Warmer;

/// Runs `pass` first `start_delay` from now, then every `every`. Each pass is awaited before the next
/// tick is taken, so one that outruns its tick puts the next off until it ends, and the ticks it missed
/// are not run to catch up: the next comes at once and the rest a whole tick apart from it.
pub async fn run_ticks<P, F>(start_delay: Duration, every: Duration, mut pass: P) -> !
where
  P: FnMut() -> F,
  F: Future<Output = ()>,
{
  let mut ticks = interval_at(Instant::now() + start_delay, every);
  ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
  loop {
    ticks.tick().await;
    pass().await;
  }
}

/// Runs the warmer's passes on its policy's clocks for as long as the service runs. A pass asked for
/// over the routes and still running is waited for; a pass the store fails is written to standard
/// error and the next tick comes as usual.
pub async fn warm(warmer: Arc<Warmer>) -> ! {
  let policy = warmer.policy();
  run_ticks(policy.start_delay(), policy.tick(), || async {
    if let Err(error) = warmer.pass(None).await {
      eprintln!("A pass ended early. {error}");
    }
  })
  .await
}
