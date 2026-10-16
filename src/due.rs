//! Which targets a pass pings, and in what order (docs/warming.md, "When a target is due" and
//! "Groups"). Everything here is judged from what it is handed, `now` included, and reads no clock and no
//! store of its own.

/// The clocks a registration declares for its target, in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clocks {
  /// How long the target may stand idle before a ping is due.
  pub idle_ms: i64,
  /// How long the provider keeps the prefix after its last touch; none when no lifetime is declared.
  pub lifetime_ms: Option<i64>,
  /// Taken off the lifetime before judging.
  pub margin_ms: i64,
  /// Whether a failed attempt counts as a touch.
  pub failure_touches: bool,
  /// Whether a pass run with an idle override applies it here.
  pub honours_idle_override: bool,
}

/// What a target's record holds of its last touches, in epoch milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
  pub last_used_at: Option<i64>,
  pub last_warmed_at: Option<i64>,
  pub last_failed_at: Option<i64>,
}

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
  /// Whether the owner has stopped the round.
  pub stopped: bool,
}

/// Whether a target warms: its subject's switch where thrown, else its registration's default, else the
/// policy's.
pub fn is_on(switch: Option<bool>, warm_by_default: Option<bool>, policy_default: bool) -> bool {
  switch.or(warm_by_default).unwrap_or(policy_default)
}

/// Whether a use report clears a pause: only work that began after the pause was set does.
pub fn use_clears_pause(paused_at: i64, started_at: i64) -> bool {
  started_at > paused_at
}

/// The usable part of a lifetime: the margin taken off, and none where nothing is left of it.
pub fn usable(lifetime_ms: i64, margin_ms: i64) -> Option<i64> {
  let usable = lifetime_ms.saturating_sub(margin_ms);

  (usable > 0).then_some(usable)
}

/// The first `units` of a text, by the UTF-16 units a script counts; a surrogate pair the cut splits ends
/// as the replacement character.
pub fn first_units(text: &str, units: usize) -> String {
  let all: Vec<u16> = text.encode_utf16().collect();

  String::from_utf16_lossy(&all[..units.min(all.len())])
}

impl Clocks {
  /// The later of the last use and the last warmed answer, and the last failed attempt where failures
  /// touch; none when the target was never used or pinged.
  pub fn touch(&self, record: &Record) -> Option<i64> {
    let failed = record.last_failed_at.filter(|_| self.failure_touches);

    [record.last_used_at, record.last_warmed_at, failed].into_iter().flatten().max()
  }

  /// How long the target may stand idle in a pass with this override.
  pub fn idle_stretch(&self, idle_override_ms: Option<i64>) -> i64 {
    idle_override_ms.filter(|_| self.honours_idle_override).unwrap_or(self.idle_ms)
  }

  /// Whether the clocks alone make the target due at `now`: never touched, or idle at least its stretch
  /// while its prefix is still there.
  pub fn due(&self, record: &Record, now: i64, idle_override_ms: Option<i64>) -> bool {
    let Some(touch) = self.touch(record) else {
      return self.lifetime_ms.is_none_or(|lifetime| usable(lifetime, self.margin_ms).is_some());
    };
    let idle = now.saturating_sub(touch);

    idle >= self.idle_stretch(idle_override_ms) && self.still_there(idle)
  }

  /// When the clocks alone next make the target due, judged without an override: `now` when it was never
  /// touched, none when its prefix is gone or no lifetime is left of it.
  pub fn due_at(&self, record: &Record, now: i64) -> Option<i64> {
    let Some(touch) = self.touch(record) else {
      return self.due(record, now, None).then_some(now);
    };
    let at = touch.saturating_add(self.idle_ms);

    (self.still_there(now.saturating_sub(touch)) && self.still_there(self.idle_ms)).then_some(at)
  }

  // Whether a prefix idle this long is still there: below the usable lifetime where one is declared.
  fn still_there(&self, idle: i64) -> bool {
    match self.lifetime_ms {
      None => true,
      Some(lifetime) => usable(lifetime, self.margin_ms).is_some_and(|usable| idle < usable),
    }
  }
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
