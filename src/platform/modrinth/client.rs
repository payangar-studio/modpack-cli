//! HTTP client of the Modrinth platform: a generic User-Agent, and waiting
//! then retrying when Modrinth answers that the rate limit is reached.

use std::io::{self, Read};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha512};
use ureq::http::{Response, StatusCode};
use ureq::{Agent, Body};

use crate::error::{Context, Error, Result};

/// Identifies the tool, never the person running it.
pub const USER_AGENT: &str = concat!("payangar-studio/modpack-cli/", env!("CARGO_PKG_VERSION"));

/// Retries after a `429 Too Many Requests` before giving up.
const MAX_RETRIES: u32 = 3;
/// Longest wait for the rate limit window to reset, whatever the headers say.
const MAX_WAIT: Duration = Duration::from_secs(60);
const HINT: &str = "retry later; if it persists, check https://status.modrinth.com";

#[derive(Debug, Clone)]
pub struct Client {
    agent: Agent,
    /// Base URL of the API, without trailing slash.
    api: String,
}

impl Client {
    pub fn new(api: &str) -> Self {
        let agent = Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self {
            agent,
            api: api.trim_end_matches('/').to_owned(),
        }
    }

    /// `GET <api><path>` with query parameters; `None` on 404.
    pub fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Option<T>> {
        let url = format!("{}{path}", self.api);
        let response = self.send(&format!("GET {path}"), || {
            self.agent
                .get(&url)
                .query_pairs(query.iter().copied())
                .call()
        })?;
        read_json(response, path)
    }

    /// `POST <api><path>` with a JSON body; `None` on 404.
    pub fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<Option<T>> {
        let url = format!("{}{path}", self.api);
        let response = self.send(&format!("POST {path}"), || {
            self.agent.post(&url).send_json(body)
        })?;
        read_json(response, path)
    }

    /// Streams the file at `url`; reading it fails at the end if its sha512
    /// is not `sha512`.
    pub fn download(&self, url: &str, sha512: &str) -> Result<Box<dyn Read + Send>> {
        let response = self.send(&format!("downloading {url}"), || self.agent.get(url).call())?;
        if !response.status().is_success() {
            return Err(Error::internal(
                format!(
                    "Modrinth answered {} to the download of {url}",
                    response.status()
                ),
                HINT,
            ));
        }
        Ok(Box::new(Sha512Check {
            inner: response.into_body().into_reader(),
            hasher: Sha512::new(),
            expected: sha512.to_ascii_lowercase(),
            checked: false,
        }))
    }

    /// Sends the request built by `call`, again after a wait while Modrinth
    /// answers 429.
    fn send(
        &self,
        what: &str,
        call: impl Fn() -> Result<Response<Body>, ureq::Error>,
    ) -> Result<Response<Body>> {
        let mut attempt = 0;
        loop {
            let response = call().context(format!("calling Modrinth ({what})"))?;
            if response.status() != StatusCode::TOO_MANY_REQUESTS {
                return Ok(response);
            }
            if attempt == MAX_RETRIES {
                return Err(Error::internal(
                    format!(
                        "Modrinth rate limit still reached after {MAX_RETRIES} retries ({what})"
                    ),
                    "wait a minute, then run the command again",
                ));
            }
            attempt += 1;
            let wait = rate_limit_wait(&response);
            tracing::warn!(
                "Modrinth rate limit reached, retrying in {}s ({what})",
                wait.as_secs()
            );
            thread::sleep(wait);
        }
    }
}

fn read_json<T: DeserializeOwned>(mut response: Response<Body>, path: &str) -> Result<Option<T>> {
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(Error::internal(
            format!("Modrinth answered {status} to {path}"),
            HINT,
        ));
    }
    let value = response
        .body_mut()
        .read_json()
        .context(format!("reading the answer of Modrinth to {path}"))?;
    Ok(Some(value))
}

/// How long to wait before retrying: `X-Ratelimit-Reset` (seconds until the
/// window resets), one second when missing, never more than [`MAX_WAIT`].
fn rate_limit_wait(response: &Response<Body>) -> Duration {
    response
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(Duration::from_secs(1), Duration::from_secs)
        .min(MAX_WAIT)
}

/// A reader that hashes what goes through it and fails at the end of the
/// stream when the sha512 differs from the expected one.
struct Sha512Check<R> {
    inner: R,
    hasher: Sha512,
    expected: String,
    checked: bool,
}

impl<R: Read> Read for Sha512Check<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        } else if !buf.is_empty() && !self.checked {
            self.checked = true;
            let actual: String = self
                .hasher
                .finalize_reset()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if actual != self.expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("sha512 mismatch: expected {}, got {actual}", self.expected),
                ));
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sha512_hex(bytes: &[u8]) -> String {
        Sha512::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn check(bytes: &'static [u8], expected: String) -> Sha512Check<Cursor<&'static [u8]>> {
        Sha512Check {
            inner: Cursor::new(bytes),
            hasher: Sha512::new(),
            expected,
            checked: false,
        }
    }

    #[test]
    fn user_agent_names_the_tool_only() {
        assert!(USER_AGENT.starts_with("payangar-studio/modpack-cli/"));
        assert!(USER_AGENT.ends_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn matching_stream_reads_through() {
        let mut out = Vec::new();
        check(b"jar", sha512_hex(b"jar"))
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"jar");
    }

    #[test]
    fn mismatching_stream_fails_at_the_end() {
        let mut out = Vec::new();
        let err = check(b"jar", sha512_hex(b"other"))
            .read_to_end(&mut out)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("sha512 mismatch"));
    }

    #[test]
    fn wait_follows_the_reset_header_within_bounds() {
        let with = |reset: Option<&str>| {
            let mut builder = Response::builder().status(429);
            if let Some(reset) = reset {
                builder = builder.header("X-Ratelimit-Reset", reset);
            }
            rate_limit_wait(&builder.body(Body::builder().data("")).unwrap())
        };
        assert_eq!(with(Some("7")), Duration::from_secs(7));
        assert_eq!(with(None), Duration::from_secs(1));
        assert_eq!(with(Some("junk")), Duration::from_secs(1));
        assert_eq!(with(Some("3600")), MAX_WAIT);
    }
}
