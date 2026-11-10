//! The pass (docs/warming.md, "The pass" and "What an answer leaves behind"), run against a fake consumer
//! on loopback whose hook answers as each test scripts it, on a clock the test moves.

use crate::fixtures;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::store::{Attempt, Outcome, Pause, Registration, RoundChange, Store, SubjectChange};
use bmo_cache_warmer::wire::Pass;
use fixtures::{IDLE, MINUTE, NOW, TestClock, line, place, store};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

const DAY: i64 = 24 * 60 * MINUTE;

/// How the fake hook answers one target's ping.
#[derive(Clone)]
enum Reply {
  Answer(Value),
  Status(u16),
  Body(&'static str),
  /// Sends the ping on to a route that answers warmed.
  Redirect,
  /// Says it has the ping, then answers warmed once released.
  Held {
    arrived: Arc<Notify>,
    release: Arc<Notify>,
  },
  Never,
}

/// What the fake consumer does before it answers.
#[derive(Clone)]
enum Act {
  StopRound,
  SwitchOff(&'static str),
  Retire(&'static str),
  Advance(i64),
}

struct Consumer {
  store: Store,
  clock: Arc<TestClock>,
  script: Mutex<HashMap<String, (Vec<Act>, Reply)>>,
  heard: Mutex<Vec<Value>>,
}

async fn hook(State(consumer): State<Arc<Consumer>>, Json(ping): Json<Value>) -> Response {
  let target = ping["targetName"].as_str().unwrap_or_default().to_owned();
  consumer.heard.lock().expect("the log of pings").push(ping);
  let scripted = consumer.script.lock().expect("the script").get(&target).cloned();
  let (acts, reply) = scripted.unwrap_or_else(|| (Vec::new(), Reply::Status(204)));

  for act in acts {
    match act {
      Act::StopRound => stop(&consumer.store).await,
      Act::SwitchOff(subject) => {
        let off = SubjectChange {
          switch: Some(Some(false)),
          ..SubjectChange::default()
        };
        consumer.store.change_subject("alpha", subject, off).await.expect("the switch");
      }
      Act::Retire(target) => {
        consumer.store.delete_target("alpha", target).await.expect("the target retires");
      }
      Act::Advance(by) => consumer.clock.advance(by),
    }
  }

  match reply {
    Reply::Answer(answer) => Json(answer).into_response(),
    Reply::Status(status) => StatusCode::from_u16(status).expect("a status").into_response(),
    Reply::Body(body) => body.into_response(),
    Reply::Redirect => (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, "/warm")]).into_response(),
    Reply::Held { arrived, release } => {
      arrived.notify_one();
      release.notified().await;
      StatusCode::NO_CONTENT.into_response()
    }
    Reply::Never => std::future::pending().await,
  }
}

async fn stop(store: &Store) {
  let stopped = RoundChange {
    stopped: Some(true),
    ..RoundChange::default()
  };
  store.change_round(stopped).await.expect("the round stops");
}

/// A store, a clock at `NOW`, and the consumer `alpha` whose hook is the fake one.
struct Bench {
  _dir: TempDir,
  store: Store,
  clock: Arc<TestClock>,
  consumer: Arc<Consumer>,
  hook_url: String,
}

async fn bench() -> Bench {
  let (dir, store) = store().await;
  let clock = Arc::new(TestClock::at(NOW));
  let consumer = Arc::new(Consumer {
    store: store.clone(),
    clock: clock.clone(),
    script: Mutex::new(HashMap::new()),
    heard: Mutex::new(Vec::new()),
  });

  let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let hook_url = format!("http://{}/ping", listener.local_addr().expect("the port's address"));
  let app = Router::new()
    .route("/ping", post(hook))
    .route("/warm", any(|| async { StatusCode::NO_CONTENT }))
    .with_state(consumer.clone());
  tokio::spawn(async move { axum::serve(listener, app).await.expect("the fake consumer serves") });
  store.put_consumer("alpha", &hook_url, NOW).await.expect("the consumer");

  Bench {
    _dir: dir,
    store,
    clock,
    consumer,
    hook_url,
  }
}

impl Bench {
  async fn warmer(&self) -> Warmer {
    self.warmer_with(Policy::builtin().expect("the built-in policy")).await
  }

  async fn warmer_with(&self, policy: Policy) -> Warmer {
    Warmer::open(self.store.clone(), policy, self.clock.clone())
      .await
      .expect("the warmer opens")
  }

