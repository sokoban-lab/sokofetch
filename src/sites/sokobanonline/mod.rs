//! sokobanonline.com: a JSON REST API behind a Vue front end.
//!
//! What the site stores and how its boards are encoded is in
//! `docs/api/sokobanonline.md`; this module only maps it onto the [`Site`]
//! abstraction.

mod api;
mod board;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use indicatif::ProgressBar;

use super::{
    Address, Collection, CollectionRef, Group, Level, LevelRef, Outcome, Plan, Scope, Site, Skip,
    SkipKind, Target, Work, address,
};

/// Host the collection paths and page URLs belong to.
const HOST: &str = "sokobanonline.com";
/// Prefix collection paths are relative to.
const PREFIX: &str = "play/community/";

/// A client for one of the sites this tool understands.
pub struct SokobanOnline {
    client: api::Client,
}

impl SokobanOnline {
    pub fn new(delay: Duration, timeout: Duration, stop: Arc<AtomicBool>) -> Self {
        Self {
            client: api::Client::new(delay, timeout, stop),
        }
    }

    /// A site aimed at a mock server: tests point `base` at its URL and
    /// collapse the retry backoff to zero.
    #[cfg(test)]
    pub(crate) fn with_base(
        base: &str,
        delay: Duration,
        timeout: Duration,
        retry_backoff: Duration,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client: api::Client::with_base(base, delay, timeout, retry_backoff, stop),
        }
    }

    /// A site aimed at a mock server, with the usual test pacing.
    #[cfg(test)]
    pub(crate) fn test_client(base: &str, stop: Arc<AtomicBool>) -> Self {
        Self {
            client: api::Client::test_client(base, stop),
        }
    }
}

impl Site for SokobanOnline {
    fn name(&self) -> &'static str {
        "Sokoban Online"
    }

    /// `<author>`, `<author>/<pack>`, `<author>/` for the levels sitting
    /// directly on the page, or `<author>[/<pack>]/<id>_<slug>` for one level —
    /// or the page URL any of them appears in.
    fn target(&self, input: &str) -> anyhow::Result<Option<Target>> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "empty collection path");
        let path = match address(trimmed, HOST) {
            // A link to some other site: not ours to parse.
            Address::Foreign => return Ok(None),
            Address::Bare => trimmed,
            Address::Owned(tail) => {
                // A link may carry a query or a fragment; neither names a
                // collection.
                let tail = tail.split(['?', '#']).next().unwrap_or_default();
                let path = tail.strip_prefix(PREFIX).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unsupported URL {trimmed:?}: expected https://www.{HOST}/{PREFIX}<path>"
                    )
                })?;
                ensure!(
                    !path.trim_end_matches('/').is_empty(),
                    "unsupported URL {trimmed:?}: it names no collection"
                );
                path
            }
        };
        // A trailing slash names the empty collection, i.e. the levels sitting
        // on the page itself. A pack holds no sub-packs, so there the empty
        // name resolves to the pack's own levels — the same thing. A level is
        // not a collection either, so the slash adds nothing there as well.
        let (path, unnamed) = match path.strip_suffix('/') {
            Some(rest) => (rest, true),
            None => (path, false),
        };
        ensure!(
            path.split('/').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            }),
            "invalid collection path {trimmed:?} (expected <author>[/<pack>][/<id>_<slug>], \
             allowed characters [a-zA-Z0-9_-])"
        );
        let segments: Vec<&str> = path.split('/').collect();
        if let Some(level) = parse_level(&segments) {
            return Ok(Some(Target::Level(level)));
        }
        Ok(Some(Target::Collection(CollectionRef {
            path: path.to_owned(),
            // Only a single-segment path can name the unnamed collection; a
            // pack's trailing slash adds nothing.
            scope: if unnamed && segments.len() == 1 {
                Scope::Page
            } else {
                Scope::All
            },
        })))
    }

    fn inspect(
        &self,
        collection: &CollectionRef,
        progress: &ProgressBar,
    ) -> anyhow::Result<Collection> {
        let (pack, listing) = self.client.list_pack(&collection.path, progress)?;
        Ok(described(&pack, listing, &collection.path))
    }

    fn plan(&self, target: &Target, progress: &ProgressBar) -> anyhow::Result<Plan> {
        match target {
            Target::Level(level) => self.plan_level(level),
            Target::Collection(collection) => self.plan_collection(collection, progress),
        }
    }

    fn fetch(&self, level: &LevelRef) -> anyhow::Result<Outcome> {
        let puzzle = self.puzzle(&level.path, &level.id)?;
        let pack = pack_of(&puzzle, &level.collection);
        Ok(outcome_of(&puzzle, &pack))
    }

    fn id_from_url(&self, url: &str) -> Option<String> {
        let path = url.strip_prefix(&format!("https://www.{HOST}/{PREFIX}"))?;
        let tail = path.rsplit('/').next()?;
        let id = tail.split('_').next()?;
        (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())).then(|| id.to_owned())
    }
}

