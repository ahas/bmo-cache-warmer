//! The service's client (docs/api.md, "The client"): one method for each route, sending and reading the
//! bodies `docs/openapi.json` states, and a refusal read back as its status, code and sentence. It carries
//! the contract and no caller's policy.

use std::time::Duration;

use reqwest::{Method, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::wire::{
  Consumer, ConsumerBody, Event, EventQuery, Group, GroupBody, Health, ListQuery, Page, Pass, PassBody,
  Round, RoundPatch, Subject, SubjectPatch, Target, TargetBody, TargetQuery, TargetUse, UseBody, WireError,
};

/// The address the deployed service listens at, where a caller names none.
pub const DEFAULT_ADDRESS: &str = "http://127.0.0.1:9023";

/// How long one call waits for its answer; a pass asked for waits on every ping it makes.
const PATIENCE: Duration = Duration::from_secs(600);

/// Why a call came to no answer of the route's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientFailure {
  /// The service answered with a status that is no success: its error body where it was one, and the body as
  /// it came.
  Answered {
    status: u16,
    answer: Option<WireError>,
    body: String,
  },
  /// Nothing answered at the address.
  Unreached(String),
  /// A success came back in no shape the route answers.
  Unreadable(String),
}

impl std::fmt::Display for ClientFailure {
  fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Answered {
        status,
        answer: Some(answer),
        ..
      } => write!(out, "{status} {}: {}", answer.code, answer.error),
      Self::Answered { status, body, .. } => write!(out, "{status}: {body}"),
      Self::Unreached(why) => write!(out, "the service could not be reached: {why}"),
      Self::Unreadable(why) => write!(out, "the service's answer does not read: {why}"),
    }
  }
}

impl std::error::Error for ClientFailure {}

type Called<T> = Result<T, ClientFailure>;

/// A query type's fields as the pairs of a query string, the absent ones left out.
fn pairs(query: &impl Serialize) -> Vec<(String, String)> {
  let Ok(Value::Object(fields)) = serde_json::to_value(query) else {
    return Vec::new();
  };

  fields
    .into_iter()
    .filter_map(|(name, value)| match value {
      Value::Null => None,
      Value::String(text) => Some((name, text)),
      other => Some((name, other.to_string())),
    })
    .collect()
}

/// The client of one service: its address and its bearer token.
#[derive(Clone)]
pub struct WarmerClient {
  address: Url,
  token: String,
  http: reqwest::Client,
}

impl WarmerClient {
  /// A client of the service at `address`, an absolute http URL, sending `token`.
  pub fn new(address: &str, token: impl Into<String>) -> Result<Self, ClientFailure> {
    let address = Url::parse(address)
      .ok()
      .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
      .ok_or_else(|| ClientFailure::Unreached(format!("{address} is not an absolute http URL")))?;
    let http = reqwest::Client::builder()
      .no_proxy()
      .timeout(PATIENCE)
      .build()
      .map_err(|why| ClientFailure::Unreached(why.to_string()))?;

    Ok(Self {
      address,
      token: token.into(),
      http,
    })
  }

  // The URL of a route: the address, then each segment encoded.
  fn url(&self, segments: &[&str]) -> Url {
    let mut url = self.address.clone();
    url.path_segments_mut().expect("an http URL has a path").pop_if_empty().extend(segments);
    url
  }

  // One call: its method, its path, its query and its body; the answer read as `T`, or none for a 204.
  async fn call<T: DeserializeOwned>(
    &self,
    method: Method,
    segments: &[&str],
    query: Vec<(String, String)>,
    body: Option<&Value>,
  ) -> Called<Option<T>> {
    let mut url = self.url(segments);
    if !query.is_empty() {
      url.query_pairs_mut().extend_pairs(&query);
    }
    let mut request = self
      .http
      .request(method, url)
      .bearer_auth(&self.token)
      .header(reqwest::header::ACCEPT, "application/json");
    if let Some(body) = body {
      request = request.json(body);
    }
    let response = request
      .send()
      .await
      .map_err(|why| ClientFailure::Unreached(why.without_url().to_string()))?;
    let status = response.status();
    let text = response
      .text()
      .await
      .map_err(|why| ClientFailure::Unreached(why.without_url().to_string()))?;

    if !status.is_success() {
      return Err(ClientFailure::Answered {
        status: status.as_u16(),
        answer: serde_json::from_str(&text).ok(),
        body: text,
      });
    }
    if status == reqwest::StatusCode::NO_CONTENT {
      return Ok(None);
    }
    serde_json::from_str(&text)
      .map(Some)
      .map_err(|why| ClientFailure::Unreadable(why.to_string()))
  }

