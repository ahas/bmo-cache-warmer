//! The pass (docs/warming.md, "The pass" and "What an answer leaves behind"), run against a fake consumer
//! on loopback whose hook answers as each test scripts it, on a clock the test moves.

mod fixtures;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bmo_cache_warmer::pass::{PassError, Report, Warmer};
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::store::{Attempt, Outcome, Registration, Store};
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
  let target = ping["target"].as_str().unwrap_or_default().to_owned();
  consumer.heard.lock().expect("the log of pings").push(ping);
  let scripted = consumer.script.lock().expect("the script").get(&target).cloned();
  let (acts, reply) =
    scripted.unwrap_or_else(|| (Vec::new(), Reply::Answer(json!({ "outcome": "warmed" }))));

  for act in acts {
    match act {
      Act::StopRound => consumer.store.set_round_stopped(true).await.expect("the round stops"),
      Act::SwitchOff(subject) => {
        consumer.store.set_switch("agency", subject, Some(false)).await.expect("the switch");
      }
      Act::Retire(target) => {
        consumer.store.delete_target("agency", target).await.expect("the target retires")
      }
      Act::Advance(by) => consumer.clock.advance(by),
    }
  }

  match reply {
    Reply::Answer(answer) => Json(answer).into_response(),
    Reply::Status(status) => StatusCode::from_u16(status).expect("a status").into_response(),
    Reply::Body(body) => body.into_response(),
    Reply::Held { arrived, release } => {
      arrived.notify_one();
      release.notified().await;
      Json(json!({ "outcome": "warmed" })).into_response()
    }
    Reply::Never => std::future::pending().await,
  }
}

/// A store, a clock at `NOW`, and the consumer `agency` whose hook is the fake one.
struct Bench {
  _dir: TempDir,
  store: Store,
  clock: Arc<TestClock>,
  consumer: Arc<Consumer>,
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
  let app = Router::new().route("/ping", post(hook)).with_state(consumer.clone());
  tokio::spawn(async move { axum::serve(listener, app).await.expect("the fake consumer serves") });
  store.put_consumer("agency", &hook_url).await.expect("the consumer");

  Bench {
    _dir: dir,
    store,
    clock,
    consumer,
  }
}

impl Bench {
  fn warmer(&self) -> Warmer {
    self.warmer_with(Policy::builtin().expect("the built-in policy"))
  }

  fn warmer_with(&self, policy: Policy) -> Warmer {
    Warmer::new(self.store.clone(), policy, self.clock.clone())
  }

  async fn register(&self, target: &str, registration: Registration) {
    self.store.put_target("agency", target, &registration).await.expect("a target");
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
      .map(|ping| ping["target"].as_str().unwrap_or_default().to_owned())
      .collect()
  }

  async fn last(&self, target: &str) -> Attempt {
    let target = self.store.target("agency", target).await.expect("the target").expect("it stands");
    target.last_attempt.expect("an attempt")
  }
}

fn used(mut registration: Registration, at: i64) -> Registration {
  registration.last_used_at = Some(at);
  registration
}

fn pinged(report: &Report) -> Vec<&str> {
  report.attempts.iter().map(|pinged| pinged.target.as_str()).collect()
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
  bench.register("line", line("worker")).await;
  bench.store.set_round_stopped(true).await.expect("the round stops");

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert!(report.stopped);
  assert!(report.attempts.is_empty());
  assert!(bench.heard().is_empty());
  let target = bench.store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.last_attempt, None);
}

#[tokio::test]
async fn due_targets_are_pinged_in_rank_order_with_their_ping() {
  let bench = bench().await;
  bench.store.put_group("agency", "opus", 1).await.expect("a group");
  bench.register("b", line("worker")).await;
  bench.register("a", line("other")).await;
  bench.register("place", place("owner", "opus")).await;
  bench.register("fresh", used(line("fresh"), NOW - MINUTE)).await;
  let mut off = line("quiet");
  off.warm_by_default = None;
  bench.register("off", off).await;

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert!(!report.stopped);
  assert_eq!(pinged(&report), vec!["place", "a", "b"]);
  assert!(report.attempts.iter().all(|pinged| pinged.attempt
    == Attempt {
      at: NOW,
      outcome: Outcome::Warmed,
      code: None,
      reason: None,
    }));

  let heard = bench.consumer.heard.lock().expect("the log of pings").clone();
  assert_eq!(
    heard[0],
    json!({
      "consumer": "agency",
      "target": "place",
      "kind": "pool_place",
      "subject": "owner",
      "bound_ms": 120_000,
    })
  );
}

