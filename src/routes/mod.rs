//! The served routes (docs/api.md, "Routes" and "Events"): the warmer's store, switches and pass over
//! HTTP behind the bearer token, every refusal in the one shape `refusal` gives, and the event stream.

mod refusal;

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::{Stream, StreamExt};
use serde::Serialize;

use crate::pass::Warmer;
use crate::schedule::Clocks;
use crate::store::{Paging, Pause, Registration, RoundChange, StoreError, StoredEvent, SubjectChange};
use crate::wire::{
  self, ConsumerBody, EventQuery, GroupBody, ListQuery, MOST_PAGE_SIZE, PAGE_SIZE, Page, PassBody,
  RoundPatch, SubjectPatch, TargetBody, TargetKey, TargetQuery, TargetUse, UseBody,
};
use crate::words::is_kind;

pub use refusal::Refusal;
use refusal::{Body, Names, Narrowed, checked_name};

/// How often the stream says it is still there.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// What every route stands on: the warmer and the token every request carries.
#[derive(Clone)]
struct Served {
  warmer: Arc<Warmer>,
  token: Arc<str>,
}

type Shared = State<Served>;
type Answer = Result<Response, Refusal>;

/// Every route of the boundary, served from one warmer, each behind the bearer `token`.
pub fn router(warmer: Arc<Warmer>, token: &str) -> Router {
  let served = Served {
    warmer,
    token: token.into(),
  };

  Router::new()
    .route("/health", get(health))
    .route("/events", get(events))
    .route("/consumers", get(list_consumers))
    .route("/consumers/{consumerName}", put(put_consumer))
    .route("/consumers/{consumerName}/groups/{groupName}", put(put_group).delete(delete_group))
    .route("/targets", get(list_targets))
    .route("/consumers/{consumerName}/targets/{targetName}", put(put_target).delete(delete_target))
    .route("/consumers/{consumerName}/targets/{targetName}/uses", post(record_use))
    .route("/consumers/{consumerName}/subjects", get(list_subjects))
    .route("/consumers/{consumerName}/subjects/{subjectName}", axum::routing::patch(change_subject))
    .route("/round", get(round).patch(change_round))
    .route("/passes", post(run_pass))
    .fallback(no_route)
    .method_not_allowed_fallback(no_route)
    .layer(middleware::from_fn_with_state(served.clone(), bearer))
    .with_state(served)
}

// Every route's bearer token, refused where it is missing or another.
async fn bearer(State(served): Shared, request: Request, next: Next) -> Response {
  let given = request
    .headers()
    .get(header::AUTHORIZATION)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.strip_prefix("Bearer "));

  match given {
    Some(token) if token == &*served.token => next.run(request).await,
    Some(_) => {
      Refusal::new(StatusCode::UNAUTHORIZED, "unauthorized", "The bearer token is not the service's.")
        .into_response()
    }
    None => Refusal::new(StatusCode::UNAUTHORIZED, "unauthorized", "The request carries no bearer token.")
      .into_response(),
  }
}

fn answer(status: StatusCode, body: &impl Serialize) -> Response {
  (status, Json(body)).into_response()
}

fn made(new: bool) -> StatusCode {
  if new { StatusCode::CREATED } else { StatusCode::OK }
}

fn done() -> Answer {
  Ok(StatusCode::NO_CONTENT.into_response())
}

/// The page a query asks for, or the refusal of a limit out of its range.
fn paging(after: Option<i64>, limit: Option<u32>) -> Result<Paging, Refusal> {
  let limit = limit.unwrap_or(PAGE_SIZE);
  if !(1..=MOST_PAGE_SIZE).contains(&limit) {
    return Err(Refusal::bad_request(format!("The limit must be 1 to {MOST_PAGE_SIZE}.")));
  }

  Ok(Paging {
    after: after.unwrap_or(0),
    limit,
  })
}

async fn health(State(served): Shared) -> Response {
  let health = match served.warmer.store().round().await {
    Ok(_) => wire::Health {
      ready: true,
      reason: None,
      version: crate::VERSION.to_owned(),
    },
    Err(_) => wire::Health {
      ready: false,
      reason: Some("The store could not be read.".to_owned()),
      version: crate::VERSION.to_owned(),
    },
  };

  answer(StatusCode::OK, &health)
}

