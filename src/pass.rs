//! The warmer (docs/warming.md, "The pass" and "What an answer leaves behind"): judge which targets are
//! due, post each one's ping to its consumer's hook in turn, bounded, and record what came back; and the
//! events every change publishes (docs/api.md, "Events").

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use crate::clock::Clock;
use crate::policy::Policy;
use crate::schedule::{self, Pass as Judged};
use crate::store::{Attempt, Outcome, Store, StoreError, Target};
use crate::ticker::Beat;
use crate::wire;
use crate::words::is_kind;

/// The store, the policy and the clock one pass at a time runs on, and the round's beat.
pub struct Warmer {
  store: Store,
  policy: Policy,
  clock: Arc<dyn Clock>,
  client: reqwest::Client,
  running: Mutex<()>,
  beat: Beat,
  published: watch::Sender<i64>,
}

/// Why a pass did not run or did not finish.
#[derive(Debug, thiserror::Error)]
pub enum PassError {
  #[error("A pass is already running.")]
  Running,
  #[error(transparent)]
  Store(#[from] StoreError),
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
  fn warmed() -> Self {
    Self {
      outcome: Outcome::Warmed,
      code: None,
      reason: None,
    }
  }

  fn of(answer: wire::PingAnswer) -> Result<Self, Fault> {
    let outcome = Outcome::parse(&answer.outcome).ok_or_else(|| {
      Fault::new("hook-answer", format!("The hook answered the outcome {:?}.", answer.outcome))
    })?;
    let code_shaped = answer.code.as_deref().is_none_or(is_kind);

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
        Err(Fault::new("hook-answer", "The hook answered skipped without a kind word as its code."))
      }
      Outcome::Failed if !code_shaped => Err(Fault::new(
        "hook-answer",
        "The hook answered failed with a code that is not a kind word.",
      )),
      Outcome::Failed => match answer.reason.filter(|reason| !reason.trim().is_empty()) {
        Some(reason) => Ok(Self {
          outcome,
          code: answer.code,
          reason: Some(reason),
        }),
        None => Err(Fault::new("hook-answer", "The hook answered failed without a reason.")),
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
  /// The warmer over an open store, its round's interval read from it.
  pub async fn open(store: Store, policy: Policy, clock: Arc<dyn Clock>) -> Result<Self, StoreError> {
    // A ping goes to the hook URL itself: no proxy from the environment, and a redirect is an answer.
    let client = reqwest::Client::builder()
      .no_proxy()
      .redirect(reqwest::redirect::Policy::none())
      .build()
      .expect("a plain HTTP client builds");
    let interval_ms = store.round().await?.interval_ms.unwrap_or_else(|| policy.tick_ms());
    let beat = Beat::new(policy.start_delay(), duration(interval_ms));
    let (published, _) = watch::channel(store.next_event_id().await?.saturating_sub(1));

    Ok(Self {
      store,
      policy,
      clock,
      client,
      running: Mutex::new(()),
      beat,
      published,
    })
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

  pub fn beat(&self) -> &Beat {
    &self.beat
  }

  /// Keeps an event carrying `data` and tells every reader of the stream.
  pub async fn publish(&self, kind: &str, data: &impl Serialize) -> Result<i64, StoreError> {
    let data = serde_json::to_string(data).expect("a member writes as JSON");
    let id = self.store.append_event(kind, self.clock.now(), &data).await?;

    self.published.send_replace(id);
    Ok(id)
  }

  /// The number of the last event published, changing as each is.
  pub fn published(&self) -> watch::Receiver<i64> {
    self.published.subscribe()
  }

  /// A target as listed now; none when there is no such target.
  pub async fn target(&self, consumer: &str, target: &str) -> Result<Option<wire::Target>, StoreError> {
    let now = self.clock.now();

    Ok(self.store.target(consumer, target).await?.map(|found| self.listed(found, now)))
  }

  /// A target as listed at `now`.
  pub fn listed(&self, target: Target, now: i64) -> wire::Target {
    wire::Target::of(target, self.policy.warm_by_default, now)
  }

  /// Publishes a target's change, where it still stands.
  pub async fn target_changed(&self, consumer: &str, target: &str) -> Result<(), StoreError> {
    if let Some(member) = self.target(consumer, target).await? {
      self.publish("target.changed", &member).await?;
    }
    Ok(())
  }

  /// The round as it stands now.
  pub async fn round(&self) -> Result<wire::Round, StoreError> {
    let kept = self.store.round().await?;
    let until_next = self.beat.next().saturating_duration_since(Instant::now());

    Ok(wire::Round {
      interval_ms: kept.interval_ms.unwrap_or_else(|| self.policy.tick_ms()),
      next_at: self.clock.now().saturating_add(millis(until_next)),
      running: !kept.stopped,
    })
  }

  /// Takes a changed interval into the beat: the one set, or the policy's tick where none is.
  pub fn interval_changed(&self, interval_ms: Option<i64>) {
    self.beat.set_interval(duration(interval_ms.unwrap_or_else(|| self.policy.tick_ms())));
  }

  /// Runs a pass now, or refuses while one runs.
  pub async fn try_pass(&self, idle_override_ms: Option<i64>) -> Result<wire::Pass, PassError> {
    let _running = self.running.try_lock().map_err(|_| PassError::Running)?;

    Ok(self.run(idle_override_ms).await?)
  }

  /// Runs a pass once the one running, if any, has ended.
  pub async fn pass(&self, idle_override_ms: Option<i64>) -> Result<wire::Pass, StoreError> {
    let _running = self.running.lock().await;

    self.run(idle_override_ms).await
  }

  /// A pass, its event, then the log and the window pruned to their days, whether or not the pass found
  /// the round stopped.
  async fn run(&self, idle_override_ms: Option<i64>) -> Result<wire::Pass, StoreError> {
    let (stopped, attempts) = self.ping_due(idle_override_ms).await?;
    let pass = wire::Pass {
      at: self.clock.now(),
      pinged_count: i64::try_from(attempts.len()).unwrap_or(i64::MAX),
      stopped,
      attempts,
    };

    self.publish("pass.created", &pass).await?;
    let now = self.clock.now();
    self.store.prune_attempts(now.saturating_sub(self.policy.log_keep_ms())).await?;
    self.store.prune_events(now.saturating_sub(self.policy.event_keep_ms())).await?;
    Ok(pass)
  }

  /// Pings every due target in the plan's order: whether the round was found stopped, and each ping.
  async fn ping_due(
    &self,
    idle_override_ms: Option<i64>,
  ) -> Result<(bool, Vec<wire::PassAttempt>), StoreError> {
    let mut attempts = Vec::new();
    if self.store.round_stopped().await? {
      return Ok((true, attempts));
    }

    let default = self.policy.warm_by_default;
    let now = self.clock.now();
    let targets = self.store.all_targets().await?;
    let candidates: Vec<_> = targets.iter().map(|target| target.candidate(default, now)).collect();
    let pass = Judged {
      now,
      idle_override_ms,
      stopped: false,
    };

    for index in schedule::plan(&candidates, &pass) {
      let planned = &targets[index];
      if self.store.round_stopped().await? {
        return Ok((true, attempts));
      }
      let Some(target) = self.store.target(&planned.consumer, &planned.name).await? else {
        continue;
      };
      let now = self.clock.now();
      if !target.on(default) || target.pause_at(now).is_some() {
        continue;
      }
      let Some(hook_url) = self.store.hook_url(&target.consumer).await? else {
        continue;
      };

      let due_at = candidates[index].due_at(pass.now).unwrap_or(pass.now);
      let attempt = self.ping(&hook_url, &target, due_at).await;
      if self.store.record_attempt(&target.consumer, &target.name, &attempt).await? {
        self.target_changed(&target.consumer, &target.name).await?;
        attempts.push(wire::PassAttempt {
          consumer_name: target.consumer,
          target_name: target.name,
          at: attempt.at,
          outcome: attempt.outcome.into(),
          code: attempt.code,
          reason: attempt.reason,
        });
      }
    }
    Ok((false, attempts))
  }

  /// Posts one ping and judges what came back, within the target's bound and the policy's slack.
  async fn ping(&self, hook_url: &str, target: &Target, due_at: i64) -> Attempt {
    let bound =
      Duration::from_millis(u64::try_from(target.ping_bound_ms).unwrap_or(0)) + self.policy.ping_slack();
    let ping = wire::TargetPing {
      consumer_name: target.consumer.clone(),
      target_name: target.name.clone(),
      subject_name: target.subject.clone(),
      kind: target.kind.clone(),
      due_at,
      bound_ms: target.ping_bound_ms,
    };

    let heard = match tokio::time::timeout(bound, self.call(hook_url, &ping)).await {
      Ok(Ok(heard)) => heard,
      Ok(Err(fault)) => fault.into(),
      Err(_) => {
        Fault::new("hook-timeout", format!("The hook did not answer within {} ms.", bound.as_millis()))
          .into()
      }
    };

    Attempt {
      at: self.clock.now(),
      outcome: heard.outcome,
      code: heard.code,
      reason: heard.reason.map(|reason| schedule::first_units(&reason, self.policy.reason_units)),
    }
  }

  async fn call(&self, hook_url: &str, ping: &wire::TargetPing) -> Result<Heard, Fault> {
    let response = self.client.post(hook_url).json(ping).send().await.map_err(|error| {
      let error = error.without_url();
      if error.is_connect() {
        Fault::new("hook-unreachable", format!("The hook could not be connected to: {error}."))
      } else {
        Fault::new("hook-unreachable", format!("The hook's answer did not arrive: {error}."))
      }
    })?;

    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
      return Ok(Heard::warmed());
    }
    if status != reqwest::StatusCode::OK {
      return Err(Fault::new("hook-status", format!("The hook answered status {}.", status.as_u16())));
    }
    let body = response.bytes().await.map_err(|error| {
      Fault::new("hook-answer", format!("The hook's answer broke off: {}.", error.without_url()))
    })?;
    let answer: wire::PingAnswer = serde_json::from_slice(&body)
      .map_err(|error| Fault::new("hook-answer", format!("The hook's answer does not read: {error}.")))?;

    Heard::of(answer)
  }
}

fn duration(ms: i64) -> Duration {
  Duration::from_millis(u64::try_from(ms).unwrap_or(1).max(1))
}

fn millis(span: Duration) -> i64 {
  i64::try_from(span.as_millis()).unwrap_or(i64::MAX)
}
