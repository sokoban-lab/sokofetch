//! The site abstraction: everything `main` needs from a Sokoban website.
//!
//! A site turns user input into a [`Target`], lists what a collection holds,
//! and fetches levels as XSB. No wire format, URL shape or board encoding
//! crosses this boundary, so `main` is unchanged when a site is added.
//!
//! # Adding a site
//!
//! 1. Add `src/sites/<name>/` with an `impl Site`, following `sokobanonline`:
//!    `target` claims the inputs the site owns and returns `None` for the rest,
//!    `inspect`/`plan` list a collection, `fetch` returns one level.
//! 2. Convert its board encoding to XSB and pass the result to
//!    [`crate::xsb::validate`]. A board the format cannot express (a mechanism,
//!    a co-op level) is an [`Outcome::Skip`] with [`SkipKind::Unsupported`], so
//!    it is counted and reported instead of silently disappearing.
//! 3. Register it in [`all`] — after the sites whose bare paths it could also
//!    match, because the first site that claims an input wins.
//! 4. Record what the site stores and what its `robots.txt` allows in
//!    `docs/api/<name>.md`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::bail;
use indicatif::ProgressBar;

pub mod sokobanonline;

/// Every site this build knows, in the order an input is offered to them.
pub fn all(delay: Duration, timeout: Duration, stop: Arc<AtomicBool>) -> Vec<Box<dyn Site>> {
    vec![Box::new(sokobanonline::SokobanOnline::new(
        delay, timeout, stop,
    ))]
}

/// Pick the site an input addresses. The first site that claims the input
/// wins, so a site whose bare paths overlap an earlier one's has to be
/// addressed by its page URL.
pub fn resolve<'a>(sites: &'a [Box<dyn Site>], input: &str) -> anyhow::Result<&'a dyn Site> {
    for site in sites {
        if site.target(input)?.is_some() {
            return Ok(site.as_ref());
        }
    }
    // A page URL already names its host, so pasting it again would not help;
    // a bare path leaves the site open, and that is what the URL settles.
    if input.starts_with("http://") || input.starts_with("https://") {
        bail!("no supported site addresses {input:?}");
    }
    bail!("no supported site addresses {input:?}; paste the page URL instead")
}

/// What an input addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A collection of levels.
    Collection(CollectionRef),
    /// One level on its own.
    Level(LevelRef),
}

impl Target {
    /// Canonical path within the site, used in messages and in file names.
    pub fn path(&self) -> &str {
        match self {
            Self::Collection(collection) => &collection.path,
            Self::Level(level) => &level.path,
        }
    }
}

/// A collection: a page, and which of its levels the input names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRef {
    /// Canonical path within the site, e.g. `t1024/corner-set`.
    pub path: String,
    /// Which levels of the page it names.
    pub scope: Scope,
}

/// Which levels of a page a target names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The page and everything below it.
    All,
    /// Only the levels that sit directly on the page, outside every group.
    Page,
}

/// One level: enough to fetch it, and enough to name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelRef {
    /// The site's id for the level; unique within its collection.
    pub id: String,
    /// The collection it belongs to, as a canonical path.
    pub collection: String,
    /// What to call it in progress output.
    pub label: String,
    /// Canonical path, e.g. `obyzz011/remakes/135023_morch`. A listing may know
    /// only `obyzz011/remakes/135023`, since the slug is decoration.
    pub path: String,
}

impl LevelRef {
    /// A level addressed by its canonical path, `<collection>/<id>`.
    pub fn new(
        id: impl Into<String>,
        collection: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        let (id, collection) = (id.into(), collection.into());
        Self {
            path: format!("{collection}/{id}"),
            id,
            collection,
            label: label.into(),
        }
    }

