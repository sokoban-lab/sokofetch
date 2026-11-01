//! letslogic.com: server-rendered pages, no JSON read API.
//!
//! What the site stores and how its boards are encoded is in
//! `docs/api/letslogic.md`; this module maps it onto the [`Site`] abstraction.

mod board;
mod client;
mod page;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{bail, ensure};
use indicatif::ProgressBar;

use super::{
    Address, Collection, CollectionRef, Level, LevelRef, Outcome, Plan, Scope, Site, Skip,
    SkipKind, Target, Work, address,
};

/// Host the pages belong to.
const HOST: &str = "letslogic.com";

/// A site whose every page is HTML: one page per level, one per collection.
pub struct LetsLogic {
    client: client::Client,
}

impl LetsLogic {
    pub fn new(delay: Duration, timeout: Duration, stop: Arc<AtomicBool>) -> Self {
        Self {
            client: client::Client::new(delay, timeout, stop),
        }
    }

    /// A site aimed at a mock server, with the usual test pacing.
    #[cfg(test)]
    pub(crate) fn test_client(base: &str, stop: Arc<AtomicBool>) -> Self {
        Self {
            client: client::Client::test_client(base, stop),
        }
    }
}

impl Site for LetsLogic {
    fn name(&self) -> &'static str {
        "Let's Logic"
    }

    /// `level/<id>` or `collection/<id>` — or the page URL either appears in.
    /// A level's `/play` page is accepted too: it shows the same level, and it
    /// is the URL the address bar holds while playing.
    fn target(&self, input: &str) -> anyhow::Result<Option<Target>> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "empty collection path");
        let (path, owned) = match address(trimmed, HOST) {
            // A link to some other site: not ours to parse.
            Address::Foreign => return Ok(None),
            Address::Bare => (trimmed, false),
            Address::Owned(tail) => {
                // A link may carry a query or a fragment; neither names a page.
                let tail = tail.split(['?', '#']).next().unwrap_or_default();
                (tail.trim_end_matches('/'), true)
            }
        };
        let segments: Vec<&str> = path.split('/').collect();
        // A site claims what it can recognise and leaves the rest alone: a bare
        // path is only this site's when it has one of the two shapes the site
        // actually has, so `t1024/corner-set` or an author named `level` pass
        // through untouched. A page on this host, in contrast, was meant for
        // this site whatever it turns out to be, so a wrong shape there is an
        // error worth explaining.
        let parsed = match segments.as_slice() {
            ["level", id] | ["level", id, "play"] => Some(Target::Level(level_ref(id)?)),
            ["collection", id] => Some(Target::Collection(CollectionRef {
                path: format!("collection/{}", numeric(id, "collection")?),
                // A collection holds no sub-collections, so there is no
                // narrower scope to ask for.
                scope: Scope::All,
            })),
            _ => None,
        };
        match (parsed, owned) {
            (Some(target), _) => Ok(Some(target)),
            (None, true) => bail!(
                "unsupported page {trimmed:?}: expected https://www.{HOST}/level/<id> \
                 or https://www.{HOST}/collection/<id>"
            ),
            (None, false) => Ok(None),
        }
    }

    fn inspect(
        &self,
        collection: &CollectionRef,
        progress: &ProgressBar,
    ) -> anyhow::Result<Collection> {
        let (name, cards) = self.cards(collection, progress)?;
        Ok(Collection {
            name,
            page: collection_url(collection_id(&collection.path)),
            total: Some(cards.len() as u64),
            groups: Vec::new(),
            own: cards.iter().map(level_ref_of).collect(),
        })
    }

    fn plan(&self, target: &Target, progress: &ProgressBar) -> anyhow::Result<Plan> {
        match target {
            Target::Level(level) => self.plan_level(level),
            Target::Collection(collection) => self.plan_collection(collection, progress),
        }
    }

    fn fetch(&self, level: &LevelRef) -> anyhow::Result<Outcome> {
        Ok(outcome_of(&self.card(level)?))
    }

    fn id_from_url(&self, url: &str) -> Option<String> {
        let path = url.strip_prefix(&format!("https://www.{HOST}/level/"))?;
        let id = path.split(['/', '?', '#']).next()?;
        (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())).then(|| id.to_owned())
    }
}

impl LetsLogic {
    /// Fetch one level's card, naming its path when the site has no such level.
    fn card(&self, level: &LevelRef) -> anyhow::Result<page::Card> {
        self.client.level(&level.id)?.ok_or_else(|| {
            anyhow::anyhow!("level not found: {} ({})", level.path, level_url(&level.id))
        })
    }

