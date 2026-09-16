use std::ops::RangeInclusive;
use std::time::Duration;

use crate::Error;

const DEFAULT_RETRY_STATUSES: [RangeInclusive<u16>; 3] = [408..=408, 429..=429, 500..=599];

/// When and how often a failed call is retried.
///
/// The defaults follow the official TypeSafe SDKs: two retries after the first attempt;
/// exponential backoff from 0.5 s to at most 5 s with up to 25% jitter; retries on 408,
/// 429 and 5xx (including 529), on connection errors and on timeouts; honors
/// `retry-after-ms` and `Retry-After` up to 60 s; and a 30 s deadline for the whole call.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RetryPolicy {
    /// Retries after the first attempt; `0` disables retries.
    pub max_retries: u32,
    /// First backoff delay, doubled for each retry up to `backoff_max`. Zero disables backoff.
    pub backoff_initial: Duration,
    /// Upper bound for the backoff delay. Zero disables backoff.
    pub backoff_max: Duration,
    /// Fraction of each backoff delay randomly subtracted, from 0 to 1.
    pub backoff_jitter: f64,
    /// HTTP statuses that are retried.
    pub retry_statuses: Vec<RangeInclusive<u16>>,
    /// Wait as long as `retry-after-ms` or `Retry-After` asks, instead of the backoff.
    pub respect_retry_after: bool,
    /// Longest server-requested wait that is honored; longer requests use the backoff.
    pub max_retry_after: Duration,
    /// Retry when the server cannot be reached.
    pub retry_connection_errors: bool,
    /// Retry when an attempt times out.
    pub retry_timeouts: bool,
    /// Deadline for the whole call, including every attempt and delay. Attempts are cut
    /// short to fit, and no retry is scheduled whose delay would reach the deadline.
    pub total_timeout: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            backoff_jitter: 0.25,
            retry_statuses: DEFAULT_RETRY_STATUSES.to_vec(),
            respect_retry_after: true,
            max_retry_after: Duration::from_secs(60),
            retry_connection_errors: true,
            retry_timeouts: true,
            total_timeout: Some(Duration::from_secs(30)),
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries. The total deadline still applies.
    pub fn disabled() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// Sets how many retries follow the first attempt.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sets the first and the largest backoff delay.
    #[must_use]
    pub fn with_backoff(mut self, initial: Duration, max: Duration) -> Self {
        self.backoff_initial = initial;
        self.backoff_max = max;
        self
    }

    /// Sets the fraction of each backoff delay randomly subtracted, from 0 to 1.
    #[must_use]
    pub fn with_backoff_jitter(mut self, jitter: f64) -> Self {
        self.backoff_jitter = jitter;
        self
    }

    /// Sets the HTTP statuses that are retried.
    #[must_use]
    pub fn with_retry_statuses(
        mut self,
        statuses: impl IntoIterator<Item = RangeInclusive<u16>>,
    ) -> Self {
        self.retry_statuses = statuses.into_iter().collect();
        self
    }

    /// Sets whether `retry-after-ms` and `Retry-After` are honored, and the longest wait.
    #[must_use]
    pub fn with_retry_after(mut self, respect: bool, max: Duration) -> Self {
        self.respect_retry_after = respect;
        self.max_retry_after = max;
        self
    }

    /// Sets whether connection failures and timeouts are retried.
    #[must_use]
    pub fn with_transport_retries(mut self, connection_errors: bool, timeouts: bool) -> Self {
        self.retry_connection_errors = connection_errors;
        self.retry_timeouts = timeouts;
        self
    }

    /// Sets the deadline for the whole call; `None` removes it.
    #[must_use]
    pub fn with_total_timeout(mut self, total_timeout: Option<Duration>) -> Self {
        self.total_timeout = total_timeout;
        self
    }

    /// Whether this policy retries `error`, ignoring the retry count and deadline.
    pub fn should_retry(&self, error: &Error) -> bool {
        match error {
            Error::Api(api) => self
                .retry_statuses
                .iter()
                .any(|statuses| statuses.contains(&api.status)),
            Error::Timeout(_) => self.retry_timeouts,
            Error::Connection(_) => self.retry_connection_errors,
            _ => false,
        }
    }
}

// Scheduling helpers for transports that retry; only the HTTP client uses them.
#[cfg_attr(not(feature = "http"), allow(dead_code))]
impl RetryPolicy {
    /// The delay before retry number `retries_done + 1`. `random_unit` is in `[0, 1)`.
    pub(crate) fn delay_before_retry(
        &self,
        retries_done: u32,
        retry_after: Option<Duration>,
        random_unit: f64,
    ) -> Duration {
        if let Some(delay) = retry_after {
            if self.respect_retry_after && delay <= self.max_retry_after {
                return delay;
            }
        }
        let jitter = self.backoff_jitter.clamp(0.0, 1.0) * random_unit.clamp(0.0, 1.0);
        self.backoff(retries_done).mul_f64(1.0 - jitter)
    }

