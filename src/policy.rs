//! The warmer's own fixed numbers (docs/warming.md, "The policy"): `policy/warmer.toml`, built into the
//! binary and read once as the service starts.

use std::time::Duration;

use serde::Deserialize;

use crate::schedule::usable;

/// The policy file's text, built into the binary.
const TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/policy/warmer.toml"));

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The warmer's policy, as the file states it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
  pub start_delay_seconds: u64,
  pub tick_seconds: u64,
  pub warm_by_default: bool,
  pub idle_percent: u8,
  pub reason_units: usize,
  pub log_days: u64,
  pub event_days: u64,
}

/// Why a policy text did not read.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
  #[error("the policy does not read: {0}")]
  Malformed(#[from] toml::de::Error),
  #[error("the policy's {key} must be above 0")]
  Zero { key: &'static str },
  #[error("the policy's idle_percent must be 100 or less")]
  OverWhole,
}

impl Policy {
  /// The policy built into the binary.
  pub fn builtin() -> Result<Self, PolicyError> {
    Self::read(TEXT)
  }

  /// A policy from its text: an unknown or missing key, a value of the wrong type, a value of 0 where one
  /// must be above it, or a share over the whole refuses it.
  pub fn read(text: &str) -> Result<Self, PolicyError> {
    let policy: Self = toml::from_str(text)?;
    let above_zero = [
      ("tick_seconds", policy.tick_seconds),
      ("idle_percent", u64::from(policy.idle_percent)),
      ("reason_units", policy.reason_units as u64),
      ("log_days", policy.log_days),
      ("event_days", policy.event_days),
    ];
    if let Some(&(key, _)) = above_zero.iter().find(|(_, value)| *value == 0) {
      return Err(PolicyError::Zero { key });
    }
    if policy.idle_percent > 100 {
      return Err(PolicyError::OverWhole);
    }
    Ok(policy)
  }

  pub fn start_delay(&self) -> Duration {
    Duration::from_secs(self.start_delay_seconds)
  }

  pub fn tick(&self) -> Duration {
    Duration::from_secs(self.tick_seconds)
  }

  /// The round's interval while none is set, in milliseconds.
  pub fn tick_ms(&self) -> i64 {
    millis(self.tick())
  }

  /// A registration's idle stretch where it states none: the policy's share of its usable lifetime, at
  /// least one millisecond.
  pub fn idle_ms(&self, ttl_ms: i64, margin_ms: i64) -> i64 {
    let usable = usable(ttl_ms, margin_ms).unwrap_or(0);

    (usable.saturating_mul(i64::from(self.idle_percent)) / 100).max(1)
  }

  /// How long an attempt stays in the attempt log, in milliseconds.
  pub fn log_keep_ms(&self) -> i64 {
    i64::try_from(self.log_days).unwrap_or(i64::MAX).saturating_mul(DAY_MS)
  }

  /// How long an event stays in the event window, in milliseconds.
  pub fn event_keep_ms(&self) -> i64 {
    i64::try_from(self.event_days).unwrap_or(i64::MAX).saturating_mul(DAY_MS)
  }
}

fn millis(span: Duration) -> i64 {
  i64::try_from(span.as_millis()).unwrap_or(i64::MAX)
}
