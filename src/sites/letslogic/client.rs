//! Fetching Let's Logic pages.
//!
//! Two shapes only: one level, and one collection — which is paginated, and
//! whose every card already carries the board (docs/api/letslogic.md §5), so a
//! collection costs one request per page rather than one per level.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use indicatif::ProgressBar;

use super::page::{self, Card};
use crate::http::Fetcher;

const BASE: &str = "https://www.letslogic.com";

/// Levels asked for per page. The site's own listing offers 10, 20, 50 and 100;
/// asking for more than its interface allows would be reaching past what the
/// site offers, so 100 is the ceiling used here.
const ITEMS: usize = 100;

pub struct Client {
    base: String,
    http: Fetcher,
}

impl Client {
    pub fn new(delay: Duration, timeout: Duration, stop: Arc<AtomicBool>) -> Self {
        Self {
            base: BASE.to_owned(),
            http: Fetcher::new(delay, timeout, stop),
        }
    }

    /// A client aimed at a local mock server: the rate-limit delay stays small
    /// but real, retries happen instantly.
    #[cfg(test)]
    pub(crate) fn test_client(base: &str, stop: Arc<AtomicBool>) -> Self {
        Self {
            base: base.to_owned(),
            http: Fetcher::with(
                Duration::from_millis(30),
                Duration::from_secs(10),
                Duration::ZERO,
                stop,
            ),
        }
    }

    /// The card for one level, or `None` when the page holds no such level. A
    /// missing level redirects to the home page rather than answering 404, so
    /// the absence of its plate is the signal.
    pub fn level(&self, id: &str) -> anyhow::Result<Option<Card>> {
        let url = format!("{}/level/{id}", self.base);
        let html = self.http.get_text(&url)?;
        Ok(page::cards(&html)
            .into_iter()
            .find(|card| card.id.to_string() == id))
    }

    /// Every card of one collection, page by page, in the site's order.
    pub fn collection(&self, id: &str, progress: &ProgressBar) -> anyhow::Result<Vec<Card>> {
        let mut cards: Vec<Card> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut page = 1;
        loop {
            let url = format!("{}/collection/{id}?page={page}&items={ITEMS}", self.base);
            let html = self.http.get_text(&url)?;
            // The site may clamp `items`; only cards not seen yet count, so a
            // repeated page ends the walk instead of looping forever.
            let fresh: Vec<Card> = page::cards(&html)
                .into_iter()
                .filter(|card| seen.insert(card.id))
                .collect();
            let added = fresh.len();
            cards.extend(fresh);
            progress.set_message(format!(
                "listing collection {id}: page {page}, {} levels",
                cards.len()
            ));
            if added < ITEMS {
                return Ok(cards);
            }
            page += 1;
        }
    }
}
