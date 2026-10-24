//! The error returned by the `text` and `json` request methods.

use std::fmt;

/// An error from [`Fetch::text`](super::Fetch::text), [`Fetch::json`](super::Fetch::json) and their `_with_options`
/// and `_response` forms.
///
/// Non-exhaustive, so a later kind of failure can be added without breaking a `match` on this one.
#[derive(Debug)]
#[non_exhaustive]
pub enum FetchError {
    /// The client could not be built, the request failed, the response carried a non-success status, or the body
    /// could not be read or deserialized.
    Http(reqwest::Error),
    /// The response body was larger than the limit set with [`Fetch::max_body_bytes`](super::Fetch::max_body_bytes)
    /// or [`RequestOptions::max_body_bytes`](super::RequestOptions::max_body_bytes).
    BodyTooLarge {
        /// The limit the body exceeded, in bytes.
        limit: u64,
    },
}

impl FetchError {
    /// The HTTP status of a response that was rejected for its status, and `None` for every other failure.
    ///
    /// The same as [`reqwest::Error::status`], kept here so the most common question about a failed request does not
    /// need a `match`.
    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            FetchError::Http(err) => err.status(),
            FetchError::BodyTooLarge { .. } => None,
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Http(err) => write!(f, "request failed: {err}"),
            FetchError::BodyTooLarge { limit } => write!(f, "response body is larger than the {limit}-byte limit"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Http(err) => Some(err),
            FetchError::BodyTooLarge { .. } => None,
        }
    }
}

impl From<reqwest::Error> for FetchError {
    fn from(err: reqwest::Error) -> Self {
        FetchError::Http(err)
    }
}
