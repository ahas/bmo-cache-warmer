//! The served routes (docs/api.md, "Routes", "Events" and "Codes"), spoken to over loopback as any caller
//! speaks to them: the token, the wire shape of each member, what each route refuses, and the events each
//! change publishes. The pass and the store judge what is warmed; their own tests hold that.

use crate::fixtures;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode as Status;
use axum::routing::post;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::routes::router;
use fixtures::{MINUTE, NOW, TestClock, warmer};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

const TOKEN: &str = "a-test-token";

/// The service on a loopback port, its warmer and the clock it reads.
struct Served {
  _dir: TempDir,
  base: String,
  warmer: Arc<Warmer>,
  clock: Arc<TestClock>,
  http: reqwest::Client,
}

async fn served() -> Served {
  let (dir, warmer, clock) = warmer().await;
  let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let base = format!("http://{}", listener.local_addr().expect("the port's address"));
  let app = router(warmer.clone(), TOKEN);
  tokio::spawn(async move { axum::serve(listener, app).await.expect("the service serves") });

  Served {
    _dir: dir,
    base,
    warmer,
    clock,
    http: reqwest::Client::builder().no_proxy().build().expect("a client"),
  }
}

impl Served {
  /// One request with the token: its status and its body read as JSON, null where it has none.
  async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(TOKEN);
    if let Some(body) = body {
      request = request.json(&body);
    }
    let response = request.send().await.expect("the service answers");
    let status = response.status();
    let text = response.text().await.expect("a body");

    (
      status,
      if text.is_empty() { Value::Null } else { serde_json::from_str(&text).expect("JSON") },
    )
  }

  async fn get(&self, path: &str) -> (StatusCode, Value) {
    self.call(Method::GET, path, None).await
  }

  async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
    self.call(Method::PUT, path, Some(body)).await
  }

  async fn consumer(&self, name: &str) {
    let (status, _) = self
      .put(&format!("/consumers/{name}"), json!({ "url": "http://[::1]:1/ping" }))
      .await;
    assert!(status.is_success());
  }

  async fn target(&self, consumer: &str, name: &str, body: Value) -> Value {
    let (status, target) = self.put(&format!("/consumers/{consumer}/targets/{name}"), body).await;
    assert!(status.is_success(), "{status} {target}");
    target
  }

  /// The kinds of the events kept, in order.
  async fn kinds(&self) -> Vec<String> {
    let events = self.warmer.store().events(0, &[], 1000).await.expect("the window").items;
    events.into_iter().map(|event| event.kind).collect()
  }
}

/// A refusal's status and code.
fn refused(answer: (StatusCode, Value)) -> (u16, String) {
  assert_eq!(answer.1.as_object().map(|body| body.len()), Some(2), "{}", answer.1);
  assert!(answer.1["error"].as_str().is_some_and(|error| error.ends_with('.')), "{}", answer.1);
  (answer.0.as_u16(), answer.1["code"].as_str().unwrap_or_default().to_owned())
}

fn code(status: u16, code: &str) -> (u16, String) {
  (status, code.to_owned())
}

