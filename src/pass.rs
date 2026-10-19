//! The pass (docs/warming.md, "The pass" and "What an answer leaves behind"): judge which targets are
//! due, post each one's ping to its consumer's hook in turn, bounded, and record what came back.

use std::sync::Arc;
use std::time::Duration;

use cache_warmer::Pass;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::clock::Clock;
use crate::policy::Policy;
use crate::store::{Attempt, Outcome, Store, StoreError, Target};
use crate::words::is_snake_case;

/// The store, the policy and the clock one pass at a time runs on.
pub struct Warmer {
  store: Store,
  policy: Policy,
  clock: Arc<dyn Clock>,
  client: reqwest::Client,
  running: Mutex<()>,
}

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
  /// Whether the pass found the round stopped, at its start or before a ping.
  pub stopped: bool,
  /// One entry per ping, in the order they went.
  pub attempts: Vec<Pinged>,
}

/// One ping a pass made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinged {
  pub consumer: String,
  pub target: String,
  pub attempt: Attempt,
}

/// Why a pass did not run or did not finish.
#[derive(Debug, thiserror::Error)]
pub enum PassError {
  #[error("A pass is already running.")]
  Running,
  #[error(transparent)]
  Store(#[from] StoreError),
}

/// What the warmer posts to a hook.
#[derive(Serialize)]
struct Ping<'a> {
  consumer: &'a str,
  target: &'a str,
  kind: &'a str,
  subject: &'a str,
  bound_ms: i64,
}

/// What a hook answers, before it is judged.
#[derive(Deserialize)]
struct Answer {
  outcome: String,
  #[serde(default)]
  code: Option<String>,
  #[serde(default)]
  reason: Option<String>,
}

/// A hook that failed by the warmer's own judging: its code and an English reason.
struct Fault {
  code: &'static str,
  reason: String,
}

impl Fault {
  fn new(code: &'static str, reason: impl Into<String>) -> Self {
    Self {
      code,
      reason: reason.into(),
    }
  }
}

/// What an answer records, before its time and the reason's cut.
struct Heard {
  outcome: Outcome,
  code: Option<String>,
  reason: Option<String>,
}

impl Heard {
  fn of(answer: Answer) -> Result<Self, Fault> {
    let outcome = Outcome::parse(&answer.outcome).ok_or_else(|| {
      Fault::new("hook_answer", format!("The hook answered the outcome {:?}.", answer.outcome))
    })?;
    let code_shaped = answer.code.as_deref().is_none_or(is_snake_case);

    match outcome {
      Outcome::Warmed | Outcome::Busy => Ok(Self {
        outcome,
        code: None,
        reason: None,
      }),
      Outcome::Skipped if answer.code.is_some() && code_shaped => Ok(Self {
        outcome,
        code: answer.code,
        reason: None,
      }),
      Outcome::Skipped => {
        Err(Fault::new("hook_answer", "The hook answered skipped without a snake_case code."))
      }
      Outcome::Failed if !code_shaped => Err(Fault::new(
        "hook_answer",
        "The hook answered failed with a code that is not snake_case.",
      )),
      Outcome::Failed => match answer.reason.filter(|reason| !reason.trim().is_empty()) {
        Some(reason) => Ok(Self {
          outcome,
          code: answer.code,
          reason: Some(reason),
        }),
        None => Err(Fault::new("hook_answer", "The hook answered failed without a reason.")),
      },
    }
  }
}

impl From<Fault> for Heard {
  fn from(fault: Fault) -> Self {
    Self {
      outcome: Outcome::Failed,
      code: Some(fault.code.to_owned()),
      reason: Some(fault.reason),
    }
  }
}

impl Warmer {
  pub fn new(store: Store, policy: Policy, clock: Arc<dyn Clock>) -> Self {
    // A ping goes to the hook URL itself: no proxy from the environment, and a redirect is an answer.
    let client = reqwest::Client::builder()
      .no_proxy()
      .redirect(reqwest::redirect::Policy::none())
      .build()
      .expect("a plain HTTP client builds");

    Self {
      store,
      policy,
      clock,
      client,
      running: Mutex::new(()),
    }
  }

  pub fn store(&self) -> &Store {
    &self.store
  }

  pub fn policy(&self) -> &Policy {
    &self.policy
  }

