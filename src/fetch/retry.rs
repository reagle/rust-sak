//! Retry helper with Fibonacci backoff, shared by the `fetch` request methods.

use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;

/// Maximum delay (seconds) between retry attempts. The Fibonacci backoff grows without bound, so without a ceiling a
/// large retry count would stall for minutes, then hours. Capping keeps a high retry count a bounded retry budget
/// rather than an effectively-infinite hang.
///
/// This cap is also what keeps [`fibonacci_delay`]'s closed form correct: Binet's formula is only exact through
/// `F(78)` before floating-point drift, but every Fibonacci value below this cap is `F(2)..=F(10)` (1..=55), well
/// inside the exact range — anything larger is clamped here regardless. Do **not** raise this anywhere near the
/// float-exact range (~9e15) or `fibonacci_delay` would start returning imprecise values.
const MAX_BACKOFF_SECS: u64 = 60;

/// The wait a server asked for in its last answer, handed from an attempt to the retry loop around it.
///
/// The request methods report a `reqwest::Error` (or a type wrapping one), which keeps the status of a rejected
/// response but drops its headers — so `Retry-After` has to be read by the attempt itself, while it still holds the
/// response, and left here for the loop to pick up. A `Mutex` rather than a `Cell` only because the attempts run
/// inside futures that must be `Send`; it is never contended.
#[derive(Debug, Default)]
pub(crate) struct RetryAfter(Mutex<Option<Duration>>);

impl RetryAfter {
    /// Records what `response` asks for — or that it asks for nothing, so a hint from an earlier attempt never
    /// outlives the answer it came with.
    pub(crate) fn record(&self, response: &reqwest::Response) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = requested_wait(response);
    }

    /// Takes the recorded wait, leaving nothing behind.
    fn take(&self) -> Option<Duration> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The wait a `429 Too Many Requests` or `503 Service Unavailable` asks for in its `Retry-After` header.
///
/// RFC 9110 §10.2.3 allows two forms: a number of seconds, or an HTTP date to wait until. A date already past means
/// "now". Any other status, or a header in neither form, asks for nothing, and the Fibonacci schedule applies.
fn requested_wait(response: &reqwest::Response) -> Option<Duration> {
    if !matches!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        return None;
    }

    parse_retry_after(response.headers().get(RETRY_AFTER)?.to_str().ok()?, SystemTime::now())
}

