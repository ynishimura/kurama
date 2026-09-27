//! Session duration value object
//!
//! This module defines a validated AWS session duration.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;
use thiserror::Error;

/// Minimum session duration: 15 minutes (900 seconds)
pub const MIN_DURATION_SECS: u64 = 900;

/// Maximum session duration: 12 hours (43200 seconds)
pub const MAX_DURATION_SECS: u64 = 43200;

/// Error type for session duration validation
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum SessionDurationError {
    #[error("Session duration {0} seconds is below minimum ({MIN_DURATION_SECS} seconds)")]
    BelowMinimum(u64),

    #[error("Session duration {0} seconds exceeds maximum ({MAX_DURATION_SECS} seconds)")]
    ExceedsMaximum(u64),
}

/// Session duration value object
///
/// Represents a validated AWS session duration.
/// Valid range: 15 minutes (900s) to 12 hours (43200s)
///
/// # Example
///
/// ```
/// use kurama::domain::types::SessionDuration;
///
/// // Create with validation
/// let duration = SessionDuration::new(3600).unwrap();
/// assert_eq!(duration.as_secs(), 3600);
///
/// // Use default (1 hour)
/// let default = SessionDuration::default();
/// assert_eq!(default.as_secs(), 3600);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionDuration(u64);

impl SessionDuration {
    /// Create a new SessionDuration with validation
    ///
    /// # Arguments
    ///
    /// * `seconds` - Duration in seconds (must be between 900 and 43200)
    ///
    /// # Returns
    ///
    /// Returns `Ok(SessionDuration)` if valid, or `Err(SessionDurationError)` if out of range.
    pub fn new(seconds: u64) -> Result<Self, SessionDurationError> {
        if seconds < MIN_DURATION_SECS {
            return Err(SessionDurationError::BelowMinimum(seconds));
        }
        if seconds > MAX_DURATION_SECS {
            return Err(SessionDurationError::ExceedsMaximum(seconds));
        }
        Ok(Self(seconds))
    }

    /// Create a SessionDuration without validation
    ///
    /// Use with caution - only when you're certain the value is valid.
    pub const fn new_unchecked(seconds: u64) -> Self {
        Self(seconds)
    }

    /// Create a SessionDuration from minutes
    pub fn from_minutes(minutes: u64) -> Result<Self, SessionDurationError> {
        Self::new(minutes * 60)
    }

    /// Create a SessionDuration from hours
    pub fn from_hours(hours: u64) -> Result<Self, SessionDurationError> {
        Self::new(hours * 3600)
    }

    /// Get the duration in seconds
    pub const fn as_secs(&self) -> u64 {
        self.0
    }

    /// Get the duration in minutes
    pub const fn as_minutes(&self) -> u64 {
        self.0 / 60
    }

    /// Get the duration in hours (rounded down)
    pub const fn as_hours(&self) -> u64 {
        self.0 / 3600
    }

    /// Convert to std::time::Duration
    pub const fn to_std_duration(&self) -> Duration {
        Duration::from_secs(self.0)
    }

    /// Convert to i32 (for AWS SDK compatibility)
    pub const fn as_i32(&self) -> i32 {
        self.0 as i32
    }

    // Common durations as constants
    /// 15 minutes (minimum)
    pub const MIN: Self = Self(MIN_DURATION_SECS);
    /// 30 minutes
    pub const THIRTY_MINUTES: Self = Self(1800);
    /// 1 hour (default)
    pub const ONE_HOUR: Self = Self(3600);
    /// 2 hours
    pub const TWO_HOURS: Self = Self(7200);
    /// 4 hours
    pub const FOUR_HOURS: Self = Self(14400);
    /// 8 hours
    pub const EIGHT_HOURS: Self = Self(28800);
    /// 12 hours (maximum)
    pub const MAX: Self = Self(MAX_DURATION_SECS);
}

impl Default for SessionDuration {
    fn default() -> Self {
        Self::ONE_HOUR
    }
}

impl fmt::Display for SessionDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hours = self.0 / 3600;
        let minutes = (self.0 % 3600) / 60;

        if hours > 0 && minutes > 0 {
            write!(f, "{}h {}m", hours, minutes)
        } else if hours > 0 {
            write!(f, "{}h", hours)
        } else {
            write!(f, "{}m", minutes)
        }
    }
}

impl From<SessionDuration> for Duration {
    fn from(session_duration: SessionDuration) -> Self {
        session_duration.to_std_duration()
    }
}

impl From<SessionDuration> for u64 {
    fn from(session_duration: SessionDuration) -> Self {
        session_duration.0
    }
}

impl From<SessionDuration> for i32 {
    fn from(session_duration: SessionDuration) -> Self {
        session_duration.0 as i32
    }
}

impl TryFrom<u64> for SessionDuration {
    type Error = SessionDurationError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let default = SessionDuration::default();
        assert_eq!(default.as_secs(), 3600);
    }

    #[test]
    fn test_valid_durations() {
        // Minimum
        let min = SessionDuration::new(900).unwrap();
        assert_eq!(min.as_secs(), 900);
        assert_eq!(min.as_minutes(), 15);

        // One hour
        let one_hour = SessionDuration::new(3600).unwrap();
        assert_eq!(one_hour.as_secs(), 3600);
        assert_eq!(one_hour.as_hours(), 1);

        // Maximum
        let max = SessionDuration::new(43200).unwrap();
        assert_eq!(max.as_secs(), 43200);
        assert_eq!(max.as_hours(), 12);
    }

    #[test]
    fn test_below_minimum() {
        let result = SessionDuration::new(899);
        assert!(matches!(
            result,
            Err(SessionDurationError::BelowMinimum(899))
        ));
    }

    #[test]
    fn test_exceeds_maximum() {
        let result = SessionDuration::new(43201);
        assert!(matches!(
            result,
            Err(SessionDurationError::ExceedsMaximum(43201))
        ));
    }

    #[test]
    fn test_from_minutes() {
        let duration = SessionDuration::from_minutes(30).unwrap();
        assert_eq!(duration.as_secs(), 1800);
    }

    #[test]
    fn test_from_hours() {
        let duration = SessionDuration::from_hours(2).unwrap();
        assert_eq!(duration.as_secs(), 7200);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", SessionDuration::ONE_HOUR), "1h");
        assert_eq!(format!("{}", SessionDuration::THIRTY_MINUTES), "30m");
        assert_eq!(format!("{}", SessionDuration::new(5400).unwrap()), "1h 30m");
    }

    #[test]
    fn test_constants() {
        assert_eq!(SessionDuration::MIN.as_secs(), 900);
        assert_eq!(SessionDuration::ONE_HOUR.as_secs(), 3600);
        assert_eq!(SessionDuration::MAX.as_secs(), 43200);
    }

    #[test]
    fn test_conversions() {
        let duration = SessionDuration::ONE_HOUR;

        let std_duration: Duration = duration.into();
        assert_eq!(std_duration.as_secs(), 3600);

        let secs: u64 = duration.into();
        assert_eq!(secs, 3600);

        let i32_val: i32 = duration.into();
        assert_eq!(i32_val, 3600);
    }

    #[test]
    fn test_try_from() {
        let duration: SessionDuration = 3600_u64.try_into().unwrap();
        assert_eq!(duration.as_secs(), 3600);

        let result: Result<SessionDuration, _> = 100_u64.try_into();
        assert!(result.is_err());
    }
}