  pub fn clock(&self) -> &dyn Clock {
    self.clock.as_ref()
  }

  /// Runs a pass now, or refuses while one runs.
  pub async fn try_pass(&self, idle_override_ms: Option<i64>) -> Result<Report, PassError> {
    let _running = self.running.try_lock().map_err(|_| PassError::Running)?;

    Ok(self.run(idle_override_ms).await?)
  }

  /// Runs a pass once the one running, if any, has ended.
  pub async fn pass(&self, idle_override_ms: Option<i64>) -> Result<Report, StoreError> {
    let _running = self.running.lock().await;

    self.run(idle_override_ms).await
  }

  /// A pass, then the log pruned to its days, whether or not the pass found the round stopped.
  async fn run(&self, idle_override_ms: Option<i64>) -> Result<Report, StoreError> {
    let report = self.ping_due(idle_override_ms).await?;

    self
      .store
      .prune_attempts(self.clock.now().saturating_sub(self.policy.log_keep_ms()))
      .await?;
    Ok(report)
  }

  async fn ping_due(&self, idle_override_ms: Option<i64>) -> Result<Report, StoreError> {
    let mut report = Report::default();
    if self.store.round_stopped().await? {
      report.stopped = true;
      return Ok(report);
    }

    let default = self.policy.warm_by_default;
    let targets = self.store.targets(None, None).await?;
    let candidates: Vec<_> = targets.iter().map(|target| target.candidate(default)).collect();
    let pass = Pass {
      now: self.clock.now(),
      idle_override_ms,
      stopped: false,
    };

    for index in cache_warmer::plan(&candidates, &pass) {
      let planned = &targets[index];
      if self.store.round_stopped().await? {
        report.stopped = true;
        break;
      }
      let Some(target) = self.store.target(&planned.consumer, &planned.name).await? else {
        continue;
      };
      if !target.on(default) || target.paused_at.is_some() {
        continue;
      }
      let Some(hook_url) = self.store.hook_url(&target.consumer).await? else {
        continue;
      };

      let attempt = self.ping(&hook_url, &target).await;
      if self.store.record_attempt(&target.consumer, &target.name, &attempt).await? {
        report.attempts.push(Pinged {
          consumer: target.consumer,
          target: target.name,
          attempt,
        });
      }
    }
    Ok(report)
  }

  /// Posts one ping and judges what came back, within the target's bound and the policy's slack.
  async fn ping(&self, hook_url: &str, target: &Target) -> Attempt {
    let bound =
      Duration::from_millis(u64::try_from(target.ping_bound_ms).unwrap_or(0)) + self.policy.ping_slack();
    let ping = Ping {
      consumer: &target.consumer,
      target: &target.name,
      kind: &target.kind,
      subject: &target.subject,
      bound_ms: target.ping_bound_ms,
    };

    let heard = match tokio::time::timeout(bound, self.call(hook_url, &ping)).await {
      Ok(Ok(heard)) => heard,
      Ok(Err(fault)) => fault.into(),
      Err(_) => {
        Fault::new("hook_timeout", format!("The hook did not answer within {} ms.", bound.as_millis()))
          .into()
      }
    };

    Attempt {
      at: self.clock.now(),
      outcome: heard.outcome,
      code: heard.code,
      reason: heard.reason.map(|reason| cache_warmer::first_units(&reason, self.policy.reason_units)),
    }
  }

  async fn call(&self, hook_url: &str, ping: &Ping<'_>) -> Result<Heard, Fault> {
    let response = self.client.post(hook_url).json(ping).send().await.map_err(|error| {
      let error = error.without_url();
      if error.is_connect() {
        Fault::new("hook_unreachable", format!("The hook could not be connected to: {error}."))
      } else {
        Fault::new("hook_unreachable", format!("The hook's answer did not arrive: {error}."))
      }
    })?;

    let status = response.status();
    if status != reqwest::StatusCode::OK {
      return Err(Fault::new("hook_status", format!("The hook answered status {}.", status.as_u16())));
    }
    let body = response.bytes().await.map_err(|error| {
      Fault::new("hook_answer", format!("The hook's answer broke off: {}.", error.without_url()))
    })?;
    let answer: Answer = serde_json::from_slice(&body)
      .map_err(|error| Fault::new("hook_answer", format!("The hook's answer does not read: {error}.")))?;

    Heard::of(answer)
  }
}
