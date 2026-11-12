//! The service (docs/api.md, "The service"): the routes served and the ticker run until the stop's signal,
//! then a drain that takes nothing new and waits for what is in flight, up to a deadline.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::pass::Warmer;
use crate::routes::router;
use crate::ticker::warm;

/// How a stop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
  /// Everything in flight finished within the deadline.
  Drained,
  /// The deadline ran out with calls, pings or passes still in flight, which the caller's exit cuts.
  Cut,
}

/// Serves the routes on `listener` behind `token` and runs the ticker, until `signal` ends or serving
/// fails; then begins the warmer's stop, so the port closes, the ticker starts no pass and every line
/// ends before its next ping, and waits up to `drain` for the calls, pings and passes in flight to
/// finish.
pub async fn serve(
  listener: TcpListener,
  warmer: Arc<Warmer>,
  token: &str,
  signal: impl Future<Output = ()>,
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

  // The server and the ticker run inside `drained`, so it is polled while the signal is awaited.
  tokio::select! {
    ended = &mut drained => return ended.map(|()| Ended::Drained),
    () = signal => {}
    () = warmer.stop().begun() => {}
  }
  warmer.stop().begin();
  eprintln!(
    "The service is stopping: it takes nothing new and waits up to {} seconds for what is in flight.",
    drain.as_secs_f64()
  );

  match tokio::time::timeout(drain, drained).await {
    Ok(ended) => ended.map(|()| Ended::Drained),
    Err(_) => Ok(Ended::Cut),
  }
}
