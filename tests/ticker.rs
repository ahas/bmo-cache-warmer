//! The ticker (docs/warming.md, "The pass") on tokio's paused clock: the first pass a start delay after
//! the start, then one every interval; a pass that outruns its interval puts the next off until it ends,
//! and the passes it missed are not run to catch up; a new interval moves the next pass.

use std::sync::Arc;
use std::time::Duration;

use bmo_cache_warmer::ticker::{Beat, run_ticks};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

const DELAY: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(60);

/// Runs the ticker on `beat` over passes that each take, in turn, the seconds `lengths` gives (none once
/// they run out); answers the receiver of when each pass began and ended, in seconds from `start`.
fn ticking(
  beat: Arc<Beat>,
  start: Instant,
  lengths: Vec<u64>,
) -> (mpsc::UnboundedReceiver<(u64, u64)>, tokio::task::JoinHandle<()>) {
  let (sender, receiver) = mpsc::unbounded_channel();
  let mut lengths = lengths.into_iter();
  let ticker = tokio::spawn(async move {
    run_ticks(&beat, move || {
      let length = lengths.next().unwrap_or(0);
      let sender = sender.clone();
      async move {
        let began = start.elapsed().as_secs();
        sleep(Duration::from_secs(length)).await;
        sender.send((began, start.elapsed().as_secs())).expect("the test listens");
      }
    })
    .await
  });

  (receiver, ticker)
}

/// The first `count` passes over passes of the given lengths, on a fixed interval.
async fn passes(lengths: Vec<u64>, count: usize) -> Vec<(u64, u64)> {
  let start = Instant::now();
  let (mut receiver, ticker) = ticking(Arc::new(Beat::new(DELAY, TICK)), start, lengths);

  let mut seen = Vec::new();
  while seen.len() < count {
    seen.push(receiver.recv().await.expect("the ticker runs"));
  }
  ticker.abort();
  seen
}

#[tokio::test(start_paused = true)]
async fn the_first_pass_waits_the_start_delay_and_the_rest_come_an_interval_apart() {
  assert_eq!(passes(vec![], 4).await, vec![(30, 30), (90, 90), (150, 150), (210, 210)]);
}

#[tokio::test(start_paused = true)]
async fn a_pass_shorter_than_its_interval_leaves_the_passes_where_they_stand() {
  assert_eq!(passes(vec![0, 45], 3).await, vec![(30, 30), (90, 135), (150, 150)]);
}

#[tokio::test(start_paused = true)]
async fn a_pass_that_outruns_its_interval_puts_the_next_off_and_the_missed_ones_are_not_run() {
  // The second pass runs past the passes due at 150 and 210: the next begins as it ends, once, and the
  // one after a whole interval later.
  let seen = passes(vec![0, 150], 4).await;

  assert_eq!(seen, vec![(30, 30), (90, 240), (240, 240), (300, 300)]);
  for pair in seen.windows(2) {
    assert!(pair[1].0 >= pair[0].1, "a pass began before the one before it ended: {seen:?}");
  }
}

#[tokio::test(start_paused = true)]
async fn a_new_interval_moves_the_next_pass_from_the_last_one() {
  let start = Instant::now();
  let beat = Arc::new(Beat::new(DELAY, TICK));
  let (mut receiver, ticker) = ticking(beat.clone(), start, vec![]);

  // Before the first pass, the first stays at the start delay.
  sleep(Duration::from_secs(10)).await;
  beat.set_interval(Duration::from_secs(20));
  assert_eq!(beat.next(), start + DELAY);
  assert_eq!(receiver.recv().await, Some((30, 30)));

  // After it, the next is the last pass's time plus the new interval.
  sleep(Duration::from_secs(5)).await;
  beat.set_interval(Duration::from_secs(100));
  assert_eq!(beat.next(), start + Duration::from_secs(130));
  assert_eq!(receiver.recv().await, Some((130, 130)));

  // An interval whose time has already passed brings the next pass now.
  sleep(Duration::from_secs(50)).await;
  beat.set_interval(Duration::from_secs(10));
  assert_eq!(receiver.recv().await, Some((180, 180)));
  assert_eq!(receiver.recv().await, Some((190, 190)));
  ticker.abort();
}