#[tokio::test]
async fn every_route_takes_the_bearer_token_and_no_route_answers_the_rest() {
  let served = served().await;
  let routes = [
    (Method::GET, "/health"),
    (Method::GET, "/events"),
    (Method::GET, "/consumers"),
    (Method::PUT, "/consumers/a"),
    (Method::PUT, "/consumers/a/groups/g"),
    (Method::DELETE, "/consumers/a/groups/g"),
    (Method::GET, "/targets"),
    (Method::PUT, "/consumers/a/targets/t"),
    (Method::DELETE, "/consumers/a/targets/t"),
    (Method::POST, "/consumers/a/targets/t/uses"),
    (Method::GET, "/consumers/a/subjects"),
    (Method::PATCH, "/consumers/a/subjects/s"),
    (Method::GET, "/round"),
    (Method::PATCH, "/round"),
    (Method::POST, "/passes"),
  ];
  for (method, path) in routes {
    let url = format!("{}{path}", served.base);
    let bare = served.http.request(method.clone(), &url).send().await.unwrap();
    assert_eq!(bare.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
    let wrong = served.http.request(method.clone(), &url).bearer_auth("another").send().await.unwrap();
    let body: Value = wrong.json().await.unwrap();
    assert_eq!(body["code"], "unauthorized", "{method} {path}");
  }

  assert_eq!(refused(served.get("/pass").await), code(404, "no-route"));
  assert_eq!(refused(served.call(Method::DELETE, "/round", None).await), code(404, "no-route"));
}

#[tokio::test]
async fn health_answers_ready_and_the_version() {
  let served = served().await;

  assert_eq!(
    served.get("/health").await,
    (
      StatusCode::OK,
      json!({ "ready": true, "reason": null, "version": env!("CARGO_PKG_VERSION") })
    )
  );
}

#[tokio::test]
async fn a_consumer_is_registered_with_an_http_url_and_listed_by_page() {
  let served = served().await;
  let (status, first) = served.put("/consumers/alpha", json!({ "url": "http://[::1]:1/ping" })).await;
  assert_eq!(status, StatusCode::CREATED);
  assert_eq!(first["name"], "alpha");
  assert_eq!(first["createdAt"], NOW);
  served.clock.advance(MINUTE);
  let (status, again) = served.put("/consumers/alpha", json!({ "url": "http://[::1]:2/ping" })).await;
  assert_eq!(status, StatusCode::OK);
  assert_eq!(
    again,
    json!({ "id": first["id"], "name": "alpha", "url": "http://[::1]:2/ping", "createdAt": NOW })
  );

  for url in ["https://127.0.0.1/ping", "/ping", "not a url"] {
    assert_eq!(
      refused(served.put("/consumers/beta", json!({ "url": url })).await),
      code(422, "invalid-url")
    );
  }
  assert_eq!(refused(served.put("/consumers/beta", json!({})).await), code(400, "bad-request"));
  assert_eq!(
    refused(served.put("/consumers/beta", json!({ "url": "http://h/", "x": 1 })).await),
    code(400, "bad-request")
  );
  let long = "n".repeat(257);
  assert_eq!(
    refused(served.put(&format!("/consumers/{long}"), json!({ "url": "http://h/" })).await),
    code(400, "bad-request")
  );

  served.consumer("beta").await;
  served.consumer("gamma").await;
  let (_, page) = served.get("/consumers?limit=2").await;
  assert_eq!(
    page["items"]
      .as_array()
      .unwrap()
      .iter()
      .map(|one| one["name"].clone())
      .collect::<Vec<_>>(),
    ["alpha", "beta"]
  );
  assert_eq!(page["more"], true);
  let (_, rest) = served.get(&format!("/consumers?after={}", page["items"][1]["id"])).await;
  assert_eq!((rest["items"][0]["name"].clone(), rest["more"].clone()), (json!("gamma"), json!(false)));
  for limit in ["0", "1001", "many"] {
    assert_eq!(
      refused(served.get(&format!("/consumers?limit={limit}")).await),
      code(400, "bad-request")
    );
  }
}

#[tokio::test]
async fn a_group_lists_the_targets_naming_it_and_is_not_removed_while_named() {
  let served = served().await;
  assert_eq!(
    refused(served.put("/consumers/alpha/groups/large", json!({ "limit": 1 })).await),
    code(404, "not-found")
  );
  served.consumer("alpha").await;
  let (status, group) = served.put("/consumers/alpha/groups/large", json!({ "limit": 1 })).await;
  assert_eq!(status, StatusCode::CREATED);
  assert_eq!(
    (group["consumerName"].clone(), group["limit"].clone(), group["targetNames"].clone()),
    (json!("alpha"), json!(1), json!([]))
  );
  assert_eq!(
    refused(served.put("/consumers/alpha/groups/large", json!({ "limit": -1 })).await),
    code(400, "bad-request")
  );

  served
    .target("alpha", "seat", json!({ "ttlMs": 3_600_000, "groupName": "large" }))
    .await;
  let (status, group) = served.put("/consumers/alpha/groups/large", json!({ "limit": 2 })).await;
  assert_eq!((status, group["targetNames"].clone()), (StatusCode::OK, json!(["seat"])));
  assert_eq!(
    refused(served.call(Method::DELETE, "/consumers/alpha/groups/large", None).await),
    code(409, "conflict")
  );

  served.call(Method::DELETE, "/consumers/alpha/targets/seat", None).await;
  assert_eq!(
    served.call(Method::DELETE, "/consumers/alpha/groups/large", None).await,
    (StatusCode::NO_CONTENT, Value::Null)
  );
  assert_eq!(
    refused(served.call(Method::DELETE, "/consumers/alpha/groups/large", None).await),
    code(404, "not-found")
  );
}

#[tokio::test]
async fn a_target_is_registered_with_the_policy_filling_what_it_leaves_out() {
  let served = served().await;
  assert_eq!(
    refused(served.put("/consumers/alpha/targets/line", json!({ "ttlMs": 1 })).await),
    code(404, "not-found")
  );
  served.consumer("alpha").await;

  let (status, target) = served.put("/consumers/alpha/targets/line", json!({ "ttlMs": 3_600_000 })).await;
  assert_eq!(status, StatusCode::CREATED);
  let id = target["id"].clone();
  assert_eq!(
    target,
    json!({
      "id": id, "consumerName": "alpha", "name": "line", "kind": null, "label": null, "subjectName": "line",
      "rank": 0, "groupName": null, "ttlMs": 3_600_000, "idleMs": 2_376_000, "marginMs": 0,
      "pingBoundMs": null, "warmByDefault": false, "failureTouches": false, "honoursIdleOverride": true,
      "on": false, "pausedUntil": null, "usedAt": null, "warmedAt": null, "failedAt": null,
      "lastAttempt": null, "nextPingAt": null,
    })
  );

  let restated = json!({
    "ttlMs": 3_600_000, "idleMs": 2_400_000, "marginMs": 60_000, "pingBoundMs": 90_000, "subjectName": "session",
    "rank": 7, "kind": "launch-line", "label": "a launch line", "warmByDefault": true, "failureTouches": true,
    "honoursIdleOverride": false, "usedAt": NOW - MINUTE,
  });
  let (status, target) = served.put("/consumers/alpha/targets/line", restated).await;
  assert_eq!(status, StatusCode::OK);
  assert_eq!(target["id"], id);
  assert_eq!(
    (target["subjectName"].clone(), target["kind"].clone(), target["on"].clone()),
    (json!("session"), json!("launch-line"), json!(true))
  );
  assert_eq!(
    (target["usedAt"].clone(), target["nextPingAt"].clone()),
    (json!(NOW - MINUTE), json!(NOW - MINUTE + 2_400_000))
  );
}

#[tokio::test]
async fn a_target_out_of_its_rules_is_refused_by_the_rule_it_breaks() {
  let served = served().await;
  served.consumer("alpha").await;
  let path = "/consumers/alpha/targets/line";

  for body in [
    json!({ "ttlMs": -1 }),
    json!({ "ttlMs": 1, "marginMs": -1 }),
    json!({ "ttlMs": 1, "idleMs": 0 }),
    json!({ "ttlMs": 1, "pingBoundMs": 0 }),
  ] {
    assert_eq!(refused(served.put(path, body.clone()).await), code(422, "invalid-clock"), "{body}");
  }
  for kind in ["chat_touch", "Touch", "1st", ""] {
    assert_eq!(
      refused(served.put(path, json!({ "ttlMs": 1, "kind": kind })).await),
      code(422, "invalid-kind"),
      "{kind}"
    );
  }
  assert_eq!(
    refused(served.put(path, json!({ "ttlMs": 1, "groupName": "none" })).await),
    code(422, "unknown-group")
  );
  assert_eq!(refused(served.put(path, json!({ "idleMs": 1 })).await), code(400, "bad-request"));
  assert_eq!(
    refused(served.put(path, json!({ "ttlMs": 1, "subjectName": "" })).await),
    code(400, "bad-request")
  );
  // A lifetime no larger than its margin is taken, and never due.
  let never = served
    .target("alpha", "short", json!({ "ttlMs": 5, "marginMs": 5, "warmByDefault": true }))
    .await;
  assert_eq!((never["idleMs"].clone(), never["nextPingAt"].clone()), (json!(1), Value::Null));
}

#[tokio::test]
async fn targets_page_by_id_and_narrow_by_consumer_and_subject() {
  let served = served().await;
  served.consumer("alpha").await;
  served.consumer("beta").await;
  served.target("beta", "touch", json!({ "ttlMs": 1 })).await;
  served.target("alpha", "b", json!({ "ttlMs": 1, "subjectName": "session" })).await;
  served.target("alpha", "a", json!({ "ttlMs": 1 })).await;

  let names = |page: &Value| -> Vec<String> {
    page["items"]
      .as_array()
      .unwrap()
      .iter()
      .map(|one| format!("{}/{}", one["consumerName"].as_str().unwrap(), one["name"].as_str().unwrap()))
      .collect()
  };
  let (_, first) = served.get("/targets?limit=2").await;
  assert_eq!(
    (names(&first), first["more"].clone()),
    (vec!["beta/touch".to_owned(), "alpha/b".to_owned()], json!(true))
  );
  let (_, rest) = served.get(&format!("/targets?limit=2&after={}", first["items"][1]["id"])).await;
  assert_eq!((names(&rest), rest["more"].clone()), (vec!["alpha/a".to_owned()], json!(false)));
  assert_eq!(names(&served.get("/targets?consumerName=alpha").await.1), ["alpha/b", "alpha/a"]);
  assert_eq!(names(&served.get("/targets?subjectName=session").await.1), ["alpha/b"]);
  assert_eq!(refused(served.get("/targets?after=soon").await), code(400, "bad-request"));
}

#[tokio::test]
async fn a_target_is_retired_once() {
  let served = served().await;
  served.consumer("alpha").await;
  let target = served.target("alpha", "line", json!({ "ttlMs": 1 })).await;

  assert_eq!(
    served.call(Method::DELETE, "/consumers/alpha/targets/line", None).await,
    (StatusCode::NO_CONTENT, Value::Null)
  );
  assert_eq!(
    refused(served.call(Method::DELETE, "/consumers/alpha/targets/line", None).await),
    code(404, "not-found")
  );
  let events = served.warmer.store().events(0, &["target.deleted".to_owned()], 10).await.unwrap().items;
  let data: Value = serde_json::from_str(&events[0].data).unwrap();
  assert_eq!(data, json!({ "id": target["id"], "consumerName": "alpha", "name": "line" }));
}

#[tokio::test]
async fn a_use_is_recorded_now_or_when_it_says_and_lifts_only_a_pause_set_before_its_work() {
  let served = served().await;
  served.consumer("alpha").await;
  served
    .target("alpha", "line", json!({ "ttlMs": 3_600_000, "subjectName": "session" }))
    .await;
  let uses = "/consumers/alpha/targets/line/uses";

  let (status, used) = served.call(Method::POST, uses, None).await;
  assert_eq!(status, StatusCode::CREATED);
  assert_eq!(
    used,
    json!({ "consumerName": "alpha", "targetName": "line", "at": NOW, "startedAt": NOW })
  );
  assert_eq!(
    refused(served.call(Method::POST, "/consumers/alpha/targets/none/uses", None).await),
    code(404, "not-found")
  );

  let pause = json!({ "pausedUntil": NOW + 60 * MINUTE });
  served.call(Method::PATCH, "/consumers/alpha/subjects/session", Some(pause)).await;
  served
    .call(Method::POST, uses, Some(json!({ "at": NOW + MINUTE, "startedAt": NOW - MINUTE })))
    .await;
  let (_, page) = served.get("/consumers/alpha/subjects").await;
  assert_eq!(page["items"][0]["pausedUntil"], NOW + 60 * MINUTE);

  served
    .call(Method::POST, uses, Some(json!({ "at": NOW + 2 * MINUTE, "startedAt": NOW + MINUTE })))
    .await;
  let (_, page) = served.get("/consumers/alpha/subjects").await;
  assert_eq!(
    (page["items"][0]["pausedAt"].clone(), page["items"][0]["pausedUntil"].clone()),
    (Value::Null, Value::Null)
  );
  let (_, targets) = served.get("/targets").await;
  assert_eq!(targets["items"][0]["usedAt"], NOW + 2 * MINUTE);
  assert_eq!(
    served.kinds().await,
    [
      "target.changed",
      "target.changed",
      "subject.changed",
      "target.changed",
      "target.changed",
      "subject.changed"
    ]
  );
}

#[tokio::test]
async fn a_subject_is_switched_and_paused_by_merge_patch() {
  let served = served().await;
  let path = "/consumers/alpha/subjects/session";
  assert_eq!(
    refused(served.call(Method::PATCH, path, Some(json!({ "on": true }))).await),
    code(404, "not-found")
  );
  served.consumer("alpha").await;

  // A subject no target names yet is made by its first change.
  let (status, subject) = served.call(Method::PATCH, path, Some(json!({ "on": false }))).await;
  assert_eq!(status, StatusCode::OK);
  let id = subject["id"].clone();
  assert_eq!(
    subject,
    json!({ "id": id, "consumerName": "alpha", "name": "session", "on": false, "pausedAt": null, "pausedUntil": null, "targetCount": 0 })
  );
  served
    .target("alpha", "line", json!({ "ttlMs": 1, "subjectName": "session", "warmByDefault": true }))
    .await;

  let (_, paused) = served.call(Method::PATCH, path, Some(json!({ "pausedUntil": NOW + MINUTE }))).await;
  assert_eq!(
    (
      paused["on"].clone(),
      paused["pausedAt"].clone(),
      paused["pausedUntil"].clone(),
      paused["targetCount"].clone()
    ),
    (json!(false), json!(NOW), json!(NOW + MINUTE), json!(1))
  );
  let (_, unset) = served.call(Method::PATCH, path, Some(json!({ "on": null }))).await;
  assert_eq!((unset["on"].clone(), unset["pausedUntil"].clone()), (Value::Null, json!(NOW + MINUTE)));
  let (_, lifted) = served.call(Method::PATCH, path, Some(json!({ "pausedUntil": null }))).await;
  assert_eq!(lifted["pausedUntil"], Value::Null);
  let (_, kept) = served.call(Method::PATCH, path, Some(json!({}))).await;
  assert_eq!(kept, lifted);

  assert_eq!(
    refused(served.call(Method::PATCH, path, Some(json!({ "pausedUntil": NOW }))).await),
    code(422, "invalid-clock")
  );
  assert_eq!(
    refused(served.call(Method::PATCH, path, Some(json!({ "on": "yes" }))).await),
    code(400, "bad-request")
  );
  assert_eq!(refused(served.get("/consumers/nobody/subjects").await), code(404, "not-found"));

  // A pause whose time has passed no longer stands.
  served.call(Method::PATCH, path, Some(json!({ "pausedUntil": NOW + MINUTE }))).await;
  served.clock.advance(MINUTE);
  let (_, page) = served.get("/consumers/alpha/subjects").await;
  assert_eq!(
    (page["items"][0]["pausedUntil"].clone(), page["more"].clone()),
    (Value::Null, json!(false))
  );
}

#[tokio::test]
async fn the_round_answers_its_interval_and_next_time_and_changes_by_merge_patch() {
  let served = served().await;

  let (status, round) = served.get("/round").await;
  assert_eq!(status, StatusCode::OK);
  assert_eq!((round["intervalMs"].clone(), round["running"].clone()), (json!(60_000), json!(true)));
  let next = round["nextAt"].as_i64().unwrap();
  assert!((NOW..=NOW + 30_000).contains(&next), "{round}");

  let (_, round) = served
    .call(Method::PATCH, "/round", Some(json!({ "intervalMs": 5_000, "running": false })))
    .await;
  assert_eq!((round["intervalMs"].clone(), round["running"].clone()), (json!(5_000), json!(false)));
  let (_, round) = served.call(Method::PATCH, "/round", Some(json!({ "running": true }))).await;
  assert_eq!((round["intervalMs"].clone(), round["running"].clone()), (json!(5_000), json!(true)));
  let (_, round) = served.call(Method::PATCH, "/round", Some(json!({ "intervalMs": null }))).await;
  assert_eq!(round["intervalMs"], 60_000);
  assert_eq!(served.warmer.beat().interval(), Duration::from_secs(60));

  for body in [json!({ "intervalMs": 0 }), json!({ "intervalMs": -5 })] {
    assert_eq!(
      refused(served.call(Method::PATCH, "/round", Some(body)).await),
      code(422, "invalid-clock")
    );
  }
  assert_eq!(
    refused(served.call(Method::PATCH, "/round", Some(json!({ "stopped": true }))).await),
    code(400, "bad-request")
  );
  assert_eq!(served.kinds().await, ["round.changed", "round.changed", "round.changed"]);
}

#[tokio::test]
async fn a_pass_asked_for_runs_now_beside_one_already_running() {
  let served = served().await;
  let held = Arc::new(tokio::sync::Notify::new());
  let arrived = Arc::new(tokio::sync::Notify::new());
  let hook = Router::new().route(
    "/ping",
    post({
      let (held, arrived) = (held.clone(), arrived.clone());
      move || async move {
        arrived.notify_one();
        held.notified().await;
        Status::NO_CONTENT
      }
    }),
  );
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let url = format!("http://{}/ping", listener.local_addr().unwrap());
  tokio::spawn(async move { axum::serve(listener, hook).await.unwrap() });
  served.put("/consumers/alpha", json!({ "url": url })).await;
  served
    .target("alpha", "line", json!({ "ttlMs": 3_600_000, "warmByDefault": true }))
    .await;

  let first = tokio::spawn({
    let (http, base) = (served.http.clone(), served.base.clone());
    async move { http.post(format!("{base}/passes")).bearer_auth(TOKEN).send().await.unwrap() }
  });
  arrived.notified().await;
  // The pass beside the first runs and ends at once: the only consumer's line is the first pass's.
  let (status, beside) = served.call(Method::POST, "/passes", None).await;
  assert_eq!(status, StatusCode::CREATED);
  assert_eq!(beside["pingedCount"], 0);
  held.notify_one();
  let answer = first.await.unwrap();
  assert_eq!(answer.status(), StatusCode::CREATED);
  let pass: Value = answer.json().await.unwrap();
  assert_eq!(
    pass,
    json!({
      "at": NOW, "pingedCount": 1, "stopped": false,
      "attempts": [{ "consumerName": "alpha", "targetName": "line", "at": NOW, "outcome": "warmed", "code": null, "reason": null }],
    })
  );
  assert_eq!(
    refused(served.call(Method::POST, "/passes", Some(json!({ "idleOverrideMs": 0 }))).await),
    code(422, "invalid-clock")
  );
}

#[tokio::test]
async fn events_are_read_as_a_page_narrowed_and_resumed_and_a_cursor_past_the_window_is_gone() {
  let served = served().await;
  served.consumer("alpha").await;
  served.target("alpha", "line", json!({ "ttlMs": 1 })).await;
  served.call(Method::PATCH, "/round", Some(json!({ "running": false }))).await;
  served.call(Method::POST, "/passes", None).await;
  let page = |query: &str| {
    let request = served
      .http
      .get(format!("{}/events{query}", served.base))
      .bearer_auth(TOKEN)
      .header("accept", "application/json");
    async move { request.send().await.unwrap().json::<Value>().await.unwrap() }
  };

  let all = page("").await;
  let kinds: Vec<&str> = all["items"]
    .as_array()
    .unwrap()
    .iter()
    .map(|event| event["kind"].as_str().unwrap())
    .collect();
  assert_eq!(kinds, ["target.changed", "round.changed", "pass.created"]);
  let first = &all["items"][0];
  assert_eq!((first["at"].clone(), first["data"]["name"].clone()), (json!(NOW), json!("line")));

  let narrowed = page("?kinds=pass.created,round.changed&limit=1").await;
  assert_eq!(
    (narrowed["items"][0]["kind"].clone(), narrowed["more"].clone()),
    (json!("round.changed"), json!(true))
  );
  let resumed = page(&format!("?after={}", all["items"][1]["id"])).await;
  assert_eq!(resumed["items"].as_array().unwrap().len(), 1);

  served.warmer.store().prune_events(NOW + 1).await.unwrap();
  let gone = served
    .http
    .get(format!("{}/events?after=0", served.base))
    .bearer_auth(TOKEN)
    .send()
    .await
    .unwrap();
  assert_eq!(gone.status(), StatusCode::GONE);
  let last = all["items"][2]["id"].as_i64().unwrap();
  let current = page(&format!("?after={last}")).await;
  assert_eq!(current, json!({ "items": [], "more": false }));

  // A reader with no cursor starts at the window, whatever was pruned before it.
  served.call(Method::PATCH, "/round", Some(json!({ "running": true }))).await;
  let fresh = page("").await;
  assert_eq!(fresh["items"].as_array().unwrap().len(), 1);
  assert_eq!(fresh["items"][0]["id"], last + 1);
}

/// Reads a stream's body into `text` until it holds `wanted`, each chunk waited for a bounded time.
async fn read_until(response: &mut reqwest::Response, text: &mut String, wanted: &str) {
  while !text.contains(wanted) {
    let chunk = tokio::time::timeout(Duration::from_secs(10), response.chunk())
      .await
      .expect("the stream speaks in time");
    text.push_str(&String::from_utf8_lossy(&chunk.expect("a chunk reads").expect("the stream stays open")));
  }
}

#[tokio::test]
async fn the_stream_sends_what_stands_past_its_cursor_then_each_event_as_it_is_published() {
  let served = served().await;
  served.consumer("alpha").await;
  served.target("alpha", "line", json!({ "ttlMs": 1 })).await;

  let mut response = served
    .http
    .get(format!("{}/events?kinds=target.changed,target.deleted", served.base))
    .bearer_auth(TOKEN)
    .send()
    .await
    .unwrap();
  assert_eq!(response.status(), StatusCode::OK);
  assert!(response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
  let mut text = String::new();

  read_until(&mut response, &mut text, "event: target.changed").await;
  served.call(Method::PATCH, "/round", Some(json!({ "running": false }))).await;
  served.call(Method::DELETE, "/consumers/alpha/targets/line", None).await;
  read_until(&mut response, &mut text, "event: target.deleted").await;
  assert!(!text.contains("round.changed"), "{text}");
  let data = text.lines().filter_map(|line| line.strip_prefix("data: ")).next_back().unwrap();
  let event: Value = serde_json::from_str(data).unwrap();
  assert_eq!(
    (event["kind"].clone(), event["data"]["name"].clone()),
    (json!("target.deleted"), json!("line"))
  );
  assert!(text.contains(&format!("id: {}", event["id"])), "{text}");
}
