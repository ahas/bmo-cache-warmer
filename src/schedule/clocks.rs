//! When a target is due by its clocks alone (docs/warming.md, 「When a target is due」): its touch, its idle
//! stretch, and whether its prefix is still there.

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

/// The usable part of a lifetime: the margin taken off, and none where nothing is left of it.
pub fn usable(lifetime_ms: i64, margin_ms: i64) -> Option<i64> {
  let usable = lifetime_ms.saturating_sub(margin_ms);

  (usable > 0).then_some(usable)
}

impl Clocks {
  /// The latest of the last use and the last warmed answer, and the last failed attempt where failures
  /// touch; none when the target was never used or pinged.
  pub fn touch(&self, record: &Record) -> Option<i64> {
    let failed = record.last_failed_at.filter(|_| self.failure_touches);

    [record.last_used_at, record.last_warmed_at, failed].into_iter().flatten().max()
  }

  /// How long the target may stand idle in a pass with this override.
  pub fn idle_stretch(&self, idle_override_ms: Option<i64>) -> i64 {
    idle_override_ms.filter(|_| self.honours_idle_override).unwrap_or(self.idle_ms)
  }

  /// Whether the clocks alone make the target due at `now`: never touched, or idle at least its stretch,
  /// while its prefix is still there.
  pub fn due(&self, record: &Record, now: i64, idle_override_ms: Option<i64>) -> bool {
    let Some(touch) = self.touch(record) else {
      return self.lifetime_ms.is_none_or(|lifetime| usable(lifetime, self.margin_ms).is_some());
    };
    let idle = now.saturating_sub(touch);

    idle >= self.idle_stretch(idle_override_ms) && self.kept(idle)
  }

  /// When the clocks alone next make the target due, judged without an override: `now` when it was never
  /// touched, none when its prefix is gone or no lifetime is left of it.
  pub fn due_at(&self, record: &Record, now: i64) -> Option<i64> {
    let Some(touch) = self.touch(record) else {
      return self.due(record, now, None).then_some(now);
    };
    let at = touch.saturating_add(self.idle_ms);

    (self.kept(now.saturating_sub(touch)) && self.kept(self.idle_ms)).then_some(at)
  }

  /// Whether a prefix idle this long is still there: below the usable lifetime where one is declared.
  pub fn kept(&self, idle_ms: i64) -> bool {
    match self.lifetime_ms {
      None => true,
      Some(lifetime) => usable(lifetime, self.margin_ms).is_some_and(|usable| idle_ms < usable),
    }
  }
}