async fn list_consumers(State(served): Shared, Narrowed(query): Narrowed<ListQuery>) -> Answer {
  let rows = served.warmer.store().consumers(paging(query.after, query.limit)?).await?;

  Ok(answer(StatusCode::OK, &Page::of(rows, wire::Consumer::from)))
}

async fn put_consumer(
  State(served): Shared,
  Names(consumer): Names<String>,
  Body(body): Body<ConsumerBody>,
) -> Answer {
  let http = reqwest::Url::parse(&body.url).is_ok_and(|url| url.scheme() == "http" && url.has_host());
  if !http {
    return Err(Refusal::rule("invalid-url", "The hook URL must be an absolute http URL."));
  }
  let warmer = &served.warmer;
  let (registered, new) = warmer.store().put_consumer(&consumer, &body.url, warmer.clock().now()).await?;

  Ok(answer(made(new), &wire::Consumer::from(registered)))
}

async fn put_group(
  State(served): Shared,
  Names((consumer, group)): Names<(String, String)>,
  Body(body): Body<GroupBody>,
) -> Answer {
  let (declared, new) = served.warmer.store().put_group(&consumer, &group, body.limit.into()).await?;

  Ok(answer(made(new), &wire::Group::from(declared)))
}

async fn delete_group(State(served): Shared, Names((consumer, group)): Names<(String, String)>) -> Answer {
  served.warmer.store().delete_group(&consumer, &group).await?;
  done()
}

async fn list_targets(State(served): Shared, Narrowed(query): Narrowed<TargetQuery>) -> Answer {
  let warmer = &served.warmer;
  let page = paging(query.after, query.limit)?;
  let rows = warmer
    .store()
    .targets(query.consumer_name.as_deref(), query.subject_name.as_deref(), page)
    .await?;
  let now = warmer.clock().now();

  Ok(answer(StatusCode::OK, &Page::of(rows, |target| warmer.listed(target, now))))
}

/// The registration a body states, every value the body leaves out filled in from the policy, or the
/// refusal of what is out of its shape.
fn registration(warmer: &Warmer, target: &str, body: TargetBody) -> Result<Registration, Refusal> {
  let subject = body.subject_name.unwrap_or_else(|| target.to_owned());
  checked_name("subjectName", &subject)?;
  if let Some(group) = &body.group_name {
    checked_name("groupName", group)?;
  }
  if body.kind.as_deref().is_some_and(|kind| !is_kind(kind)) {
    return Err(Refusal::rule("invalid-kind", "The kind must be a kebab-case word of at most 64 bytes."));
  }
  let margin_ms = body.margin_ms.unwrap_or(0);
  if body.ttl_ms < 0 || margin_ms < 0 {
    return Err(Refusal::invalid_clock("The ttlMs and marginMs must be 0 or more."));
  }
  let idle_ms = body.idle_ms.unwrap_or_else(|| warmer.policy().idle_ms(body.ttl_ms, margin_ms));
  let ping_bound_ms = body.ping_bound_ms;
  if idle_ms <= 0 || ping_bound_ms.is_some_and(|stretch| stretch <= 0) {
    return Err(Refusal::invalid_clock("The idleMs and pingBoundMs must be above 0."));
  }

  Ok(Registration {
    kind: body.kind,
    label: body.label,
    subject,
    rank: body.rank.unwrap_or(0),
    clocks: Clocks {
      idle_ms,
      lifetime_ms: Some(body.ttl_ms),
      margin_ms,
      failure_touches: body.failure_touches.unwrap_or(false),
      honours_idle_override: body.honours_idle_override.unwrap_or(true),
    },
    ping_bound_ms,
    group: body.group_name,
    warm_by_default: body.warm_by_default,
    last_used_at: body.used_at,
  })
}

