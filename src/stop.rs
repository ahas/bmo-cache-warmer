//! The service's stop (docs/api.md, "The service"): begun once, by the signal, and read by everything that
//! must take nothing new from then on: the server, the ticker, each pass's lines and the event stream.

use std::future::Future;

use tokio::sync::watch;

/// Whether the service is stopping; once begun, it stays begun.
pub struct Stop {
  begun: watch::Sender<bool>,
}

impl Default for Stop {
  fn default() -> Self {
    Self::new()
  }
}

impl Stop {
  /// A stop not yet begun.
  pub fn new() -> Self {
    Self {
      begun: watch::Sender::new(false),
    }
  }

  /// Begins the stop; beginning it again changes nothing.
  pub fn begin(&self) {
    self.begun.send_replace(true);
  }

  pub fn has_begun(&self) -> bool {
    *self.begun.borrow()
  }

  /// Ends once the stop has begun, at once where it already has. It holds no borrow of the stop, so it
  /// may outlive it; where the stop is dropped without beginning, it never ends.
  pub fn begun(&self) -> impl Future<Output = ()> + Send + use<> {
    let mut begun = self.begun.subscribe();
    async move {
      if begun.wait_for(|begun| *begun).await.is_err() {
        std::future::pending::<()>().await;
      }
    }
  }
}
