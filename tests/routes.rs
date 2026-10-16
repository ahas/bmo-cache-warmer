//! The served routes (docs/api.md, "Served routes" and "Codes"), asked over HTTP on loopback, with a
//! fake consumer whose hook answers warmed and a clock the test sets. The round, switch and pause tests
//! carry the old control routes' keepalive checks: the round stopped and read back, a body that does not
//! read refused, an agent's switch and a worker's pause.

mod fixtures;

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::routing::post;
use bmo_cache_warmer::migration::{Migrator, MigratorTrait};
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::routes::router;
use bmo_cache_warmer::store::Store;
use fixtures::{IDLE, MINUTE, NOW, TestClock, store};
use reqwest::Method;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// A ping held at the hook until the test releases it.
#[derive(Clone)]
struct Hold {
  arrived: Arc<Notify>,
  release: Arc<Notify>,
}

/// The warmer served on loopback, the store and clock under it, and a consumer hook that answers warmed.
struct Service {
  _dir: TempDir,
  store: Store,
  clock: Arc<TestClock>,
  base: String,
  hook_url: String,
  hold: Arc<Mutex<Option<Hold>>>,
  client: reqwest::Client,
}

async fn serve(app: Router) -> String {
  let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let address = listener.local_addr().expect("the port's address");
  tokio::spawn(async move { axum::serve(listener, app).await.expect("the server serves") });
  format!("http://{address}")
}

async fn service() -> Service {
  let (dir, store) = store().await;
  let clock = Arc::new(TestClock::at(NOW));
  let warmer = Warmer::new(store.clone(), Policy::builtin().expect("the built-in policy"), clock.clone());
  let base = serve(router(Arc::new(warmer))).await;

  let hold: Arc<Mutex<Option<Hold>>> = Arc::default();
  let held = hold.clone();
  let hook = Router::new().route(
    "/ping",
    post(move || {
      let held = held.lock().expect("the hold").clone();
      async move {
        if let Some(hold) = held {
          hold.arrived.notify_one();
          hold.release.notified().await;
        }
        axum::Json(json!({ "outcome": "warmed" }))
      }
    }),
  );
  let hook_url = format!("{}/ping", serve(hook).await);

  Service {
    _dir: dir,
    store,
    clock,
    base,
    hook_url,
    hold,
    client: reqwest::Client::new(),
  }
}

impl Service {
  /// Asks a route with a raw body, or none; answers the status and the body, null when there is none.
  async fn raw(&self, method: Method, path: &str, body: Option<&str>) -> (u16, Value) {
    let mut request = self.client.request(method, format!("{}{path}", self.base));
    if let Some(body) = body {
      request = request.header("content-type", "application/json").body(body.to_owned());
    }
    let response = request.send().await.expect("the warmer answers");
    let status = response.status().as_u16();
    let text = response.text().await.expect("the answer's body");
    let body = if text.is_empty() {
      Value::Null
    } else {
      serde_json::from_str(&text).expect("a JSON answer")
    };

    (status, body)
  }

  async fn ask(&self, method: Method, path: &str, body: Value) -> (u16, Value) {
    self.raw(method, path, Some(&body.to_string())).await
  }

  async fn get(&self, path: &str) -> (u16, Value) {
    self.raw(Method::GET, path, None).await
  }

  async fn consumer(&self) {
    let (status, _) =
      self.ask(Method::PUT, "/consumers/agency", json!({ "hook_url": self.hook_url })).await;
    assert_eq!(status, 201);
  }

  async fn target(&self, name: &str, body: Value) -> (u16, Value) {
    self.ask(Method::PUT, &format!("/consumers/agency/targets/{name}"), body).await
  }

  async fn listed(&self, name: &str) -> Value {
    let (_, list) = self.get("/targets").await;
    let found = list["targets"].as_array().expect("a list").iter().find(|entry| entry["target"] == name);
    found.cloned().expect("the target is listed")
  }
}

/// A launch line's registration body.
fn line(subject: &str) -> Value {
  json!({
    "kind": "launch_line",
    "label": "A launch line",
    "subject": subject,
    "rank": 50,
    "idle_ms": IDLE,
    "lifetime_ms": 60 * MINUTE,
    "ping_bound_ms": 90_000,
    "warm_by_default": true,
    "failure_touches": true,
  })
}

fn with(mut body: Value, field: &str, value: Value) -> Value {
  body[field] = value;
  body
}