    /// Every card of a collection, with the collection's name.
    fn cards(
        &self,
        collection: &CollectionRef,
        progress: &ProgressBar,
    ) -> anyhow::Result<(String, Vec<page::Card>)> {
        let id = collection_id(&collection.path);
        let cards = self.client.collection(id, progress)?;
        ensure!(
            !cards.is_empty(),
            "collection not found: {} ({})",
            collection.path,
            collection_url(id)
        );
        let name = cards
            .iter()
            .map(|card| card.collection_name.as_str())
            .find(|name| !name.is_empty())
            .unwrap_or(id)
            .to_owned();
        Ok((name, cards))
    }

    /// One level: the page carries the board, so the run costs a single request
    /// and there is nothing left to fetch afterwards.
    fn plan_level(&self, target: &LevelRef) -> anyhow::Result<Plan> {
        let card = self.card(target)?;
        let level = level_ref_of(&card);
        let collection = Collection {
            name: card.collection_name.clone(),
            page: collection_url(&collection_id_of(&card)),
            total: None,
            groups: Vec::new(),
            own: vec![level.clone()],
        };
        Ok(Plan {
            collection,
            work: vec![Work::Done(level, Box::new(outcome_of(&card)))],
        })
    }

    /// A collection: every card already holds its board, so the listing is the
    /// whole job — no per-level request is made at all.
    fn plan_collection(
        &self,
        target: &CollectionRef,
        progress: &ProgressBar,
    ) -> anyhow::Result<Plan> {
        let (name, cards) = self.cards(target, progress)?;
        let levels: Vec<LevelRef> = cards.iter().map(level_ref_of).collect();
        let work = cards
            .iter()
            .map(|card| Work::Done(level_ref_of(card), Box::new(outcome_of(card))))
            .collect();
        Ok(Plan {
            collection: Collection {
                name,
                page: collection_url(collection_id(&target.path)),
                total: Some(levels.len() as u64),
                groups: Vec::new(),
                own: levels,
            },
            work,
        })
    }
}

/// A level the user named, before its page has been read: the collection it
/// belongs to is only in the card, so it is left empty here.
fn level_ref(id: &str) -> anyhow::Result<LevelRef> {
    let id = numeric(id, "level")?;
    Ok(LevelRef::new(id, "", id).with_path(format!("level/{id}")))
}

/// A level as a card describes it: the id, its collection and its name.
fn level_ref_of(card: &page::Card) -> LevelRef {
    let id = card.id.to_string();
    LevelRef::new(id.clone(), collection_id_of(card), card.name.clone())
        .with_path(format!("level/{id}"))
}

/// The collection a card belongs to, as the canonical path the tool uses.
fn collection_id_of(card: &page::Card) -> String {
    card.collection_id
        .map(|id| id.to_string())
        .unwrap_or_default()
}

/// Convert one card into a board, or the reason there is none.
fn outcome_of(card: &page::Card) -> Outcome {
    let board = board::to_xsb(&card.map, card.width, card.height);
    let level = Level {
        id: card.id.to_string(),
        title: card.name.clone(),
        author: card.authors.clone(),
        collection_name: card.collection_name.clone(),
        url: level_url(&card.id.to_string()),
        // The site shows who published a level, not when.
        published: None,
        notes: notes(card, board.as_deref().ok()),
    };
    match board {
        Ok(board) => Outcome::Level(level, board),
        // Every tile the site has maps to XSB, so a rejection means the map
        // itself was malformed rather than a mechanism XSB cannot express.
        Err(reason) => Outcome::Skip(level, Skip::new(SkipKind::Invalid, reason)),
    }
}

/// The facts `info` prints after a level's label: the board's size and the
/// site's best known solution.
fn notes(card: &page::Card, board: Option<&str>) -> Vec<String> {
    let (width, height) = board.map_or((card.width, card.height), |board| {
        (
            board
                .lines()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0),
            board.lines().count(),
        )
    });
    let mut notes = vec![format!("{width}x{height}")];
    if let Some((moves, pushes)) = card.record {
        notes.push(format!("record {moves}/{pushes}"));
    }
    notes
}

/// Reject anything that is not a plain id before it reaches a URL.
fn numeric<'a>(id: &'a str, what: &str) -> anyhow::Result<&'a str> {
    ensure!(
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid {what} id {id:?} (expected digits)"
    );
    Ok(id)
}