impl SokobanOnline {
    /// Fetch one level's detail, naming its path when the site has no such
    /// level.
    fn puzzle(&self, path: &str, id: &str) -> anyhow::Result<api::Puzzle> {
        match self.client.fetch_puzzle(puzzle_id(id)?) {
            Err(err) if err.downcast_ref::<api::NotFound>().is_some() => {
                bail!("level not found: {path} ({})", level_url(path))
            }
            other => other,
        }
    }

    /// One level: the detail is both the plan and the board, so the run costs
    /// a single request instead of a listing plus a lookup.
    fn plan_level(&self, target: &LevelRef) -> anyhow::Result<Plan> {
        let puzzle = self.puzzle(&target.path, &target.id)?;
        // The detail names the pack but reports only its slug, so the page
        // path is derived; a detail without one falls back to the path.
        let pack = pack_of(&puzzle, &target.collection);
        let collection = Collection {
            name: pack.name.clone(),
            page: page_url(&pack),
            total: None,
            groups: Vec::new(),
            own: vec![target.clone()],
        };
        let outcome = outcome_of(&puzzle, &pack);
        Ok(Plan {
            collection,
            work: vec![Work::Done(target.clone(), Box::new(outcome))],
        })
    }

    /// A collection: its sub-packs' levels first, then the ones sitting on the
    /// page itself, or only those when the target names the unnamed collection.
    fn plan_collection(
        &self,
        target: &CollectionRef,
        progress: &ProgressBar,
    ) -> anyhow::Result<Plan> {
        // The unnamed collection takes the page's own levels only, so the
        // sub-packs are neither listed nor fetched.
        if target.scope == Scope::Page {
            let (pack, listing) = self.client.list_pack(&target.path, progress)?;
            let collection = described(&pack, listing, &target.path);
            let work = collection.own.iter().cloned().map(Work::Pending).collect();
            return Ok(Plan { collection, work });
        }
        let (pack, items) = self.client.fetch_pack(&target.path, progress)?;
        // `fetch` only needs what the file header prints; the groups are for
        // `info`, which reads them from `inspect`.
        let collection = Collection {
            name: pack.name.clone(),
            page: page_url(&pack),
            total: pack.num_puzzles,
            groups: Vec::new(),
            own: Vec::new(),
        };
        let work = items
            .iter()
            .map(|item| Work::Pending(level_ref(item, &target.path)))
            .collect();
        Ok(Plan { collection, work })
    }
}

/// Read the level out of a path that ends in a level segment, i.e. `<id>` or
/// `<id>_<slug>`. A level is never addressed by its bare id, so a one-segment
/// path stays an author; `<author>/<id>_<slug>` is a level on the page.
fn parse_level(segments: &[&str]) -> Option<LevelRef> {
    let (last, collection) = segments.split_last()?;
    if collection.is_empty() {
        return None;
    }
    let (id, slug) = last.split_once('_').unwrap_or((last, ""));
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id: i64 = id.parse().ok()?;
    let collection = collection.join("/");
    let level = LevelRef::new(id.to_string(), collection, id.to_string());
    Some(match slug {
        "" => level,
        // The slug is decoration, but keeping it makes messages match the
        // input the user pasted.
        slug => {
            let path = format!("{}_{slug}", level.path);
            level.with_path(path)
        }
    })
}

/// A listing entry as the fetch loop knows it.
fn level_ref(item: &api::PuzzleItem, collection: &str) -> LevelRef {
    LevelRef::new(
        item.puzzle_id.to_string(),
        collection.to_owned(),
        item.label(),
    )
}

