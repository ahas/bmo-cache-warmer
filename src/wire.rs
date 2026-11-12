//! The bodies the routes take and answer, as the wire writes them (docs/api.md, "Wire format" and
//! "Routes"): camelCase keys, times in epoch milliseconds. The OpenAPI document, the client and the MCP
//! server's tools are written from these same types.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::store;

/// A list's page size where none is asked for, and the most one page holds.
pub const PAGE_SIZE: u32 = 100;
pub const MOST_PAGE_SIZE: u32 = 1000;

/// A field of a merge patch: absent, `None`; given null, `Some(None)`; given a value, `Some(Some(value))`.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
  input: D,
) -> Result<Option<Option<T>>, D::Error> {
  Option::<T>::deserialize(input).map(Some)
}

/// A refusal: a kebab-case code and an English sentence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WireError {
  pub code: String,
  pub error: String,
}

/// Whether the service can answer, and its version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
  pub ready: bool,
  /// Why it is not ready; null when it is.
  pub reason: Option<String>,
  pub version: String,
}

/// A page of a list, in id order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Page<T> {
  pub items: Vec<T>,
  /// Whether anything stands past the last item.
  pub more: bool,
}

impl<T> Page<T> {
  pub fn of<S>(rows: store::Rows<S>, each: impl FnMut(S) -> T) -> Self {
    Self {
      items: rows.items.into_iter().map(each).collect(),
      more: rows.more,
    }
  }
}

/// Where a page starts and how long it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
  /// The id of the last item read; the page starts past it.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub after: Option<i64>,
  /// 1 to 1000; 100 where absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub limit: Option<u32>,
}

/// The targets to list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetQuery {
  /// One consumer's targets.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub consumer_name: Option<String>,
  /// One subject's targets.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub subject_name: Option<String>,
  /// The id of the last item read; the page starts past it.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub after: Option<i64>,
  /// 1 to 1000; 100 where absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub limit: Option<u32>,
}

/// The events to read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventQuery {
  /// A comma-separated list of the kinds to read; every kind where absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub kinds: Option<String>,
  /// The number of the last event read.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub after: Option<i64>,
  /// 1 to 1000; 100 where absent. The page form's alone.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub limit: Option<u32>,
}

/// One event: its number, its kind, when it was published and the member it carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
  pub id: i64,
  pub kind: String,
  pub at: i64,
  pub data: Value,
}

/// A consumer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Consumer {
  pub id: i64,
  pub name: String,
  /// The absolute http URL its target-ping hook is served at.
  pub url: String,
  /// When it was first registered.
  pub created_at: i64,
}

impl From<store::Consumer> for Consumer {
  fn from(consumer: store::Consumer) -> Self {
    Self {
      id: consumer.id,
      name: consumer.name,
      url: consumer.url,
      created_at: consumer.created_at,
    }
  }
}

/// A consumer to register.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsumerBody {
  /// The absolute http URL its target-ping hook is served at.
  pub url: String,
}

/// A group of a consumer's targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Group {
  pub id: i64,
  pub consumer_name: String,
  pub name: String,
  /// How many of its targets one pass may ping.
  pub limit: i64,
  /// The targets that name it, by name.
  pub target_names: Vec<String>,
}

impl From<store::Group> for Group {
  fn from(group: store::Group) -> Self {
    Self {
      id: group.id,
      consumer_name: group.consumer,
      name: group.name,
      limit: group.limit,
      target_names: group.target_names,
    }
  }
}

/// A group to declare.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupBody {
  /// How many of the group's targets one pass may ping, 0 or more.
  pub limit: u32,
}

/// What an attempt answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
  Warmed,
  Busy,
  Skipped,
  Failed,
}

impl From<store::Outcome> for Outcome {
  fn from(outcome: store::Outcome) -> Self {
    match outcome {
      store::Outcome::Warmed => Self::Warmed,
      store::Outcome::Busy => Self::Busy,
      store::Outcome::Skipped => Self::Skipped,
      store::Outcome::Failed => Self::Failed,
    }
  }
}

/// A target's last attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LastAttempt {
  pub at: i64,
  pub outcome: Outcome,
  pub code: Option<String>,
  pub reason: Option<String>,
}