  async fn register(&self, target: &str, registration: Registration) {
    self.store.put_target("alpha", target, &registration).await.expect("a target");
  }

  fn script(&self, target: &str, acts: Vec<Act>, reply: Reply) {
    self.consumer.script.lock().expect("the script").insert(target.into(), (acts, reply));
  }

  fn answer(&self, target: &str, answer: Value) {
    self.script(target, Vec::new(), Reply::Answer(answer));
  }

  /// The targets the hook heard, in order, and forgets them.
  fn heard(&self) -> Vec<String> {
    let mut heard = self.consumer.heard.lock().expect("the log of pings");
    heard
      .drain(..)
      .map(|ping| ping["targetName"].as_str().unwrap_or_default().to_owned())
      .collect()
  }

  async fn last(&self, target: &str) -> Attempt {
    let target = self.store.target("alpha", target).await.expect("the target").expect("it stands");
    target.last_attempt.expect("an attempt")
  }
}

fn used(mut registration: Registration, at: i64) -> Registration {
  registration.last_used_at = Some(at);
  registration
}

fn pinged(pass: &Pass) -> Vec<&str> {
  pass.attempts.iter().map(|pinged| pinged.target_name.as_str()).collect()
}

fn failed(code: &str) -> (Outcome, Option<&str>) {
  (Outcome::Failed, Some(code))
}

fn outcome_of(attempt: &Attempt) -> (Outcome, Option<&str>) {
  (attempt.outcome, attempt.code.as_deref())
}

#[tokio::test]
async fn a_stopped_round_pings_nothing_and_leaves_records_still() {
  let bench = bench().await;
  bench.register("line", line("session")).await;
  stop(&bench.store).await;

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert!(pass.stopped);
  assert_eq!(pass.pinged_count, 0);
  assert!(bench.heard().is_empty());
  assert_eq!(bench.store.target("alpha", "line").await.unwrap().unwrap().last_attempt, None);
}

#[tokio::test]
async fn due_targets_are_pinged_in_rank_order_with_their_ping() {
  let bench = bench().await;
  bench.store.put_group("alpha", "large", 1).await.expect("a group");
  bench.register("b", line("session")).await;
  bench.register("a", line("other")).await;
  bench.register("seat", place("holder", "large")).await;
  bench.register("fresh", used(line("fresh"), NOW - MINUTE)).await;
  let mut off = line("quiet");
  off.warm_by_default = None;
  bench.register("off", off).await;
  bench.register("used", used(line("used"), NOW - IDLE - MINUTE)).await;

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert!(!pass.stopped);
  assert_eq!(pinged(&pass), vec!["seat", "a", "b", "used"]);
  assert_eq!(pass.pinged_count, 4);
  assert!(pass.attempts.iter().all(|pinged| pinged.at == NOW && pinged.code.is_none()));

  let heard = bench.consumer.heard.lock().expect("the log of pings").clone();
  assert_eq!(
    heard[0],
    json!({
      "consumerName": "alpha",
      "targetName": "seat",
      "subjectName": "holder",
      "kind": "pool-place",
      "dueAt": NOW,
      "boundMs": 120_000,
    })
  );
  // A target used before is due an idle stretch after its use.
  assert_eq!(heard[3]["dueAt"], NOW - MINUTE);
}

#[tokio::test]
async fn a_ping_hands_back_a_stretch_only_where_one_was_registered() {
  let bench = bench().await;
  let mut bare = line("session");
  bare.ping_bound_ms = None;
  bench.register("bare", bare).await;

  bench.warmer().await.pass(None).await.expect("a pass");
  let heard = bench.consumer.heard.lock().expect("the log of pings").clone();
  assert_eq!(heard.len(), 1);
  assert!(heard[0].get("boundMs").is_none(), "{}", heard[0]);
}

#[tokio::test]
async fn only_a_warmed_answer_restarts_the_clock() {
  let bench = bench().await;
  bench.register("warmed", line("a")).await;
  bench.register("busy", line("b")).await;
  bench.register("skipped", line("c")).await;
  bench.register("said", line("d")).await;
  bench.answer("busy", json!({ "outcome": "busy", "code": "ignored", "reason": "Ignored." }));
  bench.answer("skipped", json!({ "outcome": "skipped", "code": "provider-off" }));
  bench.answer("said", json!({ "outcome": "warmed" }));
  let warmer = bench.warmer().await;

  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["busy", "said", "skipped", "warmed"]);
  assert_eq!(
    bench.last("busy").await,
    Attempt {
      at: NOW,
      outcome: Outcome::Busy,
      code: None,
      reason: None,
    }
  );
  assert_eq!(outcome_of(&bench.last("skipped").await), (Outcome::Skipped, Some("provider-off")));
  // A 204 and a 200 saying warmed are the same answer.
  for name in ["warmed", "said"] {
    let warmed = bench.store.target("alpha", name).await.expect("the target").expect("it stands");
    assert_eq!(warmed.record.last_warmed_at, Some(NOW), "{name}");
  }

