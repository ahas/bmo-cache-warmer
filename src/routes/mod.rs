//! The served routes (docs/api.md, "Served routes"): the warmer's store, switches and pass over HTTP,
//! every refusal in the one shape `refusal` gives.

mod refusal;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use cache_warmer::Clocks;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::pass::{PassError, Warmer};
use crate::store::{Attempt, Registration, StoreError, Subject, Target};
use crate::words::is_snake_case;

pub use refusal::Refusal;
use refusal::{Body, Names, Narrowed, checked_name};

type Shared = State<Arc<Warmer>>;
type Answer = Result<Response, Refusal>;

/// Every route of the boundary, served from one warmer.
pub fn router(warmer: Arc<Warmer>) -> Router {
  Router::new()
    .route("/health", get(health))
    .route("/consumers/{consumer}", put(put_consumer))
    .route("/consumers/{consumer}/groups/{group}", put(put_group).delete(delete_group))
    .route("/consumers/{consumer}/targets/{target}", put(put_target).delete(delete_target))
    .route("/consumers/{consumer}/targets/{target}/used", post(report_use))
    .route("/targets", get(list_targets))
    .route("/consumers/{consumer}/subjects", get(list_subjects))
    .route("/consumers/{consumer}/subjects/{subject}/switch", put(set_switch))
    .route("/consumers/{consumer}/subjects/{subject}/pause", post(pause))
    .route("/round", get(round).put(set_round))
    .route("/pass", post(run_pass))
    .fallback(unknown_path)
    .method_not_allowed_fallback(unknown_method)
    .with_state(warmer)
}

fn answer(status: StatusCode, body: Value) -> Response {
  (status, Json(body)).into_response()
}

fn made(new: bool) -> StatusCode {
  if new { StatusCode::CREATED } else { StatusCode::OK }
}

fn done() -> Answer {
  Ok(StatusCode::NO_CONTENT.into_response())
}

/// A target as `GET /targets` lists it, judged at `now`.
fn listed(target: &Target, policy_default: bool, now: i64) -> Value {
  json!({
    "consumer": target.consumer,
    "target": target.name,
    "kind": target.kind,
    "label": target.label,
    "subject": target.subject,
    "rank": target.rank,
    "group": target.group.as_ref().map(|group| &group.name),
    "idle_ms": target.clocks.idle_ms,
    "lifetime_ms": target.clocks.lifetime_ms,
    "margin_ms": target.clocks.margin_ms,
    "ping_bound_ms": target.ping_bound_ms,
    "warm_by_default": target.warm_by_default.unwrap_or(policy_default),
    "failure_touches": target.clocks.failure_touches,
    "honours_idle_override": target.clocks.honours_idle_override,
    "on": target.on(policy_default),
    "paused_at": target.paused_at,
    "last_used_at": target.record.last_used_at,
    "last_warmed_at": target.record.last_warmed_at,
    "last_failed_at": target.record.last_failed_at,
    "last_attempt": target.last_attempt.as_ref().map(attempt),
    "due_at": target.candidate(policy_default).due_at(now),
  })
}

fn attempt(attempt: &Attempt) -> Value {
  json!({
    "at": attempt.at,
    "outcome": attempt.outcome.as_str(),
    "code": attempt.code,
    "reason": attempt.reason,
  })
}

fn subject(subject: &Subject) -> Value {
  json!({
    "subject": subject.subject,
    "switch": subject.switch.map(|on| if on { "on" } else { "off" }),
    "paused_at": subject.paused_at,
    "targets": subject.targets,
  })
}

async fn health(State(warmer): Shared) -> Answer {
  warmer.store().round_stopped().await?;

  Ok(answer(
    StatusCode::OK,
    json!({ "ok": true, "service": "cache-warmer", "version": env!("CARGO_PKG_VERSION") }),
  ))
}

#[derive(Deserialize)]
struct ConsumerBody {
  hook_url: String,
}

