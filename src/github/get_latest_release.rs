use crate::fetch::Fetch;

use super::{API_BASE_URL, GithubError, Release};

/// Fetches the latest published release of the GitHub repository `owner/repo`.
///
/// "Latest" is GitHub's definition: the most recent release that is neither a draft nor a pre-release. The request
/// is unauthenticated, so it counts against GitHub's limit of 60 requests per hour per IP address.
///
/// # Errors
///
/// Returns [`GithubError::Request`] if the request fails, including when the repository does not exist or has no
/// releases (`404`) and when the rate limit is exhausted (`403`).
pub async fn get_latest_release(owner: &str, repo: &str) -> Result<Release, GithubError> {
    get_latest_release_from(API_BASE_URL, owner, repo).await
}

/// [`get_latest_release`] against an arbitrary API root, so tests can point it at a local server.
pub(super) async fn get_latest_release_from(base_url: &str, owner: &str, repo: &str) -> Result<Release, GithubError> {
    // A client per call rather than one shared `static`: an async client's pooled connections belong to the runtime
    // that opened them, and a caller that builds a fresh runtime per call would find the pool full of dead ones.
    // GitHub rejects requests without a `User-Agent`, and `reqwest` sends none by default.
    let fetch = Fetch::new()
        .header("User-Agent", concat!("rust-sak/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    Ok(fetch.json(release_url(base_url, owner, repo)).await?)
}

/// Builds the endpoint URL with `owner` and `repo` as single path segments.
///
/// Pasted into a format string, a `/`, `?` or `#` inside either name would point the request at a different API
/// endpoint. Added as segments they are percent-encoded instead, and a bare `.` or `..` is dropped rather than
/// climbing. A root that does not parse is passed through as text, for the request to report.
fn release_url(base_url: &str, owner: &str, repo: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(base_url) else {
        return format!("{base_url}/repos/{owner}/{repo}/releases/latest");
    };

    if let Ok(mut segments) = url.path_segments_mut() {
        segments
            .pop_if_empty()
            .extend(["repos", owner, repo, "releases", "latest"]);
    }

    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_reach_another_endpoint() {
        assert_eq!(
            release_url("https://api.github.com", "vegidio", "rust-sak"),
            "https://api.github.com/repos/vegidio/rust-sak/releases/latest"
        );
        assert_eq!(
            release_url("https://api.github.com", "x/../../users", "r?q=1"),
            "https://api.github.com/repos/x%2F..%2F..%2Fusers/r%3Fq=1/releases/latest"
        );
    }
}
