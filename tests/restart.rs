//! The binary stopped and started again (docs/api.md, "The service"): the service run as its own process on
//! a loopback port over a store in a scratch directory, sent SIGTERM in the middle of a pass whose ping a
//! stand-in consumer holds, and started again over the same store.

use crate::fixtures::refuses;

use std::net::SocketAddr;
use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode as Status;
use axum::routing::post;
use axum::{Json, Router};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const TOKEN: &str = "a-test-token";

/// How the stand-in consumer answers the next ping; every ping after it is answered warmed.
#[derive(Clone, Copy)]
enum Next {
  /// Says it has the ping, then answers warmed once released.
  Held,
  /// Says it has the ping and never answers.
  Never,
}

struct Consumer {
  next: Mutex<Option<Next>>,
  heard: Mutex<Vec<String>>,
  arrived: Notify,
  release: Notify,
}

async fn ping(State(consumer): State<Arc<Consumer>>, Json(ping): Json<Value>) -> Status {
  let target = ping["targetName"].as_str().unwrap_or_default().to_owned();
  consumer.heard.lock().expect("the log of pings").push(target);
  let next = consumer.next.lock().expect("the next answer").take();

  match next {
    Some(Next::Held) => {
      consumer.arrived.notify_one();
      consumer.release.notified().await;
    }
    Some(Next::Never) => {
      consumer.arrived.notify_one();
      std::future::pending::<()>().await;
    }
    None => {}
  }
  Status::NO_CONTENT
}

/// The stand-in consumer on a loopback port, and its hook's URL.
async fn consumer(next: Next) -> (Arc<Consumer>, String) {
  let consumer = Arc::new(Consumer {
    next: Mutex::new(Some(next)),
    heard: Mutex::new(Vec::new()),
    arrived: Notify::new(),
    release: Notify::new(),
  });
  let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let url = format!("http://{}/ping", listener.local_addr().expect("the port's address"));
  let app = Router::new().route("/ping", post(ping)).with_state(consumer.clone());
  tokio::spawn(async move { axum::serve(listener, app).await.expect("the stand-in consumer serves") });

  (consumer, url)
}

/// A scratch directory holding the token file and, once served, the store, and the loopback address every
/// run of the service listens on.
struct Place {
  dir: TempDir,
  address: SocketAddr,
  http: reqwest::Client,
}

impl Place {
  async fn new() -> Self {
    let dir = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(dir.path().join("token"), TOKEN).expect("the token file");
    let free = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
    let address = free.local_addr().expect("the port's address");
    drop(free);

    Self {
      dir,
      address,
      http: reqwest::Client::builder().no_proxy().build().expect("a client"),
    }
  }

  fn url(&self, path: &str) -> String {
    format!("http://{}{path}", self.address)
  }

  /// Starts the service and waits, a bounded number of times, for it to answer its health.
  async fn start(&self) -> Child {
    let child = service(self.dir.path(), self.address);
    for _ in 0..400 {
      let health = self.http.get(self.url("/health")).bearer_auth(TOKEN).send().await;
      if health.is_ok_and(|answer| answer.status() == StatusCode::OK) {
        return child;
      }
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the service never answered its health");
  }

  async fn put(&self, path: &str, body: Value) {
    let answer = self.http.put(self.url(path)).bearer_auth(TOKEN).json(&body).send().await;
    assert!(answer.expect("the service answers").status().is_success(), "{path}");
  }

  /// The consumer `alpha` at `hook_url`, with two targets due at once, `first` and `second`, which go on
  /// its one line in that order.
  async fn register(&self, hook_url: &str) {
    self.put("/consumers/alpha", json!({ "url": hook_url })).await;
    for (target, rank) in [("first", 1), ("second", 2)] {
      let registration = json!({ "ttlMs": 3_600_000, "rank": rank, "warmByDefault": true });
      self.put(&format!("/consumers/alpha/targets/{target}"), registration).await;
    }
  }

  /// Asks for a pass, which answers once it ends.
  fn pass(&self) -> JoinHandle<reqwest::Result<reqwest::Response>> {
    tokio::spawn(self.http.post(self.url("/passes")).bearer_auth(TOKEN).send())
  }
}

fn service(dir: &Path, address: SocketAddr) -> Child {
  Command::new(env!("CARGO_BIN_EXE_bmo-cache-warmer"))
    .arg("--listen")
    .arg(address.to_string())
    .arg("--store")
    .arg(dir.join("warmer.sqlite"))
    .arg("--token-file")
    .arg(dir.join("token"))
    .env_remove("CACHE_WARMER_LISTEN")
    .env_remove("CACHE_WARMER_STORE")
    .env_remove("CACHE_WARMER_TOKEN_FILE")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .spawn()
    .expect("the service starts")
}

async fn terminate(child: &Child) {
  let pid = child.id().expect("the service still runs").to_string();
  let sent = Command::new("kill").args(["-TERM", &pid]).status().await.expect("kill runs");
  assert!(sent.success(), "SIGTERM was not sent");
}

/// Waits for the service to exit, and what it said.
async fn exited(child: Child) -> Output {
  let waited = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output());
  waited.await.expect("the service exits").expect("its exit reads")
}

