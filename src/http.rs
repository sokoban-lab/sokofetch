//! One polite GET at a time, shared by every site.
//!
//! Sites differ in what they answer — JSON for Sokoban Online, server-rendered
//! HTML for Let's Logic — but not in how they should be asked: one request
//! after another, a pause between them, a bounded number of retries, and a
//! Ctrl+C flag that cuts the waiting short. That is all this module knows; it
//! never parses a body.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use ureq::Agent;

const USER_AGENT: &str = "Mozilla/5.0 (personal level archiver; one-off fetch)";
const MAX_ATTEMPTS: u32 = 4;

/// The site answered, and said there is no such page.
#[derive(Debug)]
pub struct NotFound(pub String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not found: {}", self.0)
    }
}

impl std::error::Error for NotFound {}

/// A status code that is not retried: 4xx, or 5xx on the last attempt.
#[derive(Debug)]
pub struct Status {
    pub code: u16,
    pub url: String,
    pub excerpt: String,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} for {}: {}", self.code, self.url, self.excerpt)
    }
}

impl std::error::Error for Status {}

/// A client that spaces its requests out and gives up after a few attempts.
pub struct Fetcher {
    agent: Agent,
    delay: Duration,
    retry_backoff: Duration,
    stop: Arc<AtomicBool>,
}

impl Fetcher {
    pub fn new(delay: Duration, timeout: Duration, stop: Arc<AtomicBool>) -> Self {
        Self::with(delay, timeout, Duration::from_secs(2), stop)
    }

    /// The same client with the retry backoff spelled out: tests collapse it to
    /// zero so a retry does not spend real time.
    pub fn with(
        delay: Duration,
        timeout: Duration,
        retry_backoff: Duration,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            agent: Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_global(Some(timeout))
                .user_agent(USER_AGENT)
                // 4xx/5xx must come back as Ok so the Retry-After header and
                // the error body remain accessible.
                .http_status_as_error(false)
                .build()
                .new_agent(),
            delay,
            retry_backoff,
            stop,
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleep in short slices so Ctrl+C takes effect promptly.
    fn sleep(&self, duration: Duration) -> anyhow::Result<()> {
        let mut left = duration;
        while left > Duration::ZERO {
            if self.stopped() {
                bail!("interrupted by user");
            }
            let step = left.min(Duration::from_millis(100));
            std::thread::sleep(step);
            left -= step;
        }
        Ok(())
    }

    /// GET one absolute URL with retries, returning the raw body.
    pub fn get_text(&self, url: &str) -> anyhow::Result<String> {
        for attempt in 1..=MAX_ATTEMPTS {
            if self.stopped() {
                bail!("interrupted by user");
            }
            match self.agent.get(url).call() {
                Ok(response) => {
                    // http_status_as_error is disabled: 4xx/5xx arrive as Ok.
                    let code = response.status().as_u16();
                    if code == 429 {
                        let wait = response
                            .headers()
                            .get("Retry-After")
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.trim().parse::<u64>().ok())
                            .unwrap_or(30);
                        eprintln!("  rate limited, waiting {wait}s ...");
                        self.sleep(Duration::from_secs(wait))?;
                        continue;
                    }
                    if code >= 500 && attempt < MAX_ATTEMPTS {
                        self.sleep(self.retry_backoff * attempt)?;
                        continue;
                    }
                    if code == 404 {
                        return Err(NotFound(url.to_owned()).into());
                    }
                    if !(200..300).contains(&code) {
                        return Err(Status {
                            code,
                            url: url.to_owned(),
                            excerpt: excerpt(response.into_body()),
                        }
                        .into());
                    }
                    let body = response
                        .into_body()
                        .read_to_string()
                        .with_context(|| format!("reading response of {url}"))?;
                    self.sleep(self.delay)?;
                    return Ok(body);
                }
                Err(err) => {
                    if attempt < MAX_ATTEMPTS {
                        eprintln!("  request failed ({err}), retrying ...");
                        self.sleep(self.retry_backoff * attempt)?;
                        continue;
                    }
                    return Err(err).with_context(|| format!("requesting {url}"));
                }
            }
        }
        bail!("giving up on {url} after {MAX_ATTEMPTS} attempts")
    }
}

/// Pull a human-readable message out of an error body: the API envelope's
/// message when there is one, otherwise the body's first line. Either way one
/// line, at most 200 characters, so a whole HTML error page cannot flood the
/// terminal.
fn excerpt(mut body: ureq::Body) -> String {
    let text = body.read_to_string().unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .pointer("/data/error/message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let joined = message.unwrap_or(text);
    let line = joined.lines().next().unwrap_or_default();
    if line.chars().count() <= 200 {
        line.to_owned()
    } else {
        line.chars().take(200).collect()
    }
}