    /// Replace the path with the one an input spelled, e.g. a slug the site
    /// treats as decoration.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

/// A collection as a site reports it, without any board.
#[derive(Debug, Default)]
pub struct Collection {
    /// Display name.
    pub name: String,
    /// The page it lives on, as a URL.
    pub page: String,
    /// The site's own level count, when it reports one.
    pub total: Option<u64>,
    /// Named groups of levels below this page.
    pub groups: Vec<Group>,
    /// The levels that sit directly on the page, outside every group.
    pub own: Vec<LevelRef>,
}

/// A named group of levels inside a collection: a sub-pack, a folder, a
/// chapter — whatever the site calls it.
#[derive(Debug)]
pub struct Group {
    /// Canonical path within the site.
    pub path: String,
    /// Display name.
    pub name: String,
    /// The site's own count, when it reports one.
    pub count: Option<u64>,
}

/// A collection plus the levels to fetch for it.
#[derive(Debug)]
pub struct Plan {
    /// What the output file describes.
    pub collection: Collection,
    /// The levels, in fetch order.
    pub work: Vec<Work>,
}

/// One level to process.
#[derive(Debug)]
pub enum Work {
    /// Still to be fetched.
    Pending(LevelRef),
    /// Fetched while planning, so the run does not pay for it twice: a
    /// single-level target arrives this way, its detail *is* the plan.
    Done(LevelRef, Box<Outcome>),
}

impl Work {
    pub fn level(&self) -> &LevelRef {
        match self {
            Self::Pending(level) => level,
            Self::Done(level, _) => level,
        }
    }
}

/// What fetching one level produced.
#[derive(Debug)]
pub enum Outcome {
    /// The level and its board, already in XSB.
    Level(Level, String),
    /// The level, and why it cannot enter the collection.
    Skip(Level, Skip),
}

/// Everything the collection file records about a level, minus the board.
#[derive(Debug)]
pub struct Level {
    /// The site's id, matching the [`LevelRef`] it was fetched from.
    pub id: String,
    /// The `Title:` line.
    pub title: String,
    /// The `Author:` line.
    pub author: String,
    /// The collection it belongs to, as the site names it.
    pub collection_name: String,
    /// The level's page URL, for the `; URL:` line.
    pub url: String,
    /// Publication date (`YYYY-MM-DD`), for the `; Published:` line.
    pub published: Option<String>,
    /// Extra facts `info` prints after the label, e.g. `classic`, `15x15`.
    pub notes: Vec<String>,
}

/// Why a level was left out, as a category the summary can count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    /// The site's board encoding cannot be expressed in XSB.
    Unsupported,
    /// The board is only shown to signed-in members.
    Locked,
    /// The board did not pass XSB validation.
    Invalid,
}

impl SkipKind {
    /// Label for the summary line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unsupported => "not expressible in XSB",
            Self::Locked => "locked",
            Self::Invalid => "rejected by the XSB parser",
        }
    }
}

/// One level left out, and why.
#[derive(Debug)]
pub struct Skip {
    pub kind: SkipKind,
    /// Human text for `--verbose`, e.g. `modern level`.
    pub reason: String,
}

impl Skip {
    pub fn new(kind: SkipKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

/// What `main` needs from a Sokoban website.
pub trait Site {
    /// Name for the file header and for messages, e.g. `Sokoban Online`.
    fn name(&self) -> &'static str;

    /// Parse an input into a target this site owns, or `None` when the input
    /// is not addressed to it (another host, another path shape).
    fn target(&self, input: &str) -> anyhow::Result<Option<Target>>;

    /// What a collection holds: its groups and the levels on its page. No
    /// board is fetched.
    fn inspect(
        &self,
        collection: &CollectionRef,
        progress: &ProgressBar,
    ) -> anyhow::Result<Collection>;

    /// The levels a target names, in fetch order.
    fn plan(&self, target: &Target, progress: &ProgressBar) -> anyhow::Result<Plan>;

    /// Fetch one level and convert its board to XSB.
    fn fetch(&self, level: &LevelRef) -> anyhow::Result<Outcome>;

    /// The id a `; URL:` line names, for `--resume`. `None` when the line is
    /// not this site's.
    fn id_from_url(&self, url: &str) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sites() -> Vec<Box<dyn Site>> {
        all(
            Duration::ZERO,
            Duration::from_secs(5),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn resolve_returns_the_site_that_claims_the_input() {
        let known = sites();
        let site = resolve(&known, "t1024/corner-set").unwrap();
        assert_eq!(site.name(), "Sokoban Online");
        let site = resolve(
            &known,
            "https://www.sokobanonline.com/play/community/t1024/corner-set",
        )
        .unwrap();
        assert_eq!(site.name(), "Sokoban Online");
    }

    #[test]
    fn resolve_rejects_inputs_no_site_owns() {
        let known = sites();
        // another site's page is nobody's business here
        let err = resolve(&known, "https://www.letslogic.com/level/89411")
            .err()
            .unwrap();
        let text = err.to_string();
        assert!(text.contains("no supported site"), "{text}");
        // the input already named a host, so there is nothing to paste instead
        assert!(!text.contains("paste"), "{text}");
        // a bare path leaves the site open, so that is the case worth advising
        let err = resolve(&known, "   ").err().unwrap();
        assert!(err.to_string().contains("empty collection path"), "{err}");
    }
}