fn said(output: &Output) -> String {
  String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The targets a pass's answer pinged, and whether it found the service or the round stopped.
async fn pinged(answer: reqwest::Response) -> (Vec<String>, bool) {
  assert_eq!(answer.status(), StatusCode::CREATED);
  let pass: Value = answer.json().await.expect("the pass");
  let attempts = pass["attempts"].as_array().expect("the attempts");
  let targets = attempts.iter().map(|attempt| attempt["targetName"].as_str().unwrap().to_owned());

  (targets.collect(), pass["stopped"].as_bool().expect("stopped"))
}

#[tokio::test]
async fn sigterm_mid_pass_answers_the_call_in_flight_and_the_restart_pings_what_the_stop_left() {
  let (consumer, hook_url) = consumer(Next::Held).await;
  let place = Place::new().await;
  let running = place.start().await;
  place.register(&hook_url).await;
  let pass = place.pass();
  consumer.arrived.notified().await;

  terminate(&running).await;
  assert!(refuses(place.address).await, "the port still takes connections");
  consumer.release.notify_one();
  let answer = pass.await.expect("the call's task").expect("the call in flight is answered");
  assert_eq!(pinged(answer).await, (vec!["first".to_owned()], true));
  let output = exited(running).await;
  assert_eq!(output.status.code(), Some(0), "{}", said(&output));
  assert!(said(&output).contains("The service is stopping"), "{}", said(&output));
  assert!(!said(&output).contains("cut"), "{}", said(&output));

  let running = place.start().await;
  let answer = place.pass().await.expect("the call's task").expect("the service answers");
  assert_eq!(pinged(answer).await, (vec!["second".to_owned()], false));
  assert_eq!(*consumer.heard.lock().unwrap(), ["first", "second"]);
  terminate(&running).await;
  assert_eq!(exited(running).await.status.code(), Some(0));
}

#[tokio::test]
async fn a_second_sigterm_cuts_the_drain_and_the_restart_pings_the_cut_target_again() {
  let (consumer, hook_url) = consumer(Next::Never).await;
  let place = Place::new().await;
  let running = place.start().await;
  place.register(&hook_url).await;
  let pass = place.pass();
  consumer.arrived.notified().await;

  terminate(&running).await;
  // The port closing says the first signal was taken, so the second is not merged into it.
  assert!(refuses(place.address).await, "the port still takes connections");
  terminate(&running).await;
  let output = exited(running).await;
  assert_eq!(output.status.code(), Some(0), "{}", said(&output));
  assert!(said(&output).contains("The drain was cut"), "{}", said(&output));
  let broken = pass.await.expect("the call's task");
  assert!(broken.is_err(), "the call in flight was answered: {broken:?}");

  let running = place.start().await;
  let answer = place.pass().await.expect("the call's task").expect("the service answers");
  assert_eq!(pinged(answer).await, (vec!["first".to_owned(), "second".to_owned()], false));
  assert_eq!(*consumer.heard.lock().unwrap(), ["first", "first", "second"]);
  terminate(&running).await;
  assert_eq!(exited(running).await.status.code(), Some(0));
}
