use std::time::{Duration, SystemTime};

use reqwest::header::HeaderMap;

/// Reads `retry-after-ms` (milliseconds) or `Retry-After` (seconds or an HTTP date).
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    parse(
        header("retry-after-ms"),
        header("retry-after"),
        SystemTime::now(),
    )
}

fn parse(
    retry_after_ms: Option<&str>,
    retry_after: Option<&str>,
    now: SystemTime,
) -> Option<Duration> {
    let non_negative_secs = |secs: f64| {
        (secs.is_finite() && secs >= 0.0)
            .then(|| Duration::try_from_secs_f64(secs).ok())
            .flatten()
    };

    if let Some(delay) = retry_after_ms
        .and_then(|value| value.trim().parse::<f64>().ok())
        .and_then(|millis| non_negative_secs(millis / 1000.0))
    {
        return Some(delay);
    }

    let value = retry_after?.trim();
    if let Ok(secs) = value.parse::<f64>() {
        return non_negative_secs(secs);
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(date.duration_since(now).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_retry_after_ms_over_retry_after() {
        let now = SystemTime::now();

        assert_eq!(
            parse(Some("250"), Some("9"), now),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            parse(None, Some("1.5"), now),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn parses_http_date_relative_to_now() {
        let now = httpdate::parse_http_date("Wed, 16 Sep 2026 20:00:00 GMT").unwrap();

        assert_eq!(
            parse(None, Some("Wed, 16 Sep 2026 20:00:30 GMT"), now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse(None, Some("Wed, 16 Sep 2026 19:59:00 GMT"), now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn ignores_malformed_or_negative_values() {
        let now = SystemTime::now();

        assert_eq!(parse(None, Some("soon"), now), None);
        assert_eq!(parse(None, Some("-3"), now), None);
        assert_eq!(parse(Some("NaN"), None, now), None);
        assert_eq!(
            parse(Some("garbage"), Some("2"), now),
            Some(Duration::from_secs(2))
        );
    }
}