/// Asserts a refusal: its status, its code, and an English sentence beside it and nothing else.
fn refused(answer: &(u16, Value), status: u16, code: &str) {
  let (got, body) = answer;
  assert_eq!(*got, status, "{body}");
  assert_eq!(body["code"], code, "{body}");
  let error = body["error"].as_str().expect("an error sentence");
  assert!(error.ends_with('.') && error.starts_with(char::is_uppercase), "{error}");
  assert_eq!(body.as_object().map(|fields| fields.len()), Some(2), "{body}");
}

#[tokio::test]
async fn health_names_the_service_and_its_version() {
  let service = service().await;

  assert_eq!(
    service.get("/health").await,
    (
      200,
      json!({ "ok": true, "service": "cache-warmer", "version": env!("CARGO_PKG_VERSION") })
    )
  );
}

#[tokio::test]
async fn health_and_every_read_answer_database_when_the_store_cannot_be_read() {
  let service = service().await;
  Migrator::down(service.store.connection(), None).await.expect("the shape goes down");

  refused(&service.get("/health").await, 500, "database");
  refused(&service.get("/round").await, 500, "database");
}

#[tokio::test]
async fn the_round_runs_until_stopped_and_reads_back() {
  let service = service().await;
  assert_eq!(service.get("/round").await, (200, json!({ "stopped": false })));

  assert_eq!(
    service.ask(Method::PUT, "/round", json!({ "stopped": true })).await,
    (200, json!({ "stopped": true }))
  );
  assert_eq!(service.get("/round").await, (200, json!({ "stopped": true })));

  assert_eq!(
    service.ask(Method::PUT, "/round", json!({ "stopped": false })).await,
    (200, json!({ "stopped": false }))
  );
  assert_eq!(service.get("/round").await, (200, json!({ "stopped": false })));
}

#[tokio::test]
async fn a_round_body_that_does_not_read_is_refused_and_moves_nothing() {
  let service = service().await;

  refused(&service.raw(Method::PUT, "/round", Some("not json")).await, 400, "invalid_body");
  refused(
    &service.ask(Method::PUT, "/round", json!({ "stopped": "yes" })).await,
    400,
    "invalid_body",
  );
  refused(&service.ask(Method::PUT, "/round", json!({})).await, 400, "invalid_body");
  refused(&service.raw(Method::PUT, "/round", None).await, 400, "invalid_body");
  assert_eq!(service.get("/round").await, (200, json!({ "stopped": false })));
}

#[tokio::test]
async fn a_subjects_switch_is_thrown_before_any_target_and_unset_with_null() {
  let service = service().await;
  service.consumer().await;

  let path = "/consumers/agency/subjects/worker/switch";
  assert_eq!(
    service.ask(Method::PUT, path, json!({ "on": false })).await,
    (200, json!({ "subject": "worker", "switch": "off", "paused_at": null, "targets": 0 }))
  );
  service.target("line", line("worker")).await;
  assert_eq!(service.listed("line").await["on"], false);
  assert_eq!(service.listed("line").await["due_at"], Value::Null);

  let (status, on) = service.ask(Method::PUT, path, json!({ "on": true })).await;
  assert_eq!((status, &on["switch"], &on["targets"]), (200, &json!("on"), &json!(1)));
  let (status, unset) = service.ask(Method::PUT, path, json!({ "on": null })).await;
  assert_eq!((status, &unset["switch"]), (200, &Value::Null));
  assert_eq!(service.listed("line").await["on"], true);
  let (_, unset) = service.raw(Method::PUT, path, None).await;
  assert_eq!(unset["switch"], Value::Null);

  assert_eq!(
    service.get("/consumers/agency/subjects").await,
    (
      200,
      json!({ "subjects": [{ "subject": "worker", "switch": null, "paused_at": null, "targets": 1 }] })
    )
  );
}

#[tokio::test]
async fn a_pause_holds_until_work_begun_after_it_is_reported() {
  let service = service().await;
  service.consumer().await;
  service.target("line", line("worker")).await;

  service.clock.set(NOW + MINUTE);
  let (status, paused) = service.raw(Method::POST, "/consumers/agency/subjects/worker/pause", None).await;
  assert_eq!((status, &paused["paused_at"]), (200, &json!(NOW + MINUTE)));
  let listed = service.listed("line").await;
  assert_eq!(
    (&listed["paused_at"], &listed["on"], &listed["due_at"]),
    (&json!(NOW + MINUTE), &json!(true), &Value::Null)
  );

  let used = "/consumers/agency/targets/line/used";
  let before = service
    .ask(Method::POST, used, json!({ "at": NOW + 2 * MINUTE, "started_at": NOW }))
    .await;
  assert_eq!(before, (204, Value::Null));
  assert_eq!(service.listed("line").await["paused_at"], json!(NOW + MINUTE));
  assert_eq!(service.listed("line").await["last_used_at"], json!(NOW + 2 * MINUTE));

  service.clock.set(NOW + 3 * MINUTE);
  assert_eq!(service.raw(Method::POST, used, None).await, (204, Value::Null));
  let listed = service.listed("line").await;
  assert_eq!(
    (&listed["paused_at"], &listed["last_used_at"]),
    (&Value::Null, &json!(NOW + 3 * MINUTE))
  );
  assert_eq!(listed["due_at"], json!(NOW + 3 * MINUTE + IDLE));
}

