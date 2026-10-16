//! The ticker (docs/warming.md, "The pass") on tokio's paused clock: the first pass a start delay after
//! the start, then one every tick; a pass that outruns its tick puts the next off until it ends, and the
//! ticks it missed are not run to catch up.

use std::time::Duration;

use bmo_cache_warmer::ticker::run_ticks;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

const DELAY: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(60);

/// Runs the ticker over passes that each take, in turn, the seconds `lengths` gives (none once they run
/// out), and answers when each of the first `count` passes began and ended, in seconds from the start.
async fn passes(lengths: Vec<u64>, count: usize) -> Vec<(u64, u64)> {
  let start = Instant::now();
  let (sender, mut receiver) = mpsc::unbounded_channel();
  let mut lengths = lengths.into_iter();
  let ticker = tokio::spawn(run_ticks(DELAY, TICK, move || {
    let length = lengths.next().unwrap_or(0);
    let sender = sender.clone();
    async move {
      let began = start.elapsed().as_secs();
      sleep(Duration::from_secs(length)).await;
      sender.send((began, start.elapsed().as_secs())).expect("the test listens");
    }
  }));

  let mut seen = Vec::new();
  while seen.len() < count {
    seen.push(receiver.recv().await.expect("the ticker runs"));
  }
  ticker.abort();
  seen
}

#[tokio::test(start_paused = true)]
async fn the_first_pass_waits_the_start_delay_and_the_rest_come_a_tick_apart() {
  assert_eq!(passes(vec![], 4).await, vec![(30, 30), (90, 90), (150, 150), (210, 210)]);
}

#[tokio::test(start_paused = true)]
async fn a_pass_shorter_than_its_tick_leaves_the_ticks_where_they_stand() {
  assert_eq!(passes(vec![0, 45], 3).await, vec![(30, 30), (90, 135), (150, 150)]);
}

#[tokio::test(start_paused = true)]
async fn a_pass_that_outruns_its_ticks_puts_the_next_off_and_the_missed_ones_are_not_run() {
  // The second pass runs past the ticks at 150 and 210: the next pass begins as it ends, once, and the
  // one after a whole tick later.
  let seen = passes(vec![0, 150], 4).await;

  assert_eq!(seen, vec![(30, 30), (90, 240), (240, 240), (300, 300)]);
  for pair in seen.windows(2) {
    assert!(pair[1].0 >= pair[0].1, "a pass began before the one before it ended: {seen:?}");
  }
}
