use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

const DEFAULT_INITIAL_DELAY: Duration = Duration::from_secs(2);
const DEFAULT_BACKOFF_FACTOR: u32 = 2;
const DEFAULT_MAX_DELAY_WITHOUT_HEADERS: Duration = Duration::from_secs(30);
const DEFAULT_MAX_DELAY: Duration = Duration::from_millis(2_147_483_647);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    pub initial_delay: Duration,
    pub backoff_factor: u32,
    pub max_delay_without_headers: Duration,
    pub max_delay: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryDecision {
    pub attempt: u32,
    pub delay: Duration,
    pub message: String,
    pub next_retry_at: SystemTime,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay: DEFAULT_INITIAL_DELAY,
            backoff_factor: DEFAULT_BACKOFF_FACTOR,
            max_delay_without_headers: DEFAULT_MAX_DELAY_WITHOUT_HEADERS,
            max_delay: DEFAULT_MAX_DELAY,
        }
    }
}

impl RetryPolicy {
    #[must_use]
    pub fn delay_for_attempt(
        &self,
        attempt: u32,
        headers: Option<&BTreeMap<String, String>>,
    ) -> Duration {
        if let Some(headers) = headers {
            if let Some(delay) = retry_after_delay(headers) {
                return delay.min(self.max_delay);
            }
            return exponential_delay(self.initial_delay, self.backoff_factor, attempt)
                .min(self.max_delay);
        }

        exponential_delay(self.initial_delay, self.backoff_factor, attempt)
            .min(self.max_delay_without_headers)
            .min(self.max_delay)
    }

    #[must_use]
    pub fn decision(
        &self,
        attempt: u32,
        message: impl Into<String>,
        headers: Option<&BTreeMap<String, String>>,
        now: SystemTime,
    ) -> RetryDecision {
        let delay = self.delay_for_attempt(attempt, headers);
        RetryDecision {
            attempt,
            delay,
            message: message.into(),
            next_retry_at: now + delay,
        }
    }
}

#[must_use]
pub fn is_retryable_error(status_code: Option<u16>, message: &str) -> bool {
    if status_code
        .is_some_and(|status| status >= 500 || status == 408 || status == 409 || status == 429)
    {
        return true;
    }

    let lower = message.to_ascii_lowercase();
    lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("overloaded")
        || lower.contains("temporarily unavailable")
        || lower.contains("timeout")
}

fn retry_after_delay(headers: &BTreeMap<String, String>) -> Option<Duration> {
    let retry_after_ms = header_value(headers, "retry-after-ms");
    if let Some(ms) = retry_after_ms.and_then(|value| value.parse::<u64>().ok()) {
        return Some(Duration::from_millis(ms));
    }

    let retry_after = header_value(headers, "retry-after");
    retry_after
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn exponential_delay(initial: Duration, factor: u32, attempt: u32) -> Duration {
    let multiplier = factor.saturating_pow(attempt.saturating_sub(1));
    initial.saturating_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::{is_retryable_error, RetryPolicy};
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn delay_uses_retry_after_headers_first() {
        let mut headers = BTreeMap::new();
        headers.insert("retry-after-ms".to_string(), "750".to_string());

        assert_eq!(
            RetryPolicy::default().delay_for_attempt(3, Some(&headers)),
            Duration::from_millis(750)
        );
    }

    #[test]
    fn delay_uses_capped_exponential_backoff() {
        let policy = RetryPolicy::default();

        assert_eq!(policy.delay_for_attempt(1, None), Duration::from_secs(2));
        assert_eq!(policy.delay_for_attempt(2, None), Duration::from_secs(4));
        assert_eq!(policy.delay_for_attempt(9, None), Duration::from_secs(30));
    }

    #[test]
    fn detects_retryable_status_and_messages() {
        assert!(is_retryable_error(Some(500), "server error"));
        assert!(is_retryable_error(Some(429), "nope"));
        assert!(is_retryable_error(None, "provider is overloaded"));
        assert!(!is_retryable_error(Some(400), "bad request"));
    }
}