#[tokio::test]
async fn the_switches_refuse_a_consumer_never_registered() {
  let service = service().await;

  refused(
    &service
      .ask(Method::PUT, "/consumers/nobody/subjects/worker/switch", json!({ "on": true }))
      .await,
    404,
    "unknown_consumer",
  );
  refused(
    &service.raw(Method::POST, "/consumers/nobody/subjects/worker/pause", None).await,
    404,
    "unknown_consumer",
  );
  refused(&service.get("/consumers/nobody/subjects").await, 404, "unknown_consumer");
  refused(
    &service.raw(Method::POST, "/consumers/nobody/targets/line/used", None).await,
    404,
    "unknown_target",
  );
}

#[tokio::test]
async fn a_consumer_is_made_then_moved_and_its_hook_must_be_http() {
  let service = service().await;
  let path = "/consumers/agency";

  let made = service.ask(Method::PUT, path, json!({ "hook_url": "http://127.0.0.1:9/ping" })).await;
  assert_eq!(made, (201, json!({ "consumer": "agency", "hook_url": "http://127.0.0.1:9/ping" })));
  let moved = service.ask(Method::PUT, path, json!({ "hook_url": "http://localhost:10/ping" })).await;
  assert_eq!(moved, (200, json!({ "consumer": "agency", "hook_url": "http://localhost:10/ping" })));

  for hook_url in ["https://127.0.0.1:9/ping", "ftp://127.0.0.1/ping", "/ping", "not a url", "http://"] {
    refused(
      &service.ask(Method::PUT, path, json!({ "hook_url": hook_url })).await,
      400,
      "invalid_hook_url",
    );
  }
  refused(&service.ask(Method::PUT, path, json!({})).await, 400, "invalid_body");
  refused(&service.ask(Method::PUT, path, json!({ "hook_url": 7 })).await, 400, "invalid_body");
  assert_eq!(
    service.store.hook_url("agency").await.expect("the hook").as_deref(),
    Some("http://localhost:10/ping")
  );
}

#[tokio::test]
async fn names_are_percent_decoded_and_held_to_their_length() {
  let service = service().await;

  let (status, made) = service
    .ask(Method::PUT, "/consumers/two%20words", json!({ "hook_url": service.hook_url }))
    .await;
  assert_eq!((status, &made["consumer"]), (201, &json!("two words")));

  let long = "n".repeat(257);
  let at_most = "n".repeat(256);
  refused(
    &service
      .ask(Method::PUT, &format!("/consumers/{long}"), json!({ "hook_url": service.hook_url }))
      .await,
    400,
    "invalid_name",
  );
  let (status, _) = service
    .ask(Method::PUT, &format!("/consumers/{at_most}"), json!({ "hook_url": service.hook_url }))
    .await;
  assert_eq!(status, 201);
  refused(
    &service
      .ask(Method::PUT, "/consumers/%FF", json!({ "hook_url": service.hook_url }))
      .await,
    400,
    "invalid_name",
  );

  service.consumer().await;
  refused(
    &service.ask(Method::PUT, "/consumers//targets/line", line("a")).await,
    400,
    "invalid_name",
  );
  refused(&service.target("line", line(&long)).await, 400, "invalid_name");
  refused(&service.target("line", line("")).await, 400, "invalid_name");
  refused(&service.target("line", with(line("a"), "group", json!(""))).await, 400, "invalid_name");
}

#[tokio::test]
async fn a_path_or_method_no_route_serves_is_refused_in_the_one_shape() {
  let service = service().await;

  refused(&service.get("/keepalive").await, 404, "unknown_route");
  refused(&service.get("/consumers/agency/targets").await, 404, "unknown_route");
  refused(&service.raw(Method::DELETE, "/round", None).await, 405, "unknown_route");
}