/// Parses a `Retry-After` value against `now`. Split from [`requested_wait`] so the date form can be tested against a
/// fixed clock.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let until = httpdate::parse_http_date(value).ok()?;
    Some(until.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Runs `operation`, retrying up to `retries` additional times on an `Err` that `should_retry` accepts.
///
/// Between attempts, it sleeps for a Fibonacci-growing number of seconds: 1s before the first retry, then 2s, 3s, 5s,
/// 8s, … — unless the failed attempt left a server-requested wait in `retry_after`, which is used instead. Either is
/// capped at [`MAX_BACKOFF_SECS`]: a server asking for an hour gets a retry after a minute, which it may well refuse
/// again, rather than a caller blocked for the hour. On success returns immediately; on an error `should_retry`
/// rejects, or once retries are exhausted, returns that error.
pub(crate) async fn with_fibonacci_backoff<F, Fut, T, E>(
    retries: u32,
    should_retry: impl Fn(&E) -> bool,
    retry_after: &RetryAfter,
    mut operation: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt: u32 = 0;

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt >= retries || !should_retry(&err) {
                    return Err(err);
                }
                let delay = retry_after
                    .take()
                    .unwrap_or_else(|| Duration::from_secs(fibonacci_delay(attempt + 1)))
                    .min(Duration::from_secs(MAX_BACKOFF_SECS));
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// Reports whether a request failure might go away if the same request were sent again.
///
/// A status says so directly: a server error, `408 Request Timeout` and `429 Too Many Requests` are worth another
/// attempt, every other `4xx` is the request itself being wrong and will be wrong again. A body that arrived whole but
/// does not parse as the JSON asked for is the same bytes on every attempt, so it is final too — `reqwest` reports it
/// as a decode error, like a body cut off mid-read, so the two are told apart by what caused it. A request that could
/// not even be built, or a redirect loop, is final. Everything else — connecting, timing out, a stream that broke
/// partway — is the network, and is retried.
pub(crate) fn is_transient(err: &reqwest::Error) -> bool {
    if let Some(status) = err.status() {
        return status.is_server_error()
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
    }

    if err.is_decode() && std::error::Error::source(err).is_some_and(|source| source.is::<serde_json::Error>()) {
        return false;
    }

    !(err.is_builder() || err.is_redirect())
}

/// Delay (in seconds) before the `n`-th retry (1-based): 1, 2, 3, 5, 8, 13, … — the standard Fibonacci `F(n + 1)`.
///
/// Computed in closed form via Binet's formula rather than iteratively. The `f64` math is exact only through `F(78)`,
/// but callers clamp the result to [`MAX_BACKOFF_SECS`], far below where drift begins, so every delay that actually
/// matters is computed exactly (and an overflow to `f64::INFINITY` for huge `n` saturates through `as u64`, then
/// clamps). See [`MAX_BACKOFF_SECS`] for the invariant this relies on.
fn fibonacci_delay(n: u32) -> u64 {
    let sqrt5 = 5f64.sqrt();
    let phi = (1.0 + sqrt5) / 2.0;
    // Use an `f64` exponent (not `powi`'s `i32`): a large `n` would otherwise wrap when cast to `i32`. Here a large
    // `n` just sends `powf` to `f64::INFINITY`, which saturates through `as u64` and is clamped by the caller.
    (phi.powf(n as f64 + 1.0) / sqrt5).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn fibonacci_delay_follows_sequence() {
        let expected = [1u64, 2, 3, 5, 8, 13, 21];
        for (i, &want) in expected.iter().enumerate() {
            assert_eq!(fibonacci_delay(i as u32 + 1), want);
        }
    }

    #[test]
    fn fibonacci_delay_is_capped_for_large_n() {
        // Past the float-exact range the closed form may drift, but every such value is astronomically larger than the
        // cap, so the clamped delay is always `MAX_BACKOFF_SECS`. `u32::MAX` also exercises the overflow-to-infinity
        // saturating path.
        for &n in &[100u32, 1_000, u32::MAX] {
            assert_eq!(fibonacci_delay(n).min(MAX_BACKOFF_SECS), MAX_BACKOFF_SECS);
        }
    }

    #[tokio::test]
    async fn returns_immediately_on_first_success() {
        let calls = Cell::new(0);
        let result: Result<u32, ()> = with_fibonacci_backoff(
            3,
            |_| true,
            &RetryAfter::default(),
            || {
                calls.set(calls.get() + 1);
                async { Ok(42) }
            },
        )
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_until_success() {
        let calls = Cell::new(0);
        let result: Result<u32, &str> = with_fibonacci_backoff(
            5,
            |_| true,
            &RetryAfter::default(),
            || {
                calls.set(calls.get() + 1);
                async { if calls.get() < 3 { Err("boom") } else { Ok(7) } }
            },
        )
        .await;

        assert_eq!(result, Ok(7));
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn no_retries_returns_first_error() {
        let calls = Cell::new(0);
        let result: Result<u32, &str> = with_fibonacci_backoff(
            0,
            |_| true,
            &RetryAfter::default(),
            || {
                calls.set(calls.get() + 1);
                async { Err("boom") }
            },
        )
        .await;

        assert_eq!(result, Err("boom"));
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn an_error_the_predicate_rejects_is_not_retried() {
        let calls = Cell::new(0);
        let result: Result<u32, &str> = with_fibonacci_backoff(
            5,
            |err| *err != "final",
            &RetryAfter::default(),
            || {
                calls.set(calls.get() + 1);
                async { Err("final") }
            },
        )
        .await;

        assert_eq!(result, Err("final"));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retry_after_reads_seconds_and_dates() {
        let now = httpdate::parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").unwrap();

        assert_eq!(parse_retry_after("120", now), Some(Duration::from_secs(120)));
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:50:07 GMT", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:00:00 GMT", now),
            Some(Duration::ZERO),
            "a date already past means now"
        );
        assert_eq!(parse_retry_after("soon", now), None);
    }
}
