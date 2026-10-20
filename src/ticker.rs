//! The ticker (docs/warming.md, "The pass"): the first pass a start delay after the service starts, then
//! one every interval of the round, never two at once and never run twice to catch up; and the beat it
//! keeps, which says when the next pass comes and takes a new interval while it runs.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{Instant, sleep_until};

use crate::pass::Warmer;

/// When the ticker's passes come: the next one, the one before it and the interval between.
pub struct Beat {
  times: Mutex<Times>,
  changed: Notify,
}

struct Times {
  /// When the next pass is due.
  next: Instant,
  /// When the last pass was due; none before the first.
  last: Option<Instant>,
  interval: Duration,
}

impl Beat {
  /// A beat whose first pass is `start_delay` from now, the rest `interval` apart.
  pub fn new(start_delay: Duration, interval: Duration) -> Self {
    Self {
      times: Mutex::new(Times {
        next: Instant::now() + start_delay,
        last: None,
        interval,
      }),
      changed: Notify::new(),
    }
  }

  fn times(&self) -> std::sync::MutexGuard<'_, Times> {
    self.times.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
  }

  /// When the next pass is due.
  pub fn next(&self) -> Instant {
    self.times().next
  }

  pub fn interval(&self) -> Duration {
    self.times().interval
  }

  /// Takes a new interval: the next pass moves to the last one's time plus it, or to now where that has
  /// passed; before the first pass, the first stays where it stands.
  pub fn set_interval(&self, interval: Duration) {
    let mut times = self.times();
    times.interval = interval;
    if let Some(last) = times.last {
      times.next = (last + interval).max(Instant::now());
    }
    drop(times);
    self.changed.notify_one();
  }

  /// The pass due at `due` has run: the next comes an interval after `due`, or at once where the pass ran
  /// past that.
  fn passed(&self, due: Instant) {
    let mut times = self.times();
    times.last = Some(due);
    times.next = (due + times.interval).max(Instant::now());
  }
}

/// Runs `pass` whenever `beat` says the next is due. Each pass is awaited before the next is taken, so one
/// that outruns its interval puts the next off until it ends, and the passes it missed are not run to
/// catch up: the next comes at once and the rest a whole interval apart from it. A new interval taken
/// while it waits moves the wait.
pub async fn run_ticks<P, F>(beat: &Beat, mut pass: P) -> !
where
  P: FnMut() -> F,
  F: Future<Output = ()>,
{
  loop {
    let due = beat.next();
    let changed = beat.changed.notified();
    tokio::select! {
      () = sleep_until(due) => {}
      () = changed => continue,
    }
    pass().await;
    beat.passed(due);
  }
}

/// Runs the warmer's passes on its round's beat for as long as the service runs. A pass asked for over the
/// routes and still running is waited for; a pass the store fails is written to standard error and the
/// next comes as usual.
pub async fn warm(warmer: Arc<Warmer>) -> ! {
  run_ticks(warmer.beat(), || async {
    if let Err(error) = warmer.pass(None).await {
      eprintln!("A pass ended early. {error}");
    }
  })
  .await
}