#[tokio::test]
async fn a_group_needs_its_consumer_and_is_not_retired_while_named() {
  let service = service().await;
  let path = "/consumers/agency/groups/opus";

  refused(&service.ask(Method::PUT, path, json!({ "limit": 2 })).await, 404, "unknown_consumer");
  service.consumer().await;
  assert_eq!(
    service.ask(Method::PUT, path, json!({ "limit": 2 })).await,
    (201, json!({ "consumer": "agency", "group": "opus", "limit": 2 }))
  );
  assert_eq!(
    service.ask(Method::PUT, path, json!({ "limit": 0 })).await,
    (200, json!({ "consumer": "agency", "group": "opus", "limit": 0 }))
  );
  refused(&service.ask(Method::PUT, path, json!({ "limit": -1 })).await, 400, "invalid_body");

  service.target("place", with(line("owner"), "group", json!("opus"))).await;
  refused(&service.raw(Method::DELETE, path, None).await, 409, "group_in_use");
  assert_eq!(
    service.raw(Method::DELETE, "/consumers/agency/targets/place", None).await,
    (204, Value::Null)
  );
  assert_eq!(service.raw(Method::DELETE, path, None).await, (204, Value::Null));
  refused(&service.raw(Method::DELETE, path, None).await, 404, "unknown_group");
}

#[tokio::test]
async fn a_target_is_registered_with_its_defaults_and_listed_whole() {
  let service = service().await;
  service.consumer().await;

  let bare = json!({ "kind": "worker_session", "label": "A worker", "rank": 80, "idle_ms": IDLE, "ping_bound_ms": 120_000 });
  let (status, made) = service.target("opus", bare).await;
  assert_eq!(status, 201);
  assert_eq!(
    made,
    json!({
      "consumer": "agency",
      "target": "opus",
      "kind": "worker_session",
      "label": "A worker",
      "subject": "opus",
      "rank": 80,
      "group": null,
      "idle_ms": IDLE,
      "lifetime_ms": null,
      "margin_ms": 0,
      "ping_bound_ms": 120_000,
      "warm_by_default": false,
      "failure_touches": false,
      "honours_idle_override": true,
      "on": false,
      "paused_at": null,
      "last_used_at": null,
      "last_warmed_at": null,
      "last_failed_at": null,
      "last_attempt": null,
      "due_at": null,
    })
  );
  assert_eq!(service.listed("opus").await, made);
}

#[tokio::test]
async fn restating_a_target_keeps_its_record_and_its_subjects_switch() {
  let service = service().await;
  service.consumer().await;
  let (status, made) =
    service.target("line", with(line("worker"), "last_used_at", json!(NOW - MINUTE))).await;
  assert_eq!((status, &made["due_at"]), (201, &json!(NOW - MINUTE + IDLE)));
  service
    .ask(Method::PUT, "/consumers/agency/subjects/worker/switch", json!({ "on": false }))
    .await;

  let restated = with(with(line("worker"), "label", json!("Restated")), "last_used_at", json!(NOW - IDLE));
  let (status, again) = service.target("line", restated).await;
  assert_eq!(status, 200);
  assert_eq!(
    (&again["label"], &again["last_used_at"], &again["on"]),
    (&json!("Restated"), &json!(NOW - MINUTE), &json!(false))
  );
}

#[tokio::test]
async fn a_registration_out_of_its_shape_is_refused_by_its_code() {
  let service = service().await;
  refused(&service.target("line", line("a")).await, 404, "unknown_consumer");
  service.consumer().await;

  refused(
    &service.target("line", with(line("a"), "kind", json!("Launch Line"))).await,
    400,
    "invalid_kind",
  );
  for (field, value) in [
    ("idle_ms", json!(0)),
    ("ping_bound_ms", json!(0)),
    ("lifetime_ms", json!(-1)),
    ("margin_ms", json!(-1)),
  ] {
    refused(&service.target("line", with(line("a"), field, value)).await, 400, "invalid_clock");
  }
  refused(
    &service.target("line", with(line("a"), "group", json!("missing"))).await,
    409,
    "unknown_group",
  );
  refused(
    &service.target("line", with(line("a"), "rank", json!("first"))).await,
    400,
    "invalid_body",
  );
  refused(
    &service.target("line", with(line("a"), "idle_ms", json!(1.5))).await,
    400,
    "invalid_body",
  );
  let mut missing = line("a");
  missing.as_object_mut().expect("an object").remove("kind");
  refused(&service.target("line", missing).await, 400, "invalid_body");

  let (_, list) = service.get("/targets").await;
  assert_eq!(list, json!({ "targets": [] }));

  // A lifetime no larger than its margin is accepted, and never due.
  let short = with(line("a"), "margin_ms", json!(60 * MINUTE));
  let (status, made) = service.target("short", short).await;
  assert_eq!((status, &made["due_at"]), (201, &Value::Null));
}