async fn put_target(
  State(served): Shared,
  Names((consumer, target)): Names<(String, String)>,
  Body(body): Body<TargetBody>,
) -> Answer {
  let warmer = &served.warmer;
  let registration = registration(warmer, &target, body)?;
  let new =
    warmer
      .store()
      .put_target(&consumer, &target, &registration)
      .await
      .map_err(|error| match error {
        StoreError::UnknownGroup => {
          Refusal::rule("unknown-group", "The groupName names no group of the consumer.")
        }
        error => error.into(),
      })?;
  let registered = warmer.target(&consumer, &target).await?.ok_or(StoreError::UnknownTarget)?;
  warmer.publish("target.changed", &registered).await?;

  Ok(answer(made(new), &registered))
}

async fn delete_target(
  State(served): Shared,
  Names((consumer, target)): Names<(String, String)>,
) -> Answer {
  let warmer = &served.warmer;
  let id = warmer.store().delete_target(&consumer, &target).await?;
  let key = TargetKey {
    id,
    consumer_name: consumer,
    name: target,
  };
  warmer.publish("target.deleted", &key).await?;
  done()
}

async fn record_use(
  State(served): Shared,
  Names((consumer, target)): Names<(String, String)>,
  Body(body): Body<UseBody>,
) -> Answer {
  let warmer = &served.warmer;
  let at = body.at.unwrap_or_else(|| warmer.clock().now());
  let started_at = body.started_at.unwrap_or(at);
  let (subject, lifted) = warmer.store().report_use(&consumer, &target, at, started_at).await?;

  warmer.target_changed(&consumer, &target).await?;
  if lifted && let Some(changed) = warmer.store().subject(&consumer, &subject).await? {
    warmer
      .publish("subject.changed", &wire::Subject::of(changed, warmer.clock().now()))
      .await?;
  }
  let used = TargetUse {
    consumer_name: consumer,
    target_name: target,
    at,
    started_at,
  };

  Ok(answer(StatusCode::CREATED, &used))
}

async fn list_subjects(
  State(served): Shared,
  Names(consumer): Names<String>,
  Narrowed(query): Narrowed<ListQuery>,
) -> Answer {
  let warmer = &served.warmer;
  let rows = warmer.store().subjects(&consumer, paging(query.after, query.limit)?).await?;
  let now = warmer.clock().now();

  Ok(answer(StatusCode::OK, &Page::of(rows, |subject| wire::Subject::of(subject, now))))
}

async fn change_subject(
  State(served): Shared,
  Names((consumer, subject)): Names<(String, String)>,
  Body(patch): Body<SubjectPatch>,
) -> Answer {
  let warmer = &served.warmer;
  let now = warmer.clock().now();
  let pause = match patch.paused_until {
    Some(Some(until)) if until <= now => {
      return Err(Refusal::invalid_clock("The pausedUntil must be after now."));
    }
    Some(until) => Some(until.map(|until| Pause { at: now, until })),
    None => None,
  };
  let change = SubjectChange {
    switch: patch.on,
    pause,
  };
  let changed = wire::Subject::of(warmer.store().change_subject(&consumer, &subject, change).await?, now);
  warmer.publish("subject.changed", &changed).await?;

  Ok(answer(StatusCode::OK, &changed))
}

async fn round(State(served): Shared) -> Answer {
  Ok(answer(StatusCode::OK, &served.warmer.round().await?))
}

async fn change_round(State(served): Shared, Body(patch): Body<RoundPatch>) -> Answer {
  if patch.interval_ms.flatten().is_some_and(|interval| interval <= 0) {
    return Err(Refusal::invalid_clock("The intervalMs must be above 0."));
  }
  let warmer = &served.warmer;
  let change = RoundChange {
    stopped: patch.running.map(|running| !running),
    interval_ms: patch.interval_ms,
  };
  let kept = warmer.store().change_round(change).await?;
  if patch.interval_ms.is_some() {
    warmer.interval_changed(kept.interval_ms);
  }
  let round = warmer.round().await?;
  warmer.publish("round.changed", &round).await?;

  Ok(answer(StatusCode::OK, &round))
}