/// A listing as the [`Site`] abstraction describes it.
fn described(pack: &api::Pack, listing: api::Listing, path: &str) -> Collection {
    // On a page without sub-packs the listing reports its levels as items; on
    // a container, as the bundle outside them.
    let mut own = listing.puzzles;
    own.extend(listing.loose);
    Collection {
        name: pack.name.clone(),
        page: page_url(pack),
        total: pack.num_puzzles,
        groups: listing
            .packs
            .iter()
            .map(|sub| Group {
                path: api::display_path(&sub.full_slug).to_owned(),
                name: sub.name.clone(),
                count: sub.num_puzzles,
            })
            .collect(),
        own: own.iter().map(|item| level_ref(item, path)).collect(),
    }
}

/// The pack a level belongs to: the detail names it, but reports only its
/// slug, so `Pack::page_path` derives the page path from that. A detail
/// without a pack falls back to the collection the target names.
fn pack_of(puzzle: &api::Puzzle, collection: &str) -> api::Pack {
    puzzle.pack.clone().unwrap_or_else(|| api::Pack {
        name: collection
            .rsplit('/')
            .next()
            .unwrap_or(collection)
            .to_owned(),
        full_slug: format!("community/{collection}"),
        path: format!("/play/community/{collection}"),
        num_puzzles: None,
    })
}

/// Convert one detail into a board, or the reason there is none.
fn outcome_of(puzzle: &api::Puzzle, pack: &api::Pack) -> Outcome {
    let level = Level {
        id: puzzle.puzzle_id.to_string(),
        title: puzzle.name.clone(),
        author: puzzle.author_name(pack).to_owned(),
        collection_name: pack.name.clone(),
        url: format!("https://www.{HOST}{}", puzzle.path),
        published: puzzle.publish_date().map(str::to_owned),
        notes: notes(puzzle),
    };
    if puzzle.guest_locked {
        return Outcome::Skip(level, Skip::new(SkipKind::Locked, "guest-locked"));
    }
    if puzzle.puzzle_type != "classic" {
        // A modern design packs four chars per cell; XSB cannot say that
        // (docs/api/sokobanonline.md §4).
        return Outcome::Skip(
            level,
            Skip::new(
                SkipKind::Unsupported,
                format!("{} level", puzzle.puzzle_type),
            ),
        );
    }
    match board::to_xsb(&puzzle.design) {
        Ok(board) => Outcome::Level(level, board),
        Err(reason) => Outcome::Skip(level, Skip::new(SkipKind::Invalid, reason)),
    }
}

/// The facts `info` prints after a level's label.
fn notes(puzzle: &api::Puzzle) -> Vec<String> {
    let mut notes = vec![puzzle.puzzle_type.clone()];
    if let (Some(width), Some(height)) = (puzzle.width, puzzle.height) {
        notes.push(format!("{width}x{height}"));
    }
    if puzzle.guest_locked {
        notes.push("guest-locked".to_owned());
    }
    notes
}

fn puzzle_id(id: &str) -> anyhow::Result<i64> {
    id.parse()
        .with_context(|| format!("{id:?} is not a level id"))
}

fn page_url(pack: &api::Pack) -> String {
    format!("https://www.{HOST}{}", pack.page_path())
}