#[tokio::test]
async fn retiring_a_target_leaves_its_subject_and_refuses_one_never_registered() {
  let service = service().await;
  service.consumer().await;
  service.target("line", line("worker")).await;

  assert_eq!(
    service.raw(Method::DELETE, "/consumers/agency/targets/line", None).await,
    (204, Value::Null)
  );
  refused(
    &service.raw(Method::DELETE, "/consumers/agency/targets/line", None).await,
    404,
    "unknown_target",
  );
  let (_, subjects) = service.get("/consumers/agency/subjects").await;
  assert_eq!(subjects["subjects"][0]["targets"], 0);
}

#[tokio::test]
async fn targets_list_in_pass_order_and_narrow_by_consumer_and_subject() {
  let service = service().await;
  service.consumer().await;
  service
    .ask(Method::PUT, "/consumers/receiver", json!({ "hook_url": service.hook_url }))
    .await;
  service.ask(Method::PUT, "/consumers/receiver/targets/chat", line("chat")).await;
  service.target("b", line("worker")).await;
  service.target("a", with(line("other"), "rank", json!(10))).await;

  let names = |list: Value| -> Vec<String> {
    list["targets"]
      .as_array()
      .expect("a list")
      .iter()
      .map(|entry| {
        format!(
          "{}/{}",
          entry["consumer"].as_str().unwrap_or_default(),
          entry["target"].as_str().unwrap_or_default()
        )
      })
      .collect()
  };
  assert_eq!(names(service.get("/targets").await.1), vec!["agency/a", "agency/b", "receiver/chat"]);
  assert_eq!(names(service.get("/targets?consumer=receiver").await.1), vec!["receiver/chat"]);
  assert_eq!(names(service.get("/targets?consumer=agency&subject=worker").await.1), vec!["agency/b"]);
  assert_eq!(names(service.get("/targets?subject=two%20words").await.1), Vec::<String>::new());
}

#[tokio::test]
async fn a_pass_runs_on_asking_and_answers_what_it_pinged() {
  let service = service().await;
  service.consumer().await;
  service.target("line", line("worker")).await;

  assert_eq!(
    service.raw(Method::POST, "/pass", None).await,
    (
      200,
      json!({
        "stopped": false,
        "attempts": [{ "consumer": "agency", "target": "line", "outcome": "warmed", "code": null, "reason": null }],
      })
    )
  );
  let listed = service.listed("line").await;
  assert_eq!(
    (&listed["last_warmed_at"], &listed["last_attempt"]["outcome"]),
    (&json!(NOW), &json!("warmed"))
  );

  service.clock.advance(10 * MINUTE);
  assert_eq!(service.ask(Method::POST, "/pass", json!({})).await.1["attempts"], json!([]));
  let overridden = service.ask(Method::POST, "/pass", json!({ "idle_override_ms": 5 * MINUTE })).await;
  assert_eq!(overridden.1["attempts"][0]["target"], "line");

  for stretch in [json!(0), json!(-1)] {
    refused(
      &service.ask(Method::POST, "/pass", json!({ "idle_override_ms": stretch })).await,
      400,
      "invalid_clock",
    );
  }

  service.ask(Method::PUT, "/round", json!({ "stopped": true })).await;
  assert_eq!(
    service.raw(Method::POST, "/pass", None).await,
    (200, json!({ "stopped": true, "attempts": [] }))
  );
}

#[tokio::test]
async fn a_pass_asked_for_while_one_runs_is_refused() {
  let service = Arc::new(service().await);
  service.consumer().await;
  service.target("line", line("worker")).await;
  let hold = Hold {
    arrived: Arc::new(Notify::new()),
    release: Arc::new(Notify::new()),
  };
  *service.hold.lock().expect("the hold") = Some(hold.clone());

  let running = tokio::spawn({
    let service = service.clone();
    async move { service.raw(Method::POST, "/pass", None).await }
  });
  hold.arrived.notified().await;
  refused(&service.raw(Method::POST, "/pass", None).await, 409, "pass_running");

  hold.release.notify_one();
  let (status, first) = running.await.expect("the first pass ends");
  assert_eq!((status, first["attempts"].as_array().map(Vec::len)), (200, Some(1)));
}
