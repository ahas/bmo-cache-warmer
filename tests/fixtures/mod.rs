//! What the store's, the pass's, the routes' and the stop's tests share: a store in a scratch directory, a
//! clock the test moves, a registration to vary, and a wait for a port to close.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use bmo_cache_warmer::clock::Clock;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::schedule::Clocks;
use bmo_cache_warmer::store::{Registration, Store};
use tempfile::TempDir;
use tokio::net::TcpStream;

pub const MINUTE: i64 = 60_000;
pub const IDLE: i64 = 40 * MINUTE;
pub const NOW: i64 = 10_000_000_000;

/// A store in a scratch directory that lives as long as the directory is held.
pub async fn store() -> (TempDir, Store) {
  let dir = tempfile::tempdir().expect("a scratch directory");
  let store = Store::open(&dir.path().join("warmer.sqlite")).await.expect("the store opens");

  (dir, store)
}

/// A warmer over a fresh store on the built-in policy and a clock standing at `NOW`.
pub async fn warmer() -> (TempDir, Arc<Warmer>, Arc<TestClock>) {
  let (dir, store) = store().await;
  let clock = Arc::new(TestClock::at(NOW));
  let policy = Policy::builtin().expect("the built-in policy reads");
  let warmer = Warmer::open(store, policy, clock.clone()).await.expect("the warmer opens");

  (dir, Arc::new(warmer), clock)
}

/// A clock that stands where the test puts it.
pub struct TestClock(AtomicI64);

impl TestClock {
  pub fn at(now: i64) -> Self {
    Self(AtomicI64::new(now))
  }

  pub fn advance(&self, by: i64) {
    self.0.fetch_add(by, Ordering::SeqCst);
  }
}

impl Clock for TestClock {
  fn now(&self) -> i64 {
    self.0.load(Ordering::SeqCst)
  }
}

/// A launch line's registration: idle 40 minutes, lifetime 60, failures touch, warm by default.
pub fn line(subject: &str) -> Registration {
  Registration {
    kind: Some("launch-line".into()),
    label: Some("a launch line".into()),
    subject: subject.into(),
    rank: 50,
    clocks: Clocks {
      idle_ms: IDLE,
      lifetime_ms: Some(60 * MINUTE),
      margin_ms: 0,
      failure_touches: true,
      honours_idle_override: true,
    },
    ping_bound_ms: Some(90_000),
    group: None,
    warm_by_default: Some(true),
    last_used_at: None,
  }
}

/// A pool place's registration in `group`: lifetime 60 less 5, failures do not touch.
pub fn place(subject: &str, group: &str) -> Registration {
  let mut registration = line(subject);
  registration.kind = Some("pool-place".into());
  registration.rank = 10;
  registration.clocks.margin_ms = 5 * MINUTE;
  registration.clocks.failure_touches = false;
  registration.clocks.honours_idle_override = false;
  registration.ping_bound_ms = Some(120_000);
  registration.group = Some(group.into());
  registration
}

/// Waits, a bounded number of times, for a port to refuse a new connection: whether it came to.
pub async fn refuses(address: SocketAddr) -> bool {
  for _ in 0..400 {
    if TcpStream::connect(address).await.is_err() {
      return true;
    }
    tokio::time::sleep(Duration::from_millis(5)).await;
  }
  false
}