async fn run_pass(State(served): Shared, Body(body): Body<PassBody>) -> Answer {
  if body.idle_override_ms.is_some_and(|stretch| stretch <= 0) {
    return Err(Refusal::invalid_clock("The idleOverrideMs must be above 0."));
  }
  let pass = served.warmer.pass(body.idle_override_ms).await?;

  Ok(answer(StatusCode::CREATED, &pass))
}

/// An event as the stream and the page carry it.
fn event_of(stored: StoredEvent) -> wire::Event {
  wire::Event {
    id: stored.id,
    kind: stored.kind,
    at: stored.at,
    data: serde_json::from_str(&stored.data).unwrap_or(serde_json::Value::Null),
  }
}

async fn events(
  State(served): Shared,
  headers: HeaderMap,
  Narrowed(query): Narrowed<EventQuery>,
) -> Answer {
  let warmer = served.warmer.clone();
  let resumed = headers
    .get("last-event-id")
    .map(|value| value.to_str().ok().and_then(|value| value.trim().parse::<i64>().ok()))
    .map(|read| read.ok_or_else(|| Refusal::bad_request("The Last-Event-ID is not an event number.")))
    .transpose()?;
  let cursor = query.after.or(resumed);
  let kinds: Vec<String> = query
    .kinds
    .as_deref()
    .map(|kinds| {
      kinds
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(str::to_owned)
        .collect()
    })
    .unwrap_or_default();
  // A reader with no cursor starts at the window; one whose cursor stands before it has lost events.
  let first_kept = warmer.store().first_kept_event().await?;
  let after = cursor.unwrap_or(first_kept - 1);
  if after.saturating_add(1) < first_kept {
    return Err(Refusal::new(
      StatusCode::GONE,
      "gone",
      "The events past that number are no longer kept; read the collections again.",
    ));
  }

  let wants_page = headers
    .get(header::ACCEPT)
    .and_then(|value| value.to_str().ok())
    .is_some_and(|accept| accept.contains("application/json"));
  if wants_page {
    let page = paging(Some(after), query.limit)?;
    let rows = warmer.store().events(page.after, &kinds, page.limit).await?;
    return Ok(answer(StatusCode::OK, &Page::of(rows, event_of)));
  }

  Ok(
    Sse::new(stream(warmer, after, kinds))
      .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
      .into_response(),
  )
}

/// What the stream holds between the events it sends.
struct Reading {
  warmer: Arc<Warmer>,
  after: i64,
  kinds: Vec<String>,
  ready: VecDeque<wire::Event>,
  published: tokio::sync::watch::Receiver<i64>,
}

/// The events past `after`, then each one as it is published, for as long as the reader stays and the
/// service does not stop; a reader resumes from the last number it read.
fn stream(
  warmer: Arc<Warmer>,
  after: i64,
  kinds: Vec<String>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> {
  let published = warmer.published();
  let stopped = warmer.stop().begun();
  let reading = Reading {
    warmer,
    after,
    kinds,
    ready: VecDeque::new(),
    published,
  };

  futures_util::stream::unfold(reading, |mut reading| async move {
    loop {
      if let Some(event) = reading.ready.pop_front() {
        let sent = SseEvent::default()
          .id(event.id.to_string())
          .event(event.kind.clone())
          .json_data(&event)
          .unwrap_or_else(|_| SseEvent::default().comment("an event did not write"));
        return Some((Ok(sent), reading));
      }
      reading.published.borrow_and_update();
      match reading.warmer.store().events(reading.after, &reading.kinds, PAGE_SIZE).await {
        Ok(rows) if !rows.items.is_empty() => {
          reading.after = rows.items.last().map_or(reading.after, |last| last.id);
          reading.ready.extend(rows.items.into_iter().map(event_of));
        }
        // A store that cannot be read ends the stream; the reader resumes from the last number it read.
        Err(_) => return None,
        Ok(_) => reading.published.changed().await.ok()?,
      }
    }
  })
  .take_until(stopped)
}

async fn no_route() -> Refusal {
  Refusal::new(StatusCode::NOT_FOUND, "no-route", "No route answers this method and path.")
}
