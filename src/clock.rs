//! The clock the pass judges and records by. The service reads the system's; a test hands in its own.

use std::time::{SystemTime, UNIX_EPOCH};

/// Where the warmer reads the time, in epoch milliseconds.
pub trait Clock: Send + Sync {
  fn now(&self) -> i64;
}

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
  fn now(&self) -> i64 {
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
  }
}