fn level_url(path: &str) -> String {
    format!("https://www.{HOST}/{PREFIX}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        Action, MockServer, Response, wire_detail, wire_listing, wire_listing_loose, wire_pack,
        wire_pack_counted, wire_pack_item, wire_puzzle_item,
    };

    fn site(input: &str) -> Target {
        SokobanOnline::new(Duration::ZERO, Duration::from_secs(5), stop())
            .target(input)
            .expect("valid input")
            .expect("this site owns the input")
    }

    fn stop() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn collection(path: &str, scope: Scope) -> Target {
        Target::Collection(CollectionRef {
            path: path.to_owned(),
            scope,
        })
    }

    fn level(collection: &str, id: i64, slug: &str) -> Target {
        let path = if slug.is_empty() {
            format!("{collection}/{id}")
        } else {
            format!("{collection}/{id}_{slug}")
        };
        Target::Level(LevelRef {
            id: id.to_string(),
            collection: collection.to_owned(),
            label: id.to_string(),
            path,
        })
    }

    #[test]
    fn collection_paths_are_parsed_and_validated() {
        assert_eq!(site("a"), collection("a", Scope::All));
        assert_eq!(site(" a/b "), collection("a/b", Scope::All));
        // the empty name after the slash is the collection that has none
        assert_eq!(site("a/"), collection("a", Scope::Page));
        assert_eq!(site(" a/ "), collection("a", Scope::Page));
        // a pack holds no sub-packs, so its empty name resolves to itself
        assert_eq!(site("a/b/"), collection("a/b", Scope::All));

        // a trailing level segment addresses that level, not a collection
        assert_eq!(
            site("obyzz011/remakes/135023_morch"),
            level("obyzz011/remakes", 135023, "morch")
        );
        assert_eq!(
            site(" obyzz011/remakes/135023_morch/ "),
            level("obyzz011/remakes", 135023, "morch")
        );
        // a level sitting directly on the author's page
        assert_eq!(
            site("obyzz011/135462_puzzle-error"),
            level("obyzz011", 135462, "puzzle-error")
        );
        // the slug is decoration: the id alone addresses the same level
        assert_eq!(
            site("obyzz011/remakes/135023"),
            level("obyzz011/remakes", 135023, "")
        );
        // ... but a level always needs an author in front of it
        assert_eq!(site("135023"), collection("135023", Scope::All));
    }

    #[test]
    fn invalid_paths_are_rejected() {
        let site = SokobanOnline::new(Duration::ZERO, Duration::from_secs(5), stop());
        for input in ["", " ", "/", "a//", "a b", "a/β"] {
            assert!(site.target(input).is_err(), "{input:?} must be rejected");
        }
    }

    #[test]
    fn page_urls_address_the_same_things_as_paths() {
        let base = "https://www.sokobanonline.com/play/community";

        assert_eq!(
            site(&format!("{base}/obyzz011")),
            collection("obyzz011", Scope::All)
        );
        // the empty name after the slash still means the levels on the page
        assert_eq!(
            site(&format!("{base}/obyzz011/")),
            collection("obyzz011", Scope::Page)
        );
        assert_eq!(
            site(&format!("{base}/obyzz011/remakes")),
            collection("obyzz011/remakes", Scope::All)
        );
        assert_eq!(
            site(&format!("{base}/obyzz011/remakes/135023_morch")),
            level("obyzz011/remakes", 135023, "morch")
        );
        // http, no `www.`, a query or a fragment, and surrounding blanks
        assert_eq!(
            site("http://sokobanonline.com/play/community/obyzz011"),
            collection("obyzz011", Scope::All)
        );
        assert_eq!(
            site(" https://www.sokobanonline.com/play/community/obyzz011?ref=list "),
            collection("obyzz011", Scope::All)
        );
        assert_eq!(
            site(&format!("{base}/obyzz011/remakes#levels")),
            collection("obyzz011/remakes", Scope::All)
        );
    }

    #[test]
    fn links_to_other_sites_are_not_ours() {
        let site = SokobanOnline::new(Duration::ZERO, Duration::from_secs(5), stop());
        for input in [
            "https://example.com/play/community/obyzz011",
            "http://www.letslogic.com/level/89411",
            "www.example.com/a",
        ] {
            assert_eq!(site.target(input).unwrap(), None, "{input:?}");
        }
    }

    #[test]
    fn unsupported_urls_say_what_was_expected() {
        let site = SokobanOnline::new(Duration::ZERO, Duration::from_secs(5), stop());
        let err = site
            .target("https://www.sokobanonline.com/play/web-archive/a/b")
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("expected https://www.sokobanonline.com/play/community/<path>"),
            "{err:#}"
        );
        let err = site
            .target("https://www.sokobanonline.com/play/community/")
            .unwrap_err();
        assert!(err.to_string().contains("names no collection"), "{err:#}");
        assert!(
            site.target("https://www.sokobanonline.com/").is_err(),
            "a bare host names no collection"
        );
    }

    #[test]
    fn a_level_path_names_its_page_and_its_id() {
        let Target::Level(level) = site("obyzz011/remakes/135023_morch") else {
            panic!("expected a level target");
        };
        assert_eq!(level.path, "obyzz011/remakes/135023_morch");
        assert_eq!(
            level_url(&level.path),
            "https://www.sokobanonline.com/play/community/obyzz011/remakes/135023_morch"
        );
        assert_eq!(level.id, "135023");
        assert_eq!(level.collection, "obyzz011/remakes");
    }

    #[test]
    fn resume_reads_the_id_out_of_a_url() {
        let site = SokobanOnline::new(Duration::ZERO, Duration::from_secs(5), stop());
        let url = "https://www.sokobanonline.com/play/community/t1024/1_corner01";
        assert_eq!(site.id_from_url(url).as_deref(), Some("1"));
        let url = "https://www.sokobanonline.com/play/community/t1024/135023";
        assert_eq!(site.id_from_url(url).as_deref(), Some("135023"));
        assert_eq!(site.id_from_url("https://example.com/level/89411"), None);
        assert_eq!(site.id_from_url("https://example.com/x/abc_def"), None);
    }

    // ---- planning and fetching against a local mock of the API ----

    fn mock(
        handler: impl Fn(usize, &str) -> Action + Send + Sync + 'static,
    ) -> (Arc<MockServer>, SokobanOnline) {
        let server = MockServer::start();
        server.set_handler(handler);
        let site = SokobanOnline::test_client(&server.base, stop());
        (server, site)
    }

    fn detail(id: i64, kind: &str, design: &str, locked: bool) -> Action {
        Action::Respond(Response::json(200, wire_detail(id, kind, design, locked)))
    }

    #[test]
    fn inspect_reports_the_groups_and_the_levels_on_the_page() {
        let (_server, site) = mock(|_call, _target| {
            Action::Respond(Response::json(
                200,
                wire_listing_loose(
                    Some(wire_pack_counted("t1024", 2)),
                    vec![wire_pack_item("community/t1024/corner-set")],
                    1,
                    vec![wire_puzzle_item(99)],
                ),
            ))
        });
        let collection = site
            .inspect(
                &CollectionRef {
                    path: "t1024".into(),
                    scope: Scope::All,
                },
                &ProgressBar::hidden(),
            )
            .unwrap();
        assert_eq!(collection.name, "t1024");
        assert_eq!(
            collection.page,
            "https://www.sokobanonline.com/play/community/t1024"
        );
        assert_eq!(collection.total, Some(2));
        assert_eq!(collection.groups.len(), 1);
        assert_eq!(collection.groups[0].path, "t1024/corner-set");
        assert_eq!(collection.own.len(), 1);
        assert_eq!(collection.own[0].id, "99");
        assert_eq!(collection.own[0].label, "p99 (99)");
    }

    #[test]
    fn plan_of_a_container_takes_the_sub_packs_then_the_page() {
        let (server, site) = mock(|_call, target| match target {
            "/packs/by-path/community/t1024?page=1" => Action::Respond(Response::json(
                200,
                wire_listing_loose(
                    Some(wire_pack_counted("t1024", 2)),
                    vec![wire_pack_item("community/t1024/corner-set")],
                    1,
                    vec![wire_puzzle_item(99)],
                ),
            )),
            "/packs/by-path/community/t1024/corner-set?page=1" => Action::Respond(Response::json(
                200,
                wire_listing(Some(wire_pack("corner-set")), vec![wire_puzzle_item(11)], 1),
            )),
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        });
        let plan = site
            .plan(&collection("t1024", Scope::All), &ProgressBar::hidden())
            .unwrap();
        let ids: Vec<&str> = plan
            .work
            .iter()
            .map(|work| work.level().id.as_str())
            .collect();
        assert_eq!(ids, ["11", "99"], "the sub-pack's levels, then the page's");
        assert_eq!(plan.collection.name, "t1024");
        assert_eq!(server.calls().len(), 2);
    }

    #[test]
    fn plan_of_the_page_scope_skips_the_sub_packs() {
        let (server, site) = mock(|_call, _target| {
            Action::Respond(Response::json(
                200,
                wire_listing_loose(
                    Some(wire_pack_counted("t1024", 2)),
                    vec![wire_pack_item("community/t1024/corner-set")],
                    1,
                    vec![wire_puzzle_item(99)],
                ),
            ))
        });
        let plan = site
            .plan(&collection("t1024", Scope::Page), &ProgressBar::hidden())
            .unwrap();
        let ids: Vec<&str> = plan
            .work
            .iter()
            .map(|work| work.level().id.as_str())
            .collect();
        assert_eq!(ids, ["99"]);
        assert_eq!(server.calls().len(), 1, "the sub-pack is not listed");
    }

    #[test]
    fn plan_of_a_level_fetches_it_once_and_keeps_it() {
        let (server, site) = mock(|_call, target| match target {
            "/puzzles/135023" => detail(135023, "classic", "#####\n#@$.#\n#####", false),
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        });
        let plan = site
            .plan(
                &level("obyzz011/remakes", 135023, "morch"),
                &ProgressBar::hidden(),
            )
            .unwrap();
        assert_eq!(
            plan.collection.name, "remakes",
            "the detail names no pack, so the path tail stands in"
        );
        assert_eq!(plan.work.len(), 1);
        match &plan.work[0] {
            Work::Done(level, outcome) => {
                assert_eq!(level.id, "135023");
                assert!(matches!(**outcome, Outcome::Level(_, _)));
            }
            Work::Pending(_) => panic!("the detail is already in hand"),
        }
        assert_eq!(server.calls(), ["/puzzles/135023"], "no second request");
    }

    #[test]
    fn a_missing_level_is_an_error_that_names_its_path() {
        let (_server, site) = mock(|_call, _target| Action::Respond(Response::json(404, "{}")));
        let err = site
            .plan(
                &level("obyzz011/remakes", 135023, "morch"),
                &ProgressBar::hidden(),
            )
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("level not found: obyzz011/remakes/135023_morch"),
            "{err:#}"
        );
    }

    #[test]
    fn fetch_converts_a_classic_board() {
        let (_server, site) = mock(|_call, target| match target {
            "/puzzles/11" => detail(11, "classic", "#####\n#@$.#\n#####", false),
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        });
        let outcome = site
            .fetch(&LevelRef::new("11", "t1024", "p11 (11)"))
            .unwrap();
        match outcome {
            Outcome::Level(level, board) => {
                assert_eq!(level.title, "p11");
                assert_eq!(level.author, "t1024", "the pack name stands in");
                assert_eq!(
                    level.url,
                    "https://www.sokobanonline.com/play/community/t1024/11"
                );
                assert_eq!(level.published.as_deref(), Some("2023-08-18"));
                assert_eq!(level.notes, ["classic"]);
                assert_eq!(board, "#####\n#@$.#\n#####");
            }
            Outcome::Skip(_, skip) => panic!("expected a board, got {:?}", skip.reason),
        }
    }

    #[test]
    fn fetch_skips_what_xsb_cannot_take() {
        let cases = [
            ("modern", "____", false, SkipKind::Unsupported),
            ("classic", "#####\n#@$.#\n#####", true, SkipKind::Locked),
            (
                "classic",
                "#####\n#$$.#\n# @ #\n#####",
                false,
                SkipKind::Invalid,
            ),
        ];
        for (kind, design, locked, expected) in cases {
            let (_server, site) = mock(move |_call, _target| detail(11, kind, design, locked));
            let outcome = site
                .fetch(&LevelRef::new("11", "t1024", "p11 (11)"))
                .unwrap();
            match outcome {
                Outcome::Skip(level, skip) => {
                    assert_eq!(skip.kind, expected, "{}: {}", kind, skip.reason);
                    assert_eq!(level.id, "11", "a skipped level still describes itself");
                }
                Outcome::Level(..) => panic!("{kind}/{design:?} must not be kept"),
            }
        }
    }

    #[test]
    fn a_failing_level_fetch_is_an_error_not_a_skip() {
        let (_server, site) = mock(|_call, _target| Action::Respond(Response::plain(403, "no")));
        assert!(
            site.fetch(&LevelRef::new("11", "t1024", "p11 (11)"))
                .is_err()
        );
    }

    #[test]
    fn level_ids_that_are_not_numbers_are_rejected() {
        let err = puzzle_id("morch").unwrap_err();
        assert!(err.to_string().contains("not a level id"), "{err:#}");
    }
}