#[tokio::test]
async fn only_a_warmed_answer_restarts_the_clock() {
  let bench = bench().await;
  bench.register("warmed", line("a")).await;
  bench.register("busy", line("b")).await;
  bench.register("skipped", line("c")).await;
  bench.answer("busy", json!({ "outcome": "busy", "code": "ignored", "reason": "Ignored." }));
  bench.answer("skipped", json!({ "outcome": "skipped", "code": "provider_off" }));
  let warmer = bench.warmer();

  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["busy", "skipped", "warmed"]);
  assert_eq!(
    bench.last("busy").await,
    Attempt {
      at: NOW,
      outcome: Outcome::Busy,
      code: None,
      reason: None,
    }
  );
  assert_eq!(outcome_of(&bench.last("skipped").await), (Outcome::Skipped, Some("provider_off")));
  let warmed = bench.store.target("agency", "warmed").await.expect("the target").expect("it stands");
  assert_eq!(warmed.record.last_warmed_at, Some(NOW));

  bench.clock.advance(MINUTE);
  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["busy", "skipped"]);

  bench.clock.advance(IDLE);
  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["busy", "skipped", "warmed"]);
}

#[tokio::test]
async fn a_failure_touches_only_where_registered() {
  let bench = bench().await;
  bench.store.put_group("agency", "opus", 5).await.expect("a group");
  bench.register("line", line("worker")).await;
  bench.register("place", place("owner", "opus")).await;
  let refusal =
    json!({ "outcome": "failed", "code": "launch_failed", "reason": "The line did not launch." });
  bench.answer("line", refusal.clone());
  bench.answer("place", refusal);
  let warmer = bench.warmer();

  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["place", "line"]);
  for name in ["line", "place"] {
    let target = bench.store.target("agency", name).await.expect("the target").expect("it stands");
    assert_eq!(target.record.last_failed_at, Some(NOW));
    assert_eq!(target.record.last_warmed_at, None);
    let attempt = target.last_attempt.expect("an attempt");
    assert_eq!(outcome_of(&attempt), failed("launch_failed"));
    assert_eq!(attempt.reason.as_deref(), Some("The line did not launch."));
  }

  bench.clock.advance(MINUTE);
  warmer.pass(None).await.expect("a pass");
  assert_eq!(bench.heard(), vec!["place"]);
}

#[tokio::test]
async fn a_hook_that_answers_wrongly_is_recorded_as_failed_with_the_warmers_code() {
  let bench = bench().await;
  let cases: [(&str, Reply, &str); 7] = [
    ("status", Reply::Status(500), "hook_status"),
    ("created", Reply::Status(201), "hook_status"),
    ("garbage", Reply::Body("not an answer"), "hook_answer"),
    ("unknown", Reply::Answer(json!({ "outcome": "cooled" })), "hook_answer"),
    ("codeless", Reply::Answer(json!({ "outcome": "skipped" })), "hook_answer"),
    (
      "reasonless",
      Reply::Answer(json!({ "outcome": "failed", "code": "broke" })),
      "hook_answer",
    ),
    (
      "misshapen",
      Reply::Answer(json!({ "outcome": "failed", "code": "Broke", "reason": "It broke." })),
      "hook_answer",
    ),
  ];
  for (name, reply, _) in &cases {
    bench.register(name, line(name)).await;
    bench.script(name, Vec::new(), reply.clone());
  }

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert_eq!(report.attempts.len(), cases.len());
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
  let closed = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
  let hook_url = format!("http://{}/ping", closed.local_addr().expect("the port's address"));
  drop(closed);
  bench.store.put_consumer("gone", &hook_url).await.expect("a consumer");
  bench.store.put_target("gone", "line", &line("worker")).await.expect("a target");

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert_eq!(pinged(&report), vec!["line"]);
  assert_eq!(outcome_of(&report.attempts[0].attempt), failed("hook_unreachable"));
  assert_eq!(report.attempts[0].consumer, "gone");
}

#[tokio::test]
async fn a_hook_silent_past_its_bound_times_out_and_the_pass_goes_on() {
  let bench = bench().await;
  let mut silent = line("a");
  silent.ping_bound_ms = 1;
  bench.register("silent", silent).await;
  bench.register("then", line("b")).await;
  bench.script("silent", Vec::new(), Reply::Never);
  let mut policy = Policy::builtin().expect("the built-in policy");
  policy.ping_slack_seconds = 0;

  let report = bench.warmer_with(policy).pass(None).await.expect("a pass");
  assert_eq!(pinged(&report), vec!["silent", "then"]);
  let attempt = &report.attempts[0].attempt;
  assert_eq!(outcome_of(attempt), failed("hook_timeout"));
  assert_eq!(attempt.reason.as_deref(), Some("The hook did not answer within 1 ms."));
  assert_eq!(report.attempts[1].attempt.outcome, Outcome::Warmed);
}

#[tokio::test]
async fn an_attempt_is_timed_when_its_answer_arrives() {
  let bench = bench().await;
  bench.register("line", line("worker")).await;
  bench.script("line", vec![Act::Advance(5_000)], Reply::Answer(json!({ "outcome": "warmed" })));

  bench.warmer().pass(None).await.expect("a pass");
  let target = bench.store.target("agency", "line").await.expect("the target").expect("it stands");
  assert_eq!(target.record.last_warmed_at, Some(NOW + 5_000));
  assert_eq!(target.last_attempt.map(|attempt| attempt.at), Some(NOW + 5_000));
}

