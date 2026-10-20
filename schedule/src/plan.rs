//! Which targets a pass pings, and in what order (docs/warming.md, 「Groups」): each candidate due by its
//! switch, its pause, the round and its clocks, by rank, in groups up to their limit.

use super::clocks::{Clocks, Record};

/// A group a target is in: only `limit` of its due targets are pinged in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
  pub name: String,
  pub limit: usize,
}

/// One registered target as a pass judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
  pub consumer: String,
  pub target: String,
  pub rank: i64,
  pub group: Option<Group>,
  pub clocks: Clocks,
  pub record: Record,
  /// Whether it warms now, as `is_on` reads its subject's switch and its default.
  pub on: bool,
  /// Whether its subject has a pause standing.
  pub paused: bool,
}

/// What one pass judges by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
  pub now: i64,
  /// Used instead of `idle_ms` for every target that honours it.
  pub idle_override_ms: Option<i64>,
  /// Whether the round is stopped.
  pub stopped: bool,
}

impl Candidate {
  /// Whether the target is due in this pass: on, not paused, the round running, and due by its clocks.
  pub fn is_due(&self, pass: &Pass) -> bool {
    self.on
      && !self.paused
      && !pass.stopped
      && self.clocks.due(&self.record, pass.now, pass.idle_override_ms)
  }

  /// When the target is next due, for the list: none while it is off or paused.
  pub fn due_at(&self, now: i64) -> Option<i64> {
    if !self.on || self.paused {
      return None;
    }
    self.clocks.due_at(&self.record, now)
  }

  fn touch(&self) -> Option<i64> {
    self.clocks.touch(&self.record)
  }

  fn group_key(&self) -> Option<(&str, &str)> {
    self.group.as_ref().map(|group| (self.consumer.as_str(), group.name.as_str()))
  }
}

/// The targets a pass pings, as indexes into `candidates`, in the order it pings them. Targets go by
/// rank, then consumer, then target name. A group goes, together, where its first target stands in that
/// order; of its due targets the oldest touch goes first, a target never touched oldest of all, up to the
/// group's limit. A stopped round pings nothing.
pub fn plan(candidates: &[Candidate], pass: &Pass) -> Vec<usize> {
  if pass.stopped {
    return Vec::new();
  }

  let mut order: Vec<usize> = (0..candidates.len()).collect();
  order.sort_by(|&a, &b| {
    let (a, b) = (&candidates[a], &candidates[b]);
    (a.rank, &a.consumer, &a.target).cmp(&(b.rank, &b.consumer, &b.target))
  });

  let mut placed: Vec<(&str, &str)> = Vec::new();
  let mut pings = Vec::new();

  for &index in &order {
    let candidate = &candidates[index];
    let Some(key) = candidate.group_key() else {
      if candidate.is_due(pass) {
        pings.push(index);
      }
      continue;
    };
    if placed.contains(&key) {
      continue;
    }
    placed.push(key);

    // `order` is stable, so targets of equal touch keep their rank order.
    let mut members: Vec<usize> = order
      .iter()
      .copied()
      .filter(|&other| candidates[other].group_key() == Some(key) && candidates[other].is_due(pass))
      .collect();
    members.sort_by_key(|&member| candidates[member].touch());

    let limit = candidate.group.as_ref().map_or(0, |group| group.limit);
    pings.extend(members.into_iter().take(limit));
  }

  pings
}
