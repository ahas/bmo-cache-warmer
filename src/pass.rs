//! The warmer (docs/warming.md, "The pass" and "What an answer leaves behind"): judge which targets are
//! due, post each one's ping to its consumer's hook on that consumer's one line, waiting for each answer
//! however long it takes, and record what came back; and the events every change publishes (docs/api.md,
//! "Events").

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::join_all;
use serde::Serialize;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::clock::Clock;
use crate::policy::Policy;
use crate::schedule::{self, Pass as Judged};
use crate::stop::Stop;
use crate::store::{Attempt, Outcome, Store, StoreError, Target};
use crate::ticker::Beat;
use crate::wire;
use crate::words::is_kind;

/// The store, the policy and the clock one pass at a time runs on, the round's beat and the service's stop.
pub struct Warmer {
  store: Store,
  policy: Policy,
  clock: Arc<dyn Clock>,
  client: reqwest::Client,
  lines: Mutex<HashSet<String>>,
  beat: Beat,
  published: watch::Sender<i64>,
  stop: Stop,
}

/// A consumer's line, held by the pass that claimed it until that pass is done with it.
struct Line<'a> {
  consumer: String,
  held: &'a Mutex<HashSet<String>>,
}

impl Drop for Line<'_> {
  fn drop(&mut self) {
    self.held.lock().expect("the lines' set is never left poisoned").remove(&self.consumer);
  }
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
    // A ping goes to the hook URL itself: no proxy from the environment, and a redirect is an answer. No
    // clock ends a ping; the client's own keep-alive probes tell a hook host that has gone.
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
      lines: Mutex::new(HashSet::new()),
      beat,
      published,
      stop: Stop::new(),
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

  /// The service's stop, which ends every line before its next ping once begun.
  pub fn stop(&self) -> &Stop {
    &self.stop
  }

  /// Whether a pass or a line ends here: the service is stopping, or the round is found stopped.
  async fn halted(&self) -> Result<bool, StoreError> {
    Ok(self.stop.has_begun() || self.store.round_stopped().await?)
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

  /// Runs a pass now, beside any other running, over the consumers whose line is free: the pass, its event,
  /// then the log and the window pruned to their days, whether or not the pass found the round stopped.
  pub async fn pass(&self, idle_override_ms: Option<i64>) -> Result<wire::Pass, StoreError> {
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

  /// Claims the line of each consumer named whose line no pass holds.
  fn claim(&self, consumers: BTreeSet<String>) -> Vec<Line<'_>> {
    let mut held = self.lines.lock().expect("the lines' set is never left poisoned");

    consumers
      .into_iter()
      .filter(|consumer| held.insert(consumer.clone()))
      .map(|consumer| Line {
        consumer,
        held: &self.lines,
      })
      .collect()
  }

  /// Pings every due target of the consumers whose line it claims, each consumer's on its own line in the
  /// plan's order and the lines side by side: whether the round was found stopped or the service stopping,
  /// and each ping.
  async fn ping_due(
    &self,
    idle_override_ms: Option<i64>,
  ) -> Result<(bool, Vec<wire::PassAttempt>), StoreError> {
    if self.halted().await? {
      return Ok((true, Vec::new()));
    }

    // The lines are claimed before the targets are read, so each claimed consumer is judged from a record
    // its last line out has already written.
    let named = self.store.all_targets().await?.into_iter().map(|target| target.consumer).collect();
    let claimed = self.claim(named);
    let default = self.policy.warm_by_default;
    let now = self.clock.now();
    let targets = self.store.all_targets().await?;
    let candidates: Vec<_> = targets.iter().map(|target| target.candidate(default, now)).collect();
    let pass = Judged {
      now,
      idle_override_ms,
      stopped: false,
    };

    let mut lines: Vec<(Line<'_>, Vec<usize>)> = Vec::new();
    let mut free = claimed;
    for index in schedule::plan(&candidates, &pass) {
      let consumer = &targets[index].consumer;
      if let Some((_, planned)) = lines.iter_mut().find(|(line, _)| &line.consumer == consumer) {
        planned.push(index);
      } else if let Some(at) = free.iter().position(|line| &line.consumer == consumer) {
        lines.push((free.swap_remove(at), vec![index]));
      }
    }
    // A claimed line with nothing due is given back before any ping goes.
    drop(free);

    let ran = join_all(lines.into_iter().map(|(line, planned)| {
      let due: Vec<_> = planned
        .into_iter()
        .map(|index| (&targets[index], candidates[index].due_at(pass.now).unwrap_or(pass.now)))
        .collect();
      self.walk(line, due)
    }))
    .await;

    let mut stopped = false;
    let mut attempts = Vec::new();
    for walked in ran {
      let (found_stopped, pinged) = walked?;
      stopped |= found_stopped;
      attempts.extend(pinged);
    }
    Ok((stopped, attempts))
  }

  /// Walks one consumer's due targets on its line, one ping at a time, each waiting for its answer:
  /// whether it found the round stopped or the service stopping, and each ping. The line is given back as
  /// the walk ends.
  async fn walk(
    &self,
    _line: Line<'_>,
    due: Vec<(&Target, i64)>,
  ) -> Result<(bool, Vec<wire::PassAttempt>), StoreError> {
    let default = self.policy.warm_by_default;
    let mut attempts = Vec::new();

    for (planned, due_at) in due {
      if self.halted().await? {
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

  /// Posts one ping and judges what came back, waiting for the answer however long the hook takes.
  async fn ping(&self, hook_url: &str, target: &Target, due_at: i64) -> Attempt {
    let ping = wire::TargetPing {
      consumer_name: target.consumer.clone(),
      target_name: target.name.clone(),
      subject_name: target.subject.clone(),
      kind: target.kind.clone(),
      due_at,
      bound_ms: target.ping_bound_ms,
    };

    let heard = match self.call(hook_url, &ping).await {
      Ok(heard) => heard,
      Err(fault) => fault.into(),
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
