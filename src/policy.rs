//! The warmer's own fixed numbers (docs/warming.md, "The policy"): `policy/warmer.toml`, built into the
//! binary and read once as the service starts.

use std::time::Duration;

use serde::Deserialize;

/// The policy file's text, built into the binary.
const TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/policy/warmer.toml"));

/// The warmer's policy, as the file states it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
  pub start_delay_seconds: u64,
  pub tick_seconds: u64,
  pub warm_by_default: bool,
  pub ping_slack_seconds: u64,
  pub reason_units: usize,
  pub log_days: u64,
}

/// Why a policy text did not read.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
  #[error("the policy does not read: {0}")]
  Malformed(#[from] toml::de::Error),
  #[error("the policy's {key} must be above 0")]
  Zero { key: &'static str },
}

impl Policy {
  /// The policy built into the binary.
  pub fn builtin() -> Result<Self, PolicyError> {
    Self::read(TEXT)
  }

  /// A policy from its text: an unknown or missing key, a value of the wrong type, or a clock of 0
  /// refuses it.
  pub fn read(text: &str) -> Result<Self, PolicyError> {
    let policy: Self = toml::from_str(text)?;
    let above_zero = [
      ("tick_seconds", policy.tick_seconds),
      ("reason_units", policy.reason_units as u64),
      ("log_days", policy.log_days),
    ];
    if let Some(&(key, _)) = above_zero.iter().find(|(_, value)| *value == 0) {
      return Err(PolicyError::Zero { key });
    }
    Ok(policy)
  }

  pub fn start_delay(&self) -> Duration {
    Duration::from_secs(self.start_delay_seconds)
  }

  pub fn tick(&self) -> Duration {
    Duration::from_secs(self.tick_seconds)
  }

  pub fn ping_slack(&self) -> Duration {
    Duration::from_secs(self.ping_slack_seconds)
  }

  /// How long an attempt stays in the attempt log, in milliseconds.
  pub fn log_keep_ms(&self) -> i64 {
    i64::try_from(self.log_days).unwrap_or(i64::MAX).saturating_mul(24 * 60 * 60 * 1000)
  }
}