  async fn read<T: DeserializeOwned>(&self, segments: &[&str], query: Vec<(String, String)>) -> Called<T> {
    self.answered(self.call(Method::GET, segments, query, None).await)
  }

  async fn send<T: DeserializeOwned>(
    &self,
    method: Method,
    segments: &[&str],
    body: &impl Serialize,
  ) -> Called<T> {
    let body = serde_json::to_value(body).map_err(|why| ClientFailure::Unreadable(why.to_string()))?;

    self.answered(self.call(method, segments, Vec::new(), Some(&body)).await)
  }

  async fn delete(&self, segments: &[&str]) -> Called<()> {
    self.call::<Value>(Method::DELETE, segments, Vec::new(), None).await.map(drop)
  }

  fn answered<T>(&self, called: Called<Option<T>>) -> Called<T> {
    called?.ok_or_else(|| ClientFailure::Unreadable("the route answered no body".to_owned()))
  }

  /// `GET /health`.
  pub async fn health(&self) -> Called<Health> {
    self.read(&["health"], Vec::new()).await
  }

  /// `GET /events`, as a page.
  pub async fn events(&self, query: &EventQuery) -> Called<Page<Event>> {
    self.read(&["events"], pairs(query)).await
  }

  /// `GET /consumers`.
  pub async fn consumers(&self, query: &ListQuery) -> Called<Page<Consumer>> {
    self.read(&["consumers"], pairs(query)).await
  }

  /// `PUT /consumers/{consumerName}`.
  pub async fn put_consumer(&self, consumer: &str, body: &ConsumerBody) -> Called<Consumer> {
    self.send(Method::PUT, &["consumers", consumer], body).await
  }

  /// `PUT /consumers/{consumerName}/groups/{groupName}`.
  pub async fn put_group(&self, consumer: &str, group: &str, body: &GroupBody) -> Called<Group> {
    self.send(Method::PUT, &["consumers", consumer, "groups", group], body).await
  }

  /// `DELETE /consumers/{consumerName}/groups/{groupName}`.
  pub async fn delete_group(&self, consumer: &str, group: &str) -> Called<()> {
    self.delete(&["consumers", consumer, "groups", group]).await
  }

  /// `GET /targets`.
  pub async fn targets(&self, query: &TargetQuery) -> Called<Page<Target>> {
    self.read(&["targets"], pairs(query)).await
  }

  /// `PUT /consumers/{consumerName}/targets/{targetName}`.
  pub async fn put_target(&self, consumer: &str, target: &str, body: &TargetBody) -> Called<Target> {
    self.send(Method::PUT, &["consumers", consumer, "targets", target], body).await
  }

  /// `DELETE /consumers/{consumerName}/targets/{targetName}`.
  pub async fn delete_target(&self, consumer: &str, target: &str) -> Called<()> {
    self.delete(&["consumers", consumer, "targets", target]).await
  }

  /// `POST /consumers/{consumerName}/targets/{targetName}/uses`.
  pub async fn record_use(&self, consumer: &str, target: &str, body: &UseBody) -> Called<TargetUse> {
    self.send(Method::POST, &["consumers", consumer, "targets", target, "uses"], body).await
  }

  /// `GET /consumers/{consumerName}/subjects`.
  pub async fn subjects(&self, consumer: &str, query: &ListQuery) -> Called<Page<Subject>> {
    self.read(&["consumers", consumer, "subjects"], pairs(query)).await
  }

  /// `PATCH /consumers/{consumerName}/subjects/{subjectName}`.
  pub async fn change_subject(
    &self,
    consumer: &str,
    subject: &str,
    patch: &SubjectPatch,
  ) -> Called<Subject> {
    self.send(Method::PATCH, &["consumers", consumer, "subjects", subject], patch).await
  }

  /// `GET /round`.
  pub async fn round(&self) -> Called<Round> {
    self.read(&["round"], Vec::new()).await
  }

  /// `PATCH /round`.
  pub async fn change_round(&self, patch: &RoundPatch) -> Called<Round> {
    self.send(Method::PATCH, &["round"], patch).await
  }

  /// `POST /passes`.
  pub async fn run_pass(&self, body: &PassBody) -> Called<Pass> {
    self.send(Method::POST, &["passes"], body).await
  }
}
