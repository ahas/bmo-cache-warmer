//! The service's stop (docs/api.md, "The service"): served on loopback with a fake consumer whose hook the
//! test holds, stopped by a signal the test sends, and drained within a deadline the test sets.

use crate::fixtures;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode as Status;
use axum::routing::post;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::service::{Ended, serve};
use bmo_cache_warmer::store::Outcome;
use fixtures::{NOW, line, refuses, warmer};
use reqwest::StatusCode;
use serde_json::Value;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

const TOKEN: &str = "a-test-token";

/// A hook that says each ping has arrived, then answers warmed once released, or never where it is told so.
async fn hook(answers: bool) -> (String, Arc<Notify>, Arc<Notify>) {
  let (arrived, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
  let app = Router::new().route(
    "/ping",
    post({
      let (arrived, release) = (arrived.clone(), release.clone());
      move || async move {
        arrived.notify_one();
        if !answers {
          std::future::pending::<()>().await;
        }
        release.notified().await;
        Status::NO_CONTENT
      }
    }),
  );
  let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let url = format!("http://{}/ping", listener.local_addr().expect("the port's address"));
  tokio::spawn(async move { axum::serve(listener, app).await.expect("the fake consumer serves") });

  (url, arrived, release)
}

/// The service serving the consumer `alpha`, whose two due targets `first` and `second` go on its one line
/// in that order, with the signal that stops it.
struct Running {
  _dir: TempDir,
  warmer: Arc<Warmer>,
  address: SocketAddr,
  http: reqwest::Client,
  signal: Option<oneshot::Sender<()>>,
  served: JoinHandle<std::io::Result<Ended>>,
}

async fn running(hook_url: &str, drain: Duration) -> Running {
  let (dir, warmer, _clock) = warmer().await;
  warmer.store().put_consumer("alpha", hook_url, NOW).await.expect("the consumer");
  for (target, subject) in [("first", "a"), ("second", "b")] {
    warmer.store().put_target("alpha", target, &line(subject)).await.expect("a target");
  }
  let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let address = listener.local_addr().expect("the port's address");
  let (signal, signalled) = oneshot::channel();
  let served = tokio::spawn({
    let warmer = warmer.clone();
    async move {
      let signalled = futures_util::stream::once(async {
        let _ = signalled.await;
      });
      serve(listener, warmer, TOKEN, signalled, drain).await
    }
  });

  Running {
    _dir: dir,
    warmer,
    address,
    http: reqwest::Client::builder().no_proxy().build().expect("a client"),
    signal: Some(signal),
    served,
  }
}

impl Running {
  fn url(&self, path: &str) -> String {
    format!("http://{}{path}", self.address)
  }

  /// Asks for a pass, which answers once it ends.
  fn pass(&self) -> JoinHandle<reqwest::Result<reqwest::Response>> {
    let request = self.http.post(self.url("/passes")).bearer_auth(TOKEN);
    tokio::spawn(request.send())
  }

  fn stop(&mut self) {
    self
      .signal
      .take()
      .expect("one stop")
      .send(())
      .expect("the service listens for its stop");
  }

  async fn outcome(&self, target: &str) -> Option<Outcome> {
    let found = self.warmer.store().target("alpha", target).await.expect("the store reads");
    found.expect("the target stands").last_attempt.map(|attempt| attempt.outcome)
  }
}

#[tokio::test]
async fn a_stop_closes_the_port_ends_the_stream_and_answers_the_pass_in_flight_without_its_next_ping() {
  let (hook_url, arrived, release) = hook(true).await;
  let mut service = running(&hook_url, Duration::from_secs(60)).await;
  let stream = service
    .http
    .get(service.url("/events"))
    .bearer_auth(TOKEN)
    .send()
    .await
    .expect("the event stream opens");
  assert_eq!(stream.status(), StatusCode::OK);
  let pass = service.pass();
  arrived.notified().await;

  service.stop();
  // The stream and the port end while the ping is still out: neither waits on it.
  stream.text().await.expect("the event stream ends cleanly");
  assert!(refuses(service.address).await, "the port still takes connections");
  release.notify_one();

  let answer = pass.await.expect("the call's task").expect("the call in flight is answered");
  assert_eq!(answer.status(), StatusCode::CREATED);
  let pass: Value = answer.json().await.expect("the pass");
  assert_eq!((pass["pingedCount"].as_i64(), pass["stopped"].as_bool()), (Some(1), Some(true)));
  assert_eq!(
    (&mut service.served).await.expect("the service's task").expect("it serves"),
    Ended::Drained
  );
  assert_eq!(service.outcome("first").await, Some(Outcome::Warmed));
  assert_eq!(service.outcome("second").await, None);
}

#[tokio::test]
async fn a_drain_deadline_cuts_a_ping_that_never_answers_and_records_nothing() {
  let (hook_url, arrived, _release) = hook(false).await;
  let mut service = running(&hook_url, Duration::from_millis(50)).await;
  let _pass = service.pass();
  arrived.notified().await;

  service.stop();

  assert_eq!((&mut service.served).await.expect("the service's task").expect("it serves"), Ended::Cut);
  assert_eq!(service.outcome("first").await, None);
}