#[tokio::test]
async fn a_target_turned_off_or_retired_mid_pass_is_passed_over_and_not_replaced() {
  let bench = bench().await;
  bench.store.put_group("agency", "opus", 1).await.expect("a group");
  let mut lead = line("lead");
  lead.rank = 1;
  bench.register("lead", lead).await;
  bench.register("oldest", used(place("owner", "opus"), NOW - 45 * MINUTE)).await;
  bench.register("spare", used(place("spare", "opus"), NOW - 41 * MINUTE)).await;
  let mut gone = line("gone");
  gone.rank = 60;
  bench.register("gone", gone).await;
  let mut tail = line("tail");
  tail.rank = 70;
  bench.register("tail", tail).await;
  bench.script(
    "lead",
    vec![Act::SwitchOff("owner"), Act::Retire("gone")],
    Reply::Answer(json!({ "outcome": "warmed" })),
  );

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert!(!report.stopped);
  assert_eq!(pinged(&report), vec!["lead", "tail"]);
  assert_eq!(bench.heard(), vec!["lead", "tail"]);
  let oldest = bench.store.target("agency", "oldest").await.expect("the target").expect("it stands");
  assert_eq!(oldest.last_attempt, None);
}

#[tokio::test]
async fn a_round_stopped_mid_pass_ends_the_pass() {
  let bench = bench().await;
  bench.register("a", line("a")).await;
  bench.register("b", line("b")).await;
  bench.script("a", vec![Act::StopRound], Reply::Answer(json!({ "outcome": "warmed" })));

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert!(report.stopped);
  assert_eq!(pinged(&report), vec!["a"]);
  assert_eq!(bench.heard(), vec!["a"]);
}

#[tokio::test]
async fn a_group_pings_its_oldest_up_to_its_limit() {
  let bench = bench().await;
  bench.store.put_group("agency", "opus", 2).await.expect("a group");
  bench.register("p1", used(place("one", "opus"), NOW - 45 * MINUTE)).await;
  bench.register("p2", used(place("two", "opus"), NOW - 50 * MINUTE)).await;
  bench.register("p3", place("three", "opus")).await;

  let report = bench.warmer().pass(None).await.expect("a pass");
  assert_eq!(pinged(&report), vec!["p3", "p2"]);
}

#[tokio::test]
async fn an_idle_override_reaches_only_targets_that_honour_it() {
  let bench = bench().await;
  bench.store.put_group("agency", "opus", 5).await.expect("a group");
  bench.register("line", used(line("worker"), NOW - 10 * MINUTE)).await;
  bench.register("place", used(place("owner", "opus"), NOW - 10 * MINUTE)).await;
  let warmer = bench.warmer();

  assert!(warmer.pass(None).await.expect("a pass").attempts.is_empty());
  let report = warmer.pass(Some(5 * MINUTE)).await.expect("a pass");
  assert_eq!(pinged(&report), vec!["line"]);
}

#[tokio::test]
async fn a_long_reason_is_cut_to_the_policys_units() {
  let bench = bench().await;
  bench.register("line", line("worker")).await;
  bench.answer("line", json!({ "outcome": "failed", "reason": "The line did not launch." }));
  let mut policy = Policy::builtin().expect("the built-in policy");
  policy.reason_units = 8;

  let report = bench.warmer_with(policy).pass(None).await.expect("a pass");
  assert_eq!(report.attempts[0].attempt.reason.as_deref(), Some("The line"));
  assert_eq!(bench.last("line").await.reason.as_deref(), Some("The line"));
  assert_eq!(bench.last("line").await.code, None);
}

#[tokio::test]
async fn a_pass_ends_by_pruning_the_log_to_its_days() {
  let bench = bench().await;
  bench.register("line", line("worker")).await;
  let old = Attempt {
    at: NOW - 2 * DAY,
    outcome: Outcome::Busy,
    code: None,
    reason: None,
  };
  bench.store.record_attempt("agency", "line", &old).await.expect("an old attempt");
  let mut policy = Policy::builtin().expect("the built-in policy");
  policy.log_days = 1;

  bench.warmer_with(policy).pass(None).await.expect("a pass");
  let log = bench.store.attempts("agency", "line").await.expect("the log");
  assert_eq!(log.iter().map(|attempt| attempt.at).collect::<Vec<_>>(), vec![NOW]);
}

#[tokio::test]
async fn a_pass_asked_for_while_one_runs_is_refused_and_a_waiting_one_runs_after() {
  let bench = bench().await;
  bench.register("line", line("worker")).await;
  let (arrived, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
  bench.script(
    "line",
    Vec::new(),
    Reply::Held {
      arrived: arrived.clone(),
      release: release.clone(),
    },
  );
  let warmer = Arc::new(bench.warmer());

  let running = tokio::spawn({
    let warmer = warmer.clone();
    async move { warmer.try_pass(None).await }
  });
  arrived.notified().await;
  assert!(matches!(warmer.try_pass(None).await, Err(PassError::Running)));
  let waiting = tokio::spawn({
    let warmer = warmer.clone();
    async move { warmer.pass(None).await }
  });

  release.notify_one();
  let first = running.await.expect("the first pass ends").expect("a pass");
  assert_eq!(pinged(&first), vec!["line"]);
  let second = waiting.await.expect("the waiting pass ends").expect("a pass");
  assert!(second.attempts.is_empty());
}