  bench.clock.advance(MINUTE);
  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["busy", "skipped"]);

  bench.clock.advance(IDLE);
  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["busy", "said", "skipped", "warmed"]);
}

#[tokio::test]
async fn a_failure_touches_only_where_registered() {
  let bench = bench().await;
  bench.store.put_group("alpha", "large", 5).await.expect("a group");
  bench.register("line", line("session")).await;
  bench.register("seat", place("holder", "large")).await;
  let refusal =
    json!({ "outcome": "failed", "code": "launch-failed", "reason": "The line did not launch." });
  bench.answer("line", refusal.clone());
  bench.answer("seat", refusal);
  let warmer = bench.warmer().await;

  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["seat", "line"]);
  for name in ["line", "seat"] {
    let target = bench.store.target("alpha", name).await.expect("the target").expect("it stands");
    assert_eq!(target.record.last_failed_at, Some(NOW));
    assert_eq!(target.record.last_warmed_at, None);
    let attempt = target.last_attempt.expect("an attempt");
    assert_eq!(outcome_of(&attempt), failed("launch-failed"));
    assert_eq!(attempt.reason.as_deref(), Some("The line did not launch."));
  }

  bench.clock.advance(MINUTE);
  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["seat"]);
}

#[tokio::test]
async fn a_hook_that_answers_wrongly_is_recorded_as_failed_with_the_warmers_code() {
  let bench = bench().await;
  let cases: [(&str, Reply, &str); 8] = [
    ("status", Reply::Status(500), "hook-status"),
    ("created", Reply::Status(201), "hook-status"),
    ("redirected", Reply::Redirect, "hook-status"),
    ("garbage", Reply::Body("not an answer"), "hook-answer"),
    ("unknown", Reply::Answer(json!({ "outcome": "cooled" })), "hook-answer"),
    ("codeless", Reply::Answer(json!({ "outcome": "skipped" })), "hook-answer"),
    (
      "reasonless",
      Reply::Answer(json!({ "outcome": "failed", "code": "broke" })),
      "hook-answer",
    ),
    (
      "misshapen",
      Reply::Answer(json!({ "outcome": "failed", "code": "broke_down", "reason": "It broke." })),
      "hook-answer",
    ),
  ];
  for (name, reply, _) in &cases {
    bench.register(name, line(name)).await;
    bench.script(name, Vec::new(), reply.clone());
  }

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert_eq!(pass.attempts.len(), cases.len());
  for (name, _, code) in &cases {
    let attempt = bench.last(name).await;
    assert_eq!(outcome_of(&attempt), failed(code), "{name}");
    let reason = attempt.reason.expect("the warmer's reason");
    assert!(reason.starts_with("The hook") && reason.ends_with('.'), "{name}: {reason}");
  }
}

#[tokio::test]
async fn a_hook_nobody_listens_on_is_unreachable() {
  let bench = bench().await;
  let closed = TcpListener::bind("[::1]:0").await.expect("a loopback port");
  let hook_url = format!("http://{}/ping", closed.local_addr().expect("the port's address"));
  drop(closed);
  bench.store.put_consumer("gone", &hook_url, NOW).await.expect("a consumer");
  bench.store.put_target("gone", "line", &line("session")).await.expect("a target");

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert_eq!(pinged(&pass), vec!["line"]);
  assert_eq!(pass.attempts[0].code.as_deref(), Some("hook-unreachable"));
  assert_eq!(pass.attempts[0].consumer_name, "gone");
}