async fn put_consumer(
  State(warmer): Shared,
  Names(consumer): Names<String>,
  Body(body): Body<ConsumerBody>,
) -> Answer {
  let http = reqwest::Url::parse(&body.hook_url).is_ok_and(|url| url.scheme() == "http" && url.has_host());
  if !http {
    return Err(Refusal::new(
      StatusCode::BAD_REQUEST,
      "invalid_hook_url",
      "The hook URL must be an absolute http URL.",
    ));
  }
  let new = warmer.store().put_consumer(&consumer, &body.hook_url).await?;

  Ok(answer(made(new), json!({ "consumer": consumer, "hook_url": body.hook_url })))
}

#[derive(Deserialize)]
struct GroupBody {
  limit: u32,
}

async fn put_group(
  State(warmer): Shared,
  Names((consumer, group)): Names<(String, String)>,
  Body(body): Body<GroupBody>,
) -> Answer {
  let new = warmer.store().put_group(&consumer, &group, body.limit.into()).await?;

  Ok(answer(made(new), json!({ "consumer": consumer, "group": group, "limit": body.limit })))
}

async fn delete_group(State(warmer): Shared, Names((consumer, group)): Names<(String, String)>) -> Answer {
  warmer.store().delete_group(&consumer, &group).await?;
  done()
}

#[derive(Deserialize)]
struct TargetBody {
  kind: String,
  label: String,
  subject: Option<String>,
  rank: i64,
  idle_ms: i64,
  lifetime_ms: Option<i64>,
  margin_ms: Option<i64>,
  ping_bound_ms: i64,
  group: Option<String>,
  warm_by_default: Option<bool>,
  failure_touches: Option<bool>,
  honours_idle_override: Option<bool>,
  last_used_at: Option<i64>,
}

impl TargetBody {
  /// The registration the body states, or the refusal of what is out of its shape.
  fn registration(self, target: &str) -> Result<Registration, Refusal> {
    let subject = self.subject.unwrap_or_else(|| target.to_owned());
    checked_name("subject", &subject)?;
    if let Some(group) = &self.group {
      checked_name("group", group)?;
    }
    if !is_snake_case(&self.kind) {
      return Err(Refusal::new(
        StatusCode::BAD_REQUEST,
        "invalid_kind",
        "The kind must be a snake_case word.",
      ));
    }
    let margin_ms = self.margin_ms.unwrap_or(0);
    if self.idle_ms <= 0 || self.ping_bound_ms <= 0 {
      return Err(Refusal::invalid_clock("The idle_ms and ping_bound_ms must be above 0."));
    }
    if self.lifetime_ms.is_some_and(|lifetime| lifetime < 0) || margin_ms < 0 {
      return Err(Refusal::invalid_clock("The lifetime_ms and margin_ms must be 0 or more."));
    }

    Ok(Registration {
      kind: self.kind,
      label: self.label,
      subject,
      rank: self.rank,
      clocks: Clocks {
        idle_ms: self.idle_ms,
        lifetime_ms: self.lifetime_ms,
        margin_ms,
        failure_touches: self.failure_touches.unwrap_or(false),
        honours_idle_override: self.honours_idle_override.unwrap_or(true),
      },
      ping_bound_ms: self.ping_bound_ms,
      group: self.group,
      warm_by_default: self.warm_by_default,
      last_used_at: self.last_used_at,
    })
  }
}

async fn put_target(
  State(warmer): Shared,
  Names((consumer, target)): Names<(String, String)>,
  Body(body): Body<TargetBody>,
) -> Answer {
  let registration = body.registration(&target)?;
  let store = warmer.store();
  let new = store.put_target(&consumer, &target, &registration).await.map_err(|error| match error {
    StoreError::UnknownGroup => {
      Refusal::new(StatusCode::CONFLICT, "unknown_group", "The group names no group of the consumer.")
    }
    error => error.into(),
  })?;
  let registered = store.target(&consumer, &target).await?.ok_or(StoreError::UnknownTarget)?;

  Ok(answer(
    made(new),
    listed(&registered, warmer.policy().warm_by_default, warmer.clock().now()),
  ))
}

async fn delete_target(
  State(warmer): Shared,
  Names((consumer, target)): Names<(String, String)>,
) -> Answer {
  warmer.store().delete_target(&consumer, &target).await?;
  done()
}

