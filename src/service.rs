//! The service (docs/api.md, "The service"): the routes served and the ticker run until the stop's signal,
//! then a drain that takes nothing new and waits for what is in flight, up to a deadline or a second
//! signal.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use tokio::net::TcpListener;

use crate::pass::Warmer;
use crate::routes::router;
use crate::ticker::warm;

/// How a stop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
  /// Everything in flight finished within the deadline.
  Drained,
  /// The deadline ran out, or a second signal came, with calls, pings or passes still in flight, which
  /// the caller's exit cuts.
  Cut,
}

/// Serves the routes on `listener` behind `token` and runs the ticker, until the first of `signals` comes
/// or serving fails; then begins the warmer's stop, so the port closes, the ticker starts no pass and every
/// line ends before its next ping, and waits for the calls, pings and passes in flight to finish, up to
/// `drain` or the next of `signals`, whichever comes first. Signals that end leave the drain to its
/// deadline.
pub async fn serve(
  listener: TcpListener,
  warmer: Arc<Warmer>,
  token: &str,
  signals: impl Stream<Item = ()>,
  drain: Duration,
) -> io::Result<Ended> {
  let served = async {
    let ended = axum::serve(listener, router(warmer.clone(), token))
      .with_graceful_shutdown(warmer.stop().begun())
      .await;
    // Serving that ends of itself stops the rest too, so the drain below still ends.
    warmer.stop().begin();
    ended
  };
  let drained = async { tokio::join!(served, warm(warmer.clone())).0 };
  tokio::pin!(drained);
  let mut signals = std::pin::pin!(signals);

  // The server and the ticker run inside `drained`, so it is polled while the signal is awaited.
  tokio::select! {
    ended = &mut drained => return ended.map(|()| Ended::Drained),
    Some(()) = signals.next() => {}
    () = warmer.stop().begun() => {}
  }
  warmer.stop().begin();
  eprintln!(
    "The service is stopping: it takes nothing new and waits up to {} seconds for what is in flight.",
    drain.as_secs_f64()
  );

  tokio::select! {
    ended = tokio::time::timeout(drain, drained) => match ended {
      Ok(ended) => ended.map(|()| Ended::Drained),
      Err(_) => Ok(Ended::Cut),
    },
    Some(()) = signals.next() => Ok(Ended::Cut),
  }
}