/// Two notices for one held reply: the ping has arrived, and the hook may answer.
fn held() -> (Arc<Notify>, Arc<Notify>, Reply) {
  let (arrived, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
  let reply = Reply::Held {
    arrived: arrived.clone(),
    release: release.clone(),
  };
  (arrived, release, reply)
}

#[tokio::test]
async fn a_hook_that_never_answers_holds_its_own_line_and_no_other() {
  let bench = bench().await;
  bench.store.put_consumer("beta", &bench.hook_url, NOW).await.expect("a second consumer");
  bench.register("silent", line("a")).await;
  bench.register("tail", line("c")).await;
  bench.store.put_target("beta", "then", &line("b")).await.expect("a target of beta");
  bench.script("silent", Vec::new(), Reply::Never);
  let (arrived, release, reply) = held();
  bench.script("then", Vec::new(), reply);
  let warmer = Arc::new(bench.warmer().await);
  let mut published = warmer.published();

  let pass = tokio::spawn({
    let warmer = warmer.clone();
    async move { warmer.pass(None).await }
  });
  // Beta's ping arrives while alpha's first is still out, and is answered.
  arrived.notified().await;
  release.notify_one();
  let then = loop {
    let target = bench.store.target("beta", "then").await.expect("the target").expect("it stands");
    if let Some(attempt) = target.last_attempt {
      break attempt;
    }
    published.changed().await.expect("the warmer stands");
  };
  assert_eq!(then.outcome, Outcome::Warmed);

  // Alpha's line waits on its silent hook: its next target has not been pinged, and the pass has not ended.
  assert!(!bench.heard().iter().any(|name| name == "tail"));
  assert!(!pass.is_finished());
  pass.abort();
}

#[tokio::test]
async fn an_attempt_is_timed_when_its_answer_arrives() {
  let bench = bench().await;
  bench.register("line", line("session")).await;
  bench.script("line", vec![Act::Advance(5_000)], Reply::Status(204));

  bench.warmer().await.pass(None).await.expect("a pass");
  let target = bench.store.target("alpha", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_warmed_at, Some(NOW + 5_000));
  assert_eq!(target.last_attempt.map(|attempt| attempt.at), Some(NOW + 5_000));
}

#[tokio::test]
async fn a_target_turned_off_or_retired_mid_pass_is_passed_over_and_not_replaced() {
  let bench = bench().await;
  bench.store.put_group("alpha", "large", 1).await.expect("a group");
  let mut lead = line("lead");
  lead.rank = 1;
  bench.register("lead", lead).await;
  bench.register("oldest", used(place("holder", "large"), NOW - 45 * MINUTE)).await;
  bench.register("spare", used(place("spare", "large"), NOW - 41 * MINUTE)).await;
  let mut gone = line("gone");
  gone.rank = 60;
  bench.register("gone", gone).await;
  let mut tail = line("tail");
  tail.rank = 70;
  bench.register("tail", tail).await;
  bench.script("lead", vec![Act::SwitchOff("holder"), Act::Retire("gone")], Reply::Status(204));

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert!(!pass.stopped);
  assert_eq!(pinged(&pass), vec!["lead", "tail"]);
  assert_eq!(bench.heard(), vec!["lead", "tail"]);
  let oldest = bench.store.target("alpha", "oldest").await.expect("the target").expect("it stands");
  assert_eq!(oldest.last_attempt, None);
}

#[tokio::test]
async fn a_round_stopped_mid_pass_ends_the_pass() {
  let bench = bench().await;
  bench.register("a", line("a")).await;
  bench.register("b", line("b")).await;
  bench.script("a", vec![Act::StopRound], Reply::Status(204));

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert!(pass.stopped);
  assert_eq!(pinged(&pass), vec!["a"]);
  assert_eq!(bench.heard(), vec!["a"]);
}

#[tokio::test]
async fn a_pause_holds_until_its_end_and_then_the_target_warms_again() {
  let bench = bench().await;
  bench.register("line", line("session")).await;
  let pause = SubjectChange {
    pause: Some(Some(Pause {
      at: NOW,
      until: NOW + MINUTE,
    })),
    ..SubjectChange::default()
  };
  bench.store.change_subject("alpha", "session", pause).await.expect("a pause");
  let warmer = bench.warmer().await;

  assert!(warmer.pass(None).await.expect("a pass").attempts.is_empty());
  bench.clock.advance(MINUTE);
  assert_eq!(pinged(&warmer.pass(None).await.expect("a pass")), vec!["line"]);
}

#[tokio::test]
async fn a_group_pings_its_oldest_up_to_its_limit() {
  let bench = bench().await;
  bench.store.put_group("alpha", "large", 2).await.expect("a group");
  bench.register("p1", used(place("one", "large"), NOW - 45 * MINUTE)).await;
  bench.register("p2", used(place("two", "large"), NOW - 50 * MINUTE)).await;
  bench.register("p3", place("three", "large")).await;

  let pass = bench.warmer().await.pass(None).await.expect("a pass");
  assert_eq!(pinged(&pass), vec!["p3", "p2"]);
}

#[tokio::test]
async fn an_idle_override_reaches_only_targets_that_honour_it() {
  let bench = bench().await;
  bench.store.put_group("alpha", "large", 5).await.expect("a group");
  bench.register("line", used(line("session"), NOW - 10 * MINUTE)).await;
  bench.register("seat", used(place("holder", "large"), NOW - 10 * MINUTE)).await;
  let warmer = bench.warmer().await;

  assert!(warmer.pass(None).await.expect("a pass").attempts.is_empty());
  let pass = warmer.pass(Some(5 * MINUTE)).await.expect("a pass");
  assert_eq!(pinged(&pass), vec!["line"]);
}

#[tokio::test]
async fn a_long_reason_is_cut_to_the_policys_units() {
  let bench = bench().await;
  bench.register("line", line("session")).await;
  bench.answer("line", json!({ "outcome": "failed", "reason": "The line did not launch." }));
  let mut policy = Policy::builtin().expect("the built-in policy");
  policy.reason_units = 8;

  let pass = bench.warmer_with(policy).await.pass(None).await.expect("a pass");
  assert_eq!(pass.attempts[0].reason.as_deref(), Some("The line"));
  assert_eq!(bench.last("line").await.reason.as_deref(), Some("The line"));
  assert_eq!(bench.last("line").await.code, None);
}

#[tokio::test]
async fn a_pass_publishes_each_attempt_and_itself() {
  let bench = bench().await;
  bench.register("a", line("a")).await;
  bench.register("b", line("b")).await;
  let warmer = bench.warmer().await;
  let mut published = warmer.published();

  let pass = warmer.pass(None).await.expect("a pass");
  let events = bench.store.events(0, &[], 10).await.expect("the window").items;
  let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
  assert_eq!(kinds, ["target.changed", "target.changed", "pass.created"]);
  let first: Value = serde_json::from_str(&events[0].data).unwrap();
  assert_eq!((first["name"].as_str(), first["warmedAt"].as_i64()), (Some("a"), Some(NOW)));
  let last: Value = serde_json::from_str(&events[2].data).unwrap();
  assert_eq!(last, serde_json::to_value(&pass).unwrap());
  assert!(published.has_changed().unwrap());
  assert_eq!(*published.borrow_and_update(), events[2].id);
}

#[tokio::test]
async fn a_pass_ends_by_pruning_the_log_and_the_window_to_their_days_even_when_stopped() {
  for stopped in [false, true] {
    let bench = bench().await;
    bench.register("line", line("session")).await;
    let old = Attempt {
      at: NOW - 2 * DAY,
      outcome: Outcome::Busy,
      code: None,
      reason: None,
    };
    bench.store.record_attempt("alpha", "line", &old).await.expect("an old attempt");
    bench
      .store
      .append_event("round.changed", NOW - 2 * DAY, "{}")
      .await
      .expect("an old event");
    if stopped {
      stop(&bench.store).await;
    }
    let mut policy = Policy::builtin().expect("the built-in policy");
    policy.log_days = 1;
    policy.event_days = 1;

    bench.warmer_with(policy).await.pass(None).await.expect("a pass");
    let log = bench.store.attempts("alpha", "line").await.expect("the log");
    let expected: &[i64] = if stopped { &[] } else { &[NOW] };
    assert_eq!(log.iter().map(|attempt| attempt.at).collect::<Vec<_>>(), expected);
    let window = bench.store.events(0, &[], 10).await.expect("the window").items;
    assert!(window.iter().all(|event| event.at == NOW), "stopped: {stopped}");
  }
}

#[tokio::test]
async fn a_pass_beside_one_running_passes_over_the_line_it_holds() {
  let bench = bench().await;
  bench.register("line", line("session")).await;
  let (arrived, release, reply) = held();
  bench.script("line", Vec::new(), reply);
  let warmer = Arc::new(bench.warmer().await);

  let running = tokio::spawn({
    let warmer = warmer.clone();
    async move { warmer.pass(None).await }
  });
  arrived.notified().await;
  let beside = warmer.pass(None).await.expect("a pass beside the first");
  assert!(beside.attempts.is_empty());

  release.notify_one();
  let first = running.await.expect("the first pass ends").expect("a pass");
  assert_eq!(pinged(&first), vec!["line"]);
  // The line given back, the next pass judges the record its ping wrote.
  let after = warmer.pass(None).await.expect("a pass after");
  assert!(after.attempts.is_empty());
}