impl From<store::Attempt> for LastAttempt {
  fn from(attempt: store::Attempt) -> Self {
    Self {
      at: attempt.at,
      outcome: attempt.outcome.into(),
      code: attempt.code,
      reason: attempt.reason,
    }
  }
}

/// A target and when it is pinged next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Target {
  pub id: i64,
  pub consumer_name: String,
  pub name: String,
  pub kind: Option<String>,
  pub label: Option<String>,
  pub subject_name: String,
  /// Its place in the pass: lower goes first.
  pub rank: i64,
  pub group_name: Option<String>,
  /// How long the provider keeps the prefix after its last touch.
  pub ttl_ms: i64,
  /// How long it may stand idle before a ping is due.
  pub idle_ms: i64,
  /// Taken off `ttlMs` before judging.
  pub margin_ms: i64,
  /// The stretch its consumer keeps for its own reading of a ping, as registered.
  pub ping_bound_ms: Option<i64>,
  /// Whether it warms while its subject's switch is unset.
  pub warm_by_default: bool,
  pub failure_touches: bool,
  pub honours_idle_override: bool,
  /// Whether it warms now: its subject's switch where set, else `warmByDefault`.
  pub on: bool,
  /// When its subject's pause ends, while one stands.
  pub paused_until: Option<i64>,
  /// The record's last use.
  pub used_at: Option<i64>,
  /// When its last warmed answer arrived.
  pub warmed_at: Option<i64>,
  /// When its last failed attempt ended.
  pub failed_at: Option<i64>,
  pub last_attempt: Option<LastAttempt>,
  /// When it is next due by its own clocks; null while it is off, paused or past its lifetime.
  pub next_ping_at: Option<i64>,
}

impl Target {
  /// The target as listed at `now`, the policy's default standing in for a registration that says none.
  pub fn of(target: store::Target, policy_default: bool, now: i64) -> Self {
    let candidate = target.candidate(policy_default, now);

    Self {
      id: target.id,
      on: candidate.on,
      next_ping_at: candidate.due_at(now),
      paused_until: target.pause_at(now).map(|pause| pause.until),
      warm_by_default: target.warm_by_default.unwrap_or(policy_default),
      consumer_name: target.consumer,
      name: target.name,
      kind: target.kind,
      label: target.label,
      subject_name: target.subject,
      rank: target.rank,
      group_name: target.group.map(|group| group.name),
      ttl_ms: target.clocks.lifetime_ms.unwrap_or(0),
      idle_ms: target.clocks.idle_ms,
      margin_ms: target.clocks.margin_ms,
      ping_bound_ms: target.ping_bound_ms,
      failure_touches: target.clocks.failure_touches,
      honours_idle_override: target.clocks.honours_idle_override,
      used_at: target.record.last_used_at,
      warmed_at: target.record.last_warmed_at,
      failed_at: target.record.last_failed_at,
      last_attempt: target.last_attempt.map(Into::into),
    }
  }
}

/// A target to register or restate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetBody {
  /// How long the provider keeps the prefix after its last touch, 0 or more.
  pub ttl_ms: i64,
  /// How long it may stand idle before a ping is due, above 0; the policy's share of its usable lifetime
  /// when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub idle_ms: Option<i64>,
  /// Taken off `ttlMs` before judging, 0 or more; 0 when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub margin_ms: Option<i64>,
  /// A stretch the consumer keeps for its own reading of a ping, above 0, handed back in each ping; the
  /// warmer bounds nothing by it.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub ping_bound_ms: Option<i64>,
  /// The subject it is switched by; the target's own name when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub subject_name: Option<String>,
  /// The group it is in.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub group_name: Option<String>,
  /// Its place in the pass: lower goes first; 0 when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub rank: Option<i64>,
  /// The consumer's kind word for what this is, handed back in each ping.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub kind: Option<String>,
  /// A line a screen shows, in the consumer's wording.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub label: Option<String>,
  /// Whether it warms while its subject's switch is unset; the policy's default when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub warm_by_default: Option<bool>,
  /// Whether a failed attempt counts as a touch; false when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub failure_touches: Option<bool>,
  /// Whether a pass run with an idle override applies it here; true when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub honours_idle_override: Option<bool>,
  /// When it was last used, where the consumer knows; taken only where later than the record's.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub used_at: Option<i64>,
}