/// The collection id out of the canonical path, `collection/632`.
fn collection_id(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn collection_url(id: &str) -> String {
    format!("https://www.{HOST}/collection/{id}")
}

fn level_url(id: &str) -> String {
    format!("https://www.{HOST}/level/{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Action, MockServer, Response, wire_page, wire_plate};
    use std::sync::atomic::AtomicBool;

    /// A board the site would accept: walls, one player, one box and one goal.
    const ROWS: [&str; 3] = ["111", "123", "114"];

    fn stop() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn site(input: &str) -> anyhow::Result<Option<Target>> {
        LetsLogic::new(Duration::ZERO, Duration::from_secs(5), stop()).target(input)
    }

    /// A mock server + a site aimed at it.
    fn mock(
        handler: impl Fn(usize, &str) -> Action + Send + Sync + 'static,
    ) -> (Arc<MockServer>, LetsLogic) {
        let server = MockServer::start_with("");
        server.set_handler(handler);
        let site = LetsLogic::test_client(&server.base, stop());
        (server, site)
    }

    fn page(plates: &[String]) -> Action {
        Action::Respond(Response::plain(200, wire_page(plates)))
    }

    #[test]
    fn levels_and_collections_are_addressed_by_path_or_by_url() {
        for input in [
            "level/89411",
            " level/89411 ",
            "https://www.letslogic.com/level/89411",
            "http://letslogic.com/level/89411",
            "https://www.letslogic.com/level/89411/",
            "https://www.letslogic.com/level/89411?ref=list#top",
            // the play page shows the same level, and is the URL the address
            // bar holds while playing
            "https://www.letslogic.com/level/89411/play",
        ] {
            let target = site(input)
                .expect("valid input")
                .expect("this site owns it");
            match target {
                Target::Level(level) => {
                    assert_eq!(level.id, "89411", "{input}");
                    // the canonical path, whatever the input spelled
                    assert_eq!(level.path, "level/89411", "{input}");
                }
                other => panic!("{input}: expected a level, got {other:?}"),
            }
        }

        for input in [
            "collection/632",
            "https://www.letslogic.com/collection/632",
            "https://www.letslogic.com/collection/632?page=3&items=10",
        ] {
            let target = site(input)
                .expect("valid input")
                .expect("this site owns it");
            match target {
                Target::Collection(collection) => {
                    assert_eq!(collection.path, "collection/632", "{input}");
                    assert_eq!(collection.scope, Scope::All);
                }
                other => panic!("{input}: expected a collection, got {other:?}"),
            }
        }
    }

    #[test]
    fn paths_this_site_does_not_recognise_are_left_to_the_next_one() {
        for input in [
            "t1024/corner-set",
            "level",
            "89411",
            "https://www.sokobanonline.com/play/community/t1024",
        ] {
            assert!(site(input).expect("no error").is_none(), "{input}");
        }
    }

    #[test]
    fn a_page_on_this_host_with_the_wrong_shape_is_an_error() {
        let err = site("https://www.letslogic.com/faq").unwrap_err();
        assert!(err.to_string().contains("expected"), "{err}");
        let err = site("level/abc").unwrap_err();
        assert!(err.to_string().contains("invalid level id"), "{err}");
        let err = site("collection/6x2").unwrap_err();
        assert!(err.to_string().contains("invalid collection id"), "{err}");
        assert!(site("  ").is_err());
    }

    #[test]
    fn inspect_reports_the_collection_and_its_levels() {
        let (_server, site) = mock(|_call, _target| {
            page(&[
                wire_plate(88461, "Level 1", 632, "Recreation", &ROWS),
                wire_plate(88462, "Level 2", 632, "Recreation", &ROWS),
            ])
        });
        let collection = site
            .inspect(
                &CollectionRef {
                    path: "collection/632".into(),
                    scope: Scope::All,
                },
                &ProgressBar::hidden(),
            )
            .unwrap();
        assert_eq!(collection.name, "Recreation");
        assert_eq!(collection.page, "https://www.letslogic.com/collection/632");
        assert_eq!(collection.total, Some(2));
        assert!(
            collection.groups.is_empty(),
            "a collection has no sub-groups"
        );
        let ids: Vec<&str> = collection
            .own
            .iter()
            .map(|level| level.id.as_str())
            .collect();
        assert_eq!(ids, ["88461", "88462"]);
        assert_eq!(collection.own[0].label, "Level 1");
    }

    #[test]
    fn a_collection_costs_one_request_and_no_level_lookup() {
        let (server, site) = mock(|_call, _target| {
            page(&[
                wire_plate(88461, "Level 1", 632, "Recreation", &ROWS),
                wire_plate(88462, "Level 2", 632, "Recreation", &ROWS),
            ])
        });
        let plan = site
            .plan(
                &Target::Collection(CollectionRef {
                    path: "collection/632".into(),
                    scope: Scope::All,
                }),
                &ProgressBar::hidden(),
            )
            .unwrap();
        // the listing carries the boards, so nothing is left pending
        assert_eq!(plan.work.len(), 2);
        assert!(plan.work.iter().all(|work| matches!(work, Work::Done(..))));
        match &plan.work[0] {
            Work::Done(level, outcome) => {
                assert_eq!(level.path, "level/88461");
                match outcome.as_ref() {
                    Outcome::Level(facts, board) => {
                        assert_eq!(facts.title, "Level 1");
                        assert_eq!(facts.author, "Werner Ettinger + xht2xsb");
                        assert_eq!(facts.collection_name, "Recreation");
                        assert_eq!(facts.url, "https://www.letslogic.com/level/88461");
                        assert_eq!(facts.notes, ["3x3", "record 1191/284"]);
                        assert_eq!(board, "###\n#@$\n##.");
                    }
                    Outcome::Skip(_, skip) => panic!("expected a board, got {:?}", skip.reason),
                }
            }
            Work::Pending(_) => panic!("a listing's levels are already fetched"),
        }
        assert_eq!(server.calls(), ["/collection/632?page=1&items=100"]);
    }

    #[test]
    fn a_long_collection_is_walked_page_by_page() {
        let first: Vec<String> = (1..=100)
            .map(|id| wire_plate(id, &format!("Level {id}"), 632, "Recreation", &ROWS))
            .collect();
        let (server, site) = mock(move |_call, target| match target {
            "/collection/632?page=1&items=100" => page(&first),
            "/collection/632?page=2&items=100" => page(&[
                wire_plate(101, "Level 101", 632, "Recreation", &ROWS),
                wire_plate(102, "Level 102", 632, "Recreation", &ROWS),
            ]),
            _ => Action::Respond(Response::plain(500, "unexpected page")),
        });
        let plan = site
            .plan(
                &Target::Collection(CollectionRef {
                    path: "collection/632".into(),
                    scope: Scope::All,
                }),
                &ProgressBar::hidden(),
            )
            .unwrap();
        assert_eq!(plan.work.len(), 102);
        assert_eq!(
            server.calls(),
            [
                "/collection/632?page=1&items=100",
                "/collection/632?page=2&items=100"
            ]
        );
    }

    #[test]
    fn fetch_converts_one_level() {
        let (server, site) = mock(|_call, _target| {
            page(&[wire_plate(89411, "Level 957", 632, "Recreation", &ROWS)])
        });
        let level = LevelRef::new("89411", "", "89411").with_path("level/89411");
        match site.fetch(&level).unwrap() {
            Outcome::Level(facts, board) => {
                assert_eq!(facts.id, "89411");
                assert_eq!(facts.collection_name, "Recreation");
                assert_eq!(board, "###\n#@$\n##.");
            }
            Outcome::Skip(_, skip) => panic!("expected a board, got {:?}", skip.reason),
        }
        assert_eq!(server.calls(), ["/level/89411"]);
    }

    #[test]
    fn a_map_xsb_cannot_take_is_skipped_rather_than_fetched_again() {
        // two players: the map is the site's, but no loader will take it
        let (_server, site) = mock(|_call, _target| {
            page(&[wire_plate(
                7,
                "Level 7",
                632,
                "Recreation",
                &["111", "123", "1@4"],
            )])
        });
        let level = LevelRef::new("7", "", "7").with_path("level/7");
        match site.fetch(&level).unwrap() {
            Outcome::Skip(facts, skip) => {
                assert_eq!(skip.kind, SkipKind::Invalid);
                assert_eq!(facts.id, "7", "a skipped level still describes itself");
            }
            Outcome::Level(..) => panic!("a two-player map must not be kept"),
        }
    }

    #[test]
    fn a_missing_level_or_collection_names_its_path() {
        // the site redirects a missing level to a page without a plate
        let (_server, site) = mock(|_call, _target| page(&[]));
        let err = site
            .fetch(&LevelRef::new("999", "", "999").with_path("level/999"))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "level not found: level/999 (https://www.letslogic.com/level/999)"
        );

        let err = site
            .inspect(
                &CollectionRef {
                    path: "collection/999".into(),
                    scope: Scope::All,
                },
                &ProgressBar::hidden(),
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "collection not found: collection/999 (https://www.letslogic.com/collection/999)"
        );
    }

    #[test]
    fn resume_reads_the_id_out_of_a_url() {
        let site = LetsLogic::new(Duration::ZERO, Duration::from_secs(5), stop());
        assert_eq!(
            site.id_from_url("https://www.letslogic.com/level/89411")
                .as_deref(),
            Some("89411")
        );
        assert_eq!(
            site.id_from_url("https://www.letslogic.com/collection/632"),
            None
        );
        assert_eq!(site.id_from_url("https://example.com/level/89411"), None);
    }
}