    fn backoff(&self, retries_done: u32) -> Duration {
        if self.backoff_initial.is_zero() || self.backoff_max.is_zero() {
            return Duration::ZERO;
        }
        2_u32
            .checked_pow(retries_done)
            .and_then(|factor| self.backoff_initial.checked_mul(factor))
            .map_or(self.backoff_max, |delay| delay.min(self.backoff_max))
    }

    /// Time left before the deadline; `None` when there is no deadline.
    pub(crate) fn remaining(&self, elapsed: Duration) -> Option<Duration> {
        self.total_timeout
            .map(|budget| budget.saturating_sub(elapsed))
    }

    pub(crate) fn exceeds_budget(&self, elapsed: Duration, delay: Duration) -> bool {
        self.total_timeout
            .is_some_and(|budget| elapsed.saturating_add(delay) >= budget)
    }
}

pub(crate) fn retryable_by_default(error: &Error) -> bool {
    match error {
        Error::Api(api) => DEFAULT_RETRY_STATUSES
            .iter()
            .any(|statuses| statuses.contains(&api.status)),
        Error::Timeout(_) | Error::Connection(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApiError, TransportError};

    fn no_jitter() -> RetryPolicy {
        RetryPolicy {
            backoff_jitter: 0.0,
            ..RetryPolicy::default()
        }
    }

    fn api_error(status: u16) -> Error {
        ApiError::new(status, "failed").into()
    }

    #[test]
    fn doubles_backoff_until_capped() {
        let policy = no_jitter();
        let delays: Vec<_> = (0..5)
            .map(|retry| policy.delay_before_retry(retry, None, 0.0))
            .collect();

        assert_eq!(
            delays,
            [500, 1000, 2000, 4000, 5000].map(Duration::from_millis)
        );
        assert_eq!(
            policy.delay_before_retry(u32::MAX, None, 0.0),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn subtracts_at_most_jitter_fraction_of_the_backoff() {
        let policy = RetryPolicy::default();

        assert_eq!(
            policy.delay_before_retry(0, None, 0.0),
            Duration::from_millis(500)
        );
        assert_eq!(
            policy.delay_before_retry(0, None, 1.0),
            Duration::from_millis(375)
        );
    }

    #[test]
    fn has_no_backoff_when_initial_or_max_is_zero() {
        let zero_initial = RetryPolicy {
            backoff_initial: Duration::ZERO,
            ..no_jitter()
        };
        let zero_max = RetryPolicy {
            backoff_max: Duration::ZERO,
            ..no_jitter()
        };

        assert_eq!(
            zero_initial.delay_before_retry(3, None, 0.0),
            Duration::ZERO
        );
        assert_eq!(zero_max.delay_before_retry(3, None, 0.0), Duration::ZERO);
    }

    #[test]
    fn waits_for_retry_after_only_when_respected_and_within_cap() {
        let asked = Some(Duration::from_secs(7));

        assert_eq!(
            no_jitter().delay_before_retry(0, asked, 0.0),
            Duration::from_secs(7)
        );
        let ignoring = RetryPolicy {
            respect_retry_after: false,
            ..no_jitter()
        };
        assert_eq!(
            ignoring.delay_before_retry(0, asked, 0.0),
            Duration::from_millis(500)
        );
        let too_long = Some(Duration::from_secs(3600));
        assert_eq!(
            no_jitter().delay_before_retry(0, too_long, 0.0),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn retries_documented_statuses_but_not_client_errors() {
        let policy = RetryPolicy::default();

        for status in [408, 429, 500, 503, 529] {
            assert!(
                policy.should_retry(&api_error(status)),
                "{status} should retry"
            );
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(
                !policy.should_retry(&api_error(status)),
                "{status} should not retry"
            );
        }
        assert!(policy.should_retry(&Error::Timeout(TransportError::new("slow"))));
        assert!(!policy.should_retry(&Error::Request(TransportError::new("bad header"))));
        assert!(!policy.should_retry(&Error::MissingApiKey));
    }

    #[test]
    fn stops_when_delay_would_reach_the_budget() {
        let policy = RetryPolicy {
            total_timeout: Some(Duration::from_secs(1)),
            ..no_jitter()
        };

        assert!(!policy.exceeds_budget(Duration::from_millis(400), Duration::from_millis(500)));
        assert!(policy.exceeds_budget(Duration::from_millis(500), Duration::from_millis(500)));
        assert_eq!(
            policy.remaining(Duration::from_millis(300)),
            Some(Duration::from_millis(700))
        );
        assert_eq!(
            policy.remaining(Duration::from_secs(5)),
            Some(Duration::ZERO)
        );

        let unlimited = RetryPolicy {
            total_timeout: None,
            ..no_jitter()
        };
        assert!(!unlimited.exceeds_budget(Duration::MAX, Duration::MAX));
        assert_eq!(unlimited.remaining(Duration::MAX), None);
    }

    #[test]
    fn prints_status_ranges_compactly() {
        assert!(format!("{:?}", RetryPolicy::default()).contains("500..=599"));
    }
}