/// A use recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetUse {
  pub consumer_name: String,
  pub target_name: String,
  /// When the prefix was last written.
  pub at: i64,
  /// When the work that used it began.
  pub started_at: i64,
}

/// A use to record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UseBody {
  /// When the prefix was last written; the moment the use arrives when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub at: Option<i64>,
  /// When the work that used it began; `at` when absent.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub started_at: Option<i64>,
}

/// A subject of a consumer and whether it is on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
  pub id: i64,
  pub consumer_name: String,
  pub name: String,
  /// Its switch: true or false where set, null while unset.
  pub on: Option<bool>,
  /// When its pause was set, while one stands.
  pub paused_at: Option<i64>,
  /// When its pause ends, while one stands.
  pub paused_until: Option<i64>,
  /// How many targets name it.
  pub target_count: i64,
}

impl Subject {
  /// The subject as it stands at `now`.
  pub fn of(subject: store::Subject, now: i64) -> Self {
    let pause = subject.pause.filter(|pause| pause.stands(now));

    Self {
      id: subject.id,
      consumer_name: subject.consumer,
      name: subject.name,
      on: subject.switch,
      paused_at: pause.map(|pause| pause.at),
      paused_until: pause.map(|pause| pause.until),
      target_count: subject.targets,
    }
  }
}

/// A change of a subject, as a merge patch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubjectPatch {
  /// True or false throws the switch; null returns every target of the subject to its registration's
  /// default.
  #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
  #[schemars(with = "Option<bool>")]
  pub on: Option<Option<bool>>,
  /// Pauses the subject from now until then, moving a pause already standing; null lifts the pause.
  #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
  #[schemars(with = "Option<i64>")]
  pub paused_until: Option<Option<i64>>,
}

/// The warming round.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Round {
  /// How often a pass comes round.
  pub interval_ms: i64,
  /// When the next pass of the ticker is due.
  pub next_at: i64,
  /// False while every warming is stopped at once.
  pub running: bool,
}

/// A change of the round, as a merge patch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoundPatch {
  /// How often a pass comes round, above 0; null returns it to the policy's tick.
  #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
  #[schemars(with = "Option<i64>")]
  pub interval_ms: Option<Option<i64>>,
  /// False stops every warming at once; true lets the round run.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub running: Option<bool>,
}

/// A pass to run now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PassBody {
  /// Used instead of `idleMs` for every target that honours it, above 0.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub idle_override_ms: Option<i64>,
}

/// One ping a pass made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassAttempt {
  pub consumer_name: String,
  pub target_name: String,
  pub at: i64,
  pub outcome: Outcome,
  pub code: Option<String>,
  pub reason: Option<String>,
}

/// A pass that ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pass {
  /// When it ended.
  pub at: i64,
  /// How many pings went.
  pub pinged_count: i64,
  /// Whether it found the round stopped or the service stopping, so what remained was not pinged.
  pub stopped: bool,
  /// One per ping, each consumer's in the order they went, the consumers in the order of their first target
  /// in the pass.
  pub attempts: Vec<PassAttempt>,
}

/// A retired target, as its event carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetKey {
  pub id: i64,
  pub consumer_name: String,
  pub name: String,
}

/// What the warmer posts to a consumer's target-ping hook.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetPing {
  pub consumer_name: String,
  pub target_name: String,
  pub subject_name: String,
  pub kind: Option<String>,
  /// When the target fell due.
  pub due_at: i64,
  /// The target's `pingBoundMs`, where it registered one.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub bound_ms: Option<i64>,
}

/// What a consumer's target-ping hook answers with a 200.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PingAnswer {
  /// `warmed`, `busy`, `skipped` or `failed`.
  pub outcome: String,
  /// For skipped, required: a kind word saying why; for failed, optional.
  #[serde(default)]
  pub code: Option<String>,
  /// For failed, required: an English sentence.
  #[serde(default)]
  pub reason: Option<String>,
}