#[derive(Deserialize)]
struct UseBody {
  at: Option<i64>,
  started_at: Option<i64>,
}

async fn report_use(
  State(warmer): Shared,
  Names((consumer, target)): Names<(String, String)>,
  Body(body): Body<UseBody>,
) -> Answer {
  let at = body.at.unwrap_or_else(|| warmer.clock().now());
  warmer.store().report_use(&consumer, &target, at, body.started_at.unwrap_or(at)).await?;
  done()
}

#[derive(Deserialize)]
struct TargetQuery {
  consumer: Option<String>,
  subject: Option<String>,
}

async fn list_targets(State(warmer): Shared, Narrowed(query): Narrowed<TargetQuery>) -> Answer {
  let targets = warmer.store().targets(query.consumer.as_deref(), query.subject.as_deref()).await?;
  let (default, now) = (warmer.policy().warm_by_default, warmer.clock().now());
  let targets: Vec<Value> = targets.iter().map(|target| listed(target, default, now)).collect();

  Ok(answer(StatusCode::OK, json!({ "targets": targets })))
}

async fn list_subjects(State(warmer): Shared, Names(consumer): Names<String>) -> Answer {
  let subjects: Vec<Value> = warmer.store().subjects(&consumer).await?.iter().map(subject).collect();

  Ok(answer(StatusCode::OK, json!({ "subjects": subjects })))
}

#[derive(Deserialize)]
struct SwitchBody {
  on: Option<bool>,
}

async fn set_switch(
  State(warmer): Shared,
  Names((consumer, name)): Names<(String, String)>,
  Body(body): Body<SwitchBody>,
) -> Answer {
  let switched = warmer.store().set_switch(&consumer, &name, body.on).await?;

  Ok(answer(StatusCode::OK, subject(&switched)))
}

async fn pause(State(warmer): Shared, Names((consumer, name)): Names<(String, String)>) -> Answer {
  let paused = warmer.store().pause(&consumer, &name, warmer.clock().now()).await?;

  Ok(answer(StatusCode::OK, subject(&paused)))
}

async fn round(State(warmer): Shared) -> Answer {
  let stopped = warmer.store().round_stopped().await?;

  Ok(answer(StatusCode::OK, json!({ "stopped": stopped })))
}

#[derive(Deserialize)]
struct RoundBody {
  stopped: bool,
}

async fn set_round(State(warmer): Shared, Body(body): Body<RoundBody>) -> Answer {
  warmer.store().set_round_stopped(body.stopped).await?;

  Ok(answer(StatusCode::OK, json!({ "stopped": body.stopped })))
}

#[derive(Deserialize)]
struct PassBody {
  idle_override_ms: Option<i64>,
}

async fn run_pass(State(warmer): Shared, Body(body): Body<PassBody>) -> Answer {
  if body.idle_override_ms.is_some_and(|stretch| stretch <= 0) {
    return Err(Refusal::invalid_clock("The idle_override_ms must be above 0."));
  }
  let report = warmer.try_pass(body.idle_override_ms).await.map_err(|error| match error {
    PassError::Running => Refusal::new(StatusCode::CONFLICT, "pass_running", error.to_string()),
    PassError::Store(error) => error.into(),
  })?;
  let attempts: Vec<Value> = report
    .attempts
    .iter()
    .map(|pinged| {
      json!({
        "consumer": pinged.consumer,
        "target": pinged.target,
        "outcome": pinged.attempt.outcome.as_str(),
        "code": pinged.attempt.code,
        "reason": pinged.attempt.reason,
      })
    })
    .collect();

  Ok(answer(StatusCode::OK, json!({ "stopped": report.stopped, "attempts": attempts })))
}

async fn unknown_path() -> Refusal {
  Refusal::new(StatusCode::NOT_FOUND, "unknown_route", "No route serves this path.")
}

async fn unknown_method() -> Refusal {
  Refusal::new(
    StatusCode::METHOD_NOT_ALLOWED,
    "unknown_route",
    "No route serves this path under this method.",
  )
}
