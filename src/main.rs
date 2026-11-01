//! Inspect and fetch level collections from Sokoban websites.
//!
//! A collection is a page plus every level below it: the levels of its groups
//! (sub-packs, folders) and the ones sitting directly on the page. `info`
//! describes both kinds and `fetch` downloads both, so neither needs a flag to
//! be complete.
//!
//! How a collection is addressed is the site's business — [`sites::Site`] owns
//! that — so this file only knows targets, levels, and the XSB file they end
//! up in. Sites live in `src/sites/`; `sites::all` lists them and
//! `sites::resolve` picks the one an input addresses.

mod http;
mod sites;
#[cfg(test)]
mod test_support;
mod xsb;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use crate::sites::{
    Collection, CollectionRef, Level, LevelRef, Outcome, Scope, Site, SkipKind, Target, Work,
};

/// Fetch levels of a collection from a Sokoban website and write an XSB level
/// collection (.txt).
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what a collection holds: its groups and the levels that sit
    /// directly on its page
    Info(InfoArgs),
    /// Fetch a collection's levels as an XSB level collection (.txt)
    Fetch(FetchArgs),
}

/// What every request-making subcommand shares.
#[derive(Debug, Args)]
struct Connection {
    /// Collection path, or the page URL it appears in. What a path may look
    /// like is the site's business — see `docs/api/` — and it may name one
    /// level instead of a collection
    #[arg(value_name = "COLLECTION")]
    collection: String,

    /// Seconds to sleep between requests (Sokoban Online's robots.txt asks
    /// for 5)
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0)]
    delay: f64,

    /// Seconds until a single request times out
    #[arg(long, value_name = "SECONDS", default_value_t = 30.0)]
    timeout: f64,
}

#[derive(Debug, Args)]
struct InfoArgs {
    #[command(flatten)]
    connection: Connection,
}

#[derive(Debug, Args)]
struct FetchArgs {
    #[command(flatten)]
    connection: Connection,

    /// Output file (default: `<COLLECTION>` with '/' replaced by '-'.txt)
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Only fetch the first N puzzles of the collection
    #[arg(long, value_name = "N")]
    limit: Option<usize>,

    /// Print a line for every skipped level (default: a summary breakdown only)
    #[arg(short, long)]
    verbose: bool,

    /// Keep the levels already in the output file and fetch only the missing
    /// ones, appending them
    #[arg(long)]
    resume: bool,
}

impl Command {
    fn connection(&self) -> &Connection {
        match self {
            Self::Info(args) => &args.connection,
            Self::Fetch(args) => &args.connection,
        }
    }
}

fn main() -> ExitCode {
    match try_main() {
        Ok(code) => code,
        Err(err) => {
            let mut chain = err.chain();
            if let Some(top) = chain.next() {
                anstream::eprintln!("{} {top}", paint(anstyle::AnsiColor::Red, "error:"));
            }
            for cause in chain {
                anstream::eprintln!("  caused by: {cause}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Everything `main` does that can fail: parse the args, install the Ctrl+C
/// handler, pick the site the input addresses, then run.
fn try_main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    let connection = cli.command.connection();
    validate(connection)?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("installing the Ctrl+C handler")?;
    }
    let known = sites::all(
        Duration::from_secs_f64(connection.delay),
        Duration::from_secs_f64(connection.timeout),
        stop.clone(),
    );
    let site = sites::resolve(&known, &connection.collection)?;
    run(site, cli.command, &stop)
}

/// Color `text` for the terminal; anstream strips the codes when not a TTY.
fn paint(color: anstyle::AnsiColor, text: impl std::fmt::Display) -> String {
    let style = anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(color)));
    format!("{}{text}{}", style.render(), anstyle::Reset.render())
}

/// Print a line above the progress bar, or plainly when stderr is not a TTY
/// (indicatif drops `println` on hidden draw targets).
fn note(progress: &ProgressBar, line: &str) {
    if console::Term::stderr().is_term() {
        progress.println(line);
    } else {
        eprintln!("{line}");
    }
}

/// Skip counts by category, rendered as a compact summary suffix.
#[derive(Default)]
struct SkipBreakdown {
    unsupported: usize,
    locked: usize,
    invalid: usize,
}

impl SkipBreakdown {
    fn count(&mut self, kind: SkipKind) {
        match kind {
            SkipKind::Unsupported => self.unsupported += 1,
            SkipKind::Locked => self.locked += 1,
            SkipKind::Invalid => self.invalid += 1,
        }
    }

    fn breakdown(&self) -> String {
        let parts: Vec<String> = [
            (self.unsupported, SkipKind::Unsupported),
            (self.locked, SkipKind::Locked),
            (self.invalid, SkipKind::Invalid),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, kind)| format!("{count} {}", kind.label()))
        .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    }
}

/// Default output name: path segments joined with `-`, so collections of
/// different authors cannot silently overwrite each other. The unnamed
/// collection gets a `-page` suffix rather than a dangling `-`.
fn default_output(target: &Target) -> PathBuf {
    match target {
        Target::Collection(collection) if collection.scope == Scope::Page => {
            PathBuf::from(format!("{}-page.txt", collection.path.replace('/', "-")))
        }
        _ => PathBuf::from(format!("{}.txt", target.path().replace('/', "-"))),
    }
}

fn run(site: &dyn Site, command: Command, stop: &Arc<AtomicBool>) -> anyhow::Result<ExitCode> {
    validate(command.connection())?;
    let input = &command.connection().collection;
    let target = site
        .target(input)?
        .with_context(|| format!("{} does not address {input:?}", site.name()))?;

    match command {
        Command::Info(_) => {
            info(site, &target)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Fetch(args) => {
            let out_path = args
                .output
                .clone()
                .unwrap_or_else(|| default_output(&target));
            let interrupted = fetch_and_write(site, &target, &out_path, &args, stop)?;
            Ok(if interrupted {
                ExitCode::from(130)
            } else {
                ExitCode::SUCCESS
            })
        }
    }
}

/// Reject nonsensical pacing before any request (and before the client, which
/// cannot be built from a negative timeout).
fn validate(connection: &Connection) -> anyhow::Result<()> {
    ensure!(connection.delay >= 0.0, "--delay must not be negative");
    ensure!(connection.timeout > 0.0, "--timeout must be positive");
    Ok(())
}

/// Print what a target addresses.
fn info(site: &dyn Site, target: &Target) -> anyhow::Result<()> {
    match target {
        Target::Level(level) => info_level(site, level),
        Target::Collection(collection) => info_collection(site, collection),
    }
}

/// Describe one level: what it is, where it lives, and which collection holds
/// it.
fn info_level(site: &dyn Site, level: &LevelRef) -> anyhow::Result<()> {
    let spinner = ProgressBar::new_spinner();
    spinner.enable_steady_tick(Duration::from_millis(100));
    let facts = match site.fetch(level)? {
        Outcome::Level(facts, _) | Outcome::Skip(facts, _) => facts,
    };
    spinner.finish_and_clear();

    let mut line = level.label.clone();
    for note in &facts.notes {
        line.push_str(" · ");
        line.push_str(note);
    }
    line.push_str(" · by ");
    line.push_str(&facts.author);
    anstream::println!("{line}");
    anstream::println!("  {}", facts.url);
    // A site that only learns the collection from the level's own page has
    // none to name here, so the level's account of it stands alone.
    let owner = if level.collection.is_empty() {
        facts.collection_name.clone()
    } else {
        format!("{} ({})", level.collection, facts.collection_name)
    };
    anstream::println!(
        "  pack: {owner}{}",
        match &facts.published {
            Some(date) => format!(" · published {date}"),
            None => String::new(),
        },
    );
    Ok(())
}

/// Describe a collection: the levels sitting directly on its page, or its
/// groups followed by those levels.
fn info_collection(site: &dyn Site, target: &CollectionRef) -> anyhow::Result<()> {
    let spinner = ProgressBar::new_spinner();
    spinner.enable_steady_tick(Duration::from_millis(100));
    let collection = site.inspect(target, &spinner)?;
    spinner.finish_and_clear();

    if target.scope == Scope::Page {
        if collection.own.is_empty() {
            anstream::println!("{} · no levels directly on the page", collection.name);
            anstream::println!("  {}", collection.page);
            return Ok(());
        }
        anstream::println!(
            "{} · {} levels directly on the page",
            collection.name,
            collection.own.len()
        );
        anstream::println!("  {}", collection.page);
        anstream::println!();
        print_ids(&collection.own, 2);
        return Ok(());
    }

    if collection.groups.is_empty() {
        // No sub-packs to summarise, so the levels themselves are the summary —
        // however many pages the site spreads them over.
        anstream::println!(
            "{} · {} puzzles, in a single list",
            collection.name,
            collection.own.len()
        );
        anstream::println!("  {}", collection.page);
        return Ok(());
    }

    let total = collection
        .total
        .map_or_else(|| "?".to_owned(), |n| n.to_string());
    let suffix = if collection.own.is_empty() {
        String::new()
    } else {
        format!(" · {} on the page", collection.own.len())
    };
    anstream::println!(
        "{} · {} puzzles in {} groups{suffix}",
        collection.name,
        total,
        collection.groups.len()
    );
    anstream::println!("  {}", collection.page);

    let width = collection
        .groups
        .iter()
        .map(|group| group.path.chars().count())
        .chain(std::iter::once("(page)".len()))
        .max()
        .unwrap_or(0);
    anstream::println!();
    for group in &collection.groups {
        let count = group
            .count
            .map_or_else(|| "?".to_owned(), |n| n.to_string());
        anstream::println!(
            "  {:<width$}  {:>4}  {}",
            group.path,
            count,
            group.name,
            width = width
        );
    }
    if !collection.own.is_empty() {
        anstream::println!(
            "  {:<width$}  {:>4}  levels directly on the page",
            "(page)",
            collection.own.len(),
            width = width
        );
        // The ids are what `fetch` works from, so they are listed as well.
        print_ids(&collection.own, width + 10);
    }
    Ok(())
}

/// Print level ids in rows of six, indented.
fn print_ids(levels: &[LevelRef], indent: usize) {
    let pad = " ".repeat(indent);
    for chunk in levels.chunks(6) {
        let ids: Vec<&str> = chunk.iter().map(|level| level.id.as_str()).collect();
        anstream::println!("{pad}{}", ids.join("  "));
    }
}

/// Fetch one collection (or one level) and write its XSB collection file.
/// Returns `true` when the user interrupted the fetch.
fn fetch_and_write(
    site: &dyn Site,
    target: &Target,
    out_path: &Path,
    args: &FetchArgs,
    stop: &Arc<AtomicBool>,
) -> anyhow::Result<bool> {
    // Phase 1: learn what to fetch. The site decides whether that is a listing
    // (a collection) or a single detail (a level).
    let spinner = ProgressBar::new_spinner();
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}").expect("template is valid"),
    );
    spinner.set_message(match target {
        Target::Level(level) => format!("fetching {} ...", level.path),
        Target::Collection(collection) => format!("listing {} ...", collection.path),
    });
    let plan = match site.plan(target, &spinner) {
        Ok(plan) => plan,
        Err(err) => {
            spinner.finish_and_clear();
            if stop.load(Ordering::Relaxed) {
                anstream::eprintln!("interrupted");
                return Ok(true);
            }
            return Err(err);
        }
    };
    spinner.finish_and_clear();
    let collection = plan.collection;
    let mut work = plan.work;

    // Everything already in the output file is left alone when resuming, so a
    // re-run only pays for the levels that appeared since the last one.
    let present = if args.resume {
        existing_ids(site, out_path)?
    } else {
        HashSet::new()
    };
    let already = work
        .iter()
        .filter(|work| present.contains(&work.level().id))
        .count();
    work.retain(|work| !present.contains(&work.level().id));

    let work: Vec<_> = match args.limit {
        Some(limit) => work.into_iter().take(limit).collect(),
        None => work,
    };
    if work.is_empty() {
        if already > 0 {
            anstream::println!(
                "{}: nothing new in {} ({} levels already in {})",
                paint(anstyle::AnsiColor::Green, "done"),
                collection.name,
                paint(anstyle::AnsiColor::Green, already),
                out_path.display(),
            );
            return Ok(false);
        }
        bail!(
            "{}",
            match target {
                Target::Level(level) =>
                    format!("--limit 0 leaves nothing to fetch of {}", level.path),
                Target::Collection(collection) if collection.scope == Scope::Page =>
                    format!("no levels sit directly on the page of {}", collection.path),
                Target::Collection(collection) => {
                    format!("pack {} holds no puzzles", collection.path)
                }
            }
        );
    }

    // Phase 2: fetch every level and convert/filter it for the XSB format.
    let progress = ProgressBar::new(work.len() as u64);
    progress.enable_steady_tick(Duration::from_millis(100));
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] {bar:30.cyan/blue} {pos}/{len} ({percent}%) {msg}",
        )
        .expect("template is valid"),
    );

    let mut kept: Vec<(Level, String)> = Vec::new();
    let mut fetched = 0usize; // successful level fetches, reported in the summary
    let mut skipped = 0usize;
    let mut skipped_by = SkipBreakdown::default();
    let mut failed = 0usize;
    let mut interrupted = false;

    for work in work {
        if stop.load(Ordering::Relaxed) {
            interrupted = true;
            break;
        }
        let label = work.level().label.clone();
        progress.set_message(format!(
            "{} - kept {} · skipped {} · failed {}",
            label,
            kept.len(),
            skipped,
            failed,
        ));
        let outcome = match work {
            Work::Done(_, outcome) => Ok(*outcome),
            Work::Pending(level) => site.fetch(&level),
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(err) => {
                if stop.load(Ordering::Relaxed) {
                    interrupted = true;
                    break;
                }
                failed += 1;
                note(&progress, &format!("  failed {label}: {err:#}"));
                progress.inc(1);
                continue;
            }
        };
        match outcome {
            Outcome::Level(level, board) => {
                fetched += 1;
                kept.push((level, board));
            }
            Outcome::Skip(level, skip) => {
                skipped += 1;
                skipped_by.count(skip.kind);
                if args.verbose {
                    note(
                        &progress,
                        &format!("  skipped {} {}: {}", level.id, level.title, skip.reason),
                    );
                }
            }
        }
        progress.inc(1);
    }
    progress.finish_and_clear();

    if interrupted {
        anstream::eprintln!(
            "{}",
            paint(
                anstyle::AnsiColor::Yellow,
                "interrupted - writing the levels fetched so far",
            )
        );
    }

    write_output(out_path, site, &collection, &kept, args.resume)?;

    anstream::println!(
        "{}: kept {}, skipped {}{}, failed {} of {fetched} fetched{} -> {}",
        paint(anstyle::AnsiColor::Green, "done"),
        paint(anstyle::AnsiColor::Green, kept.len()),
        paint(anstyle::AnsiColor::Yellow, skipped),
        skipped_by.breakdown(),
        paint(anstyle::AnsiColor::Red, failed),
        if already > 0 {
            format!(
                ", {} already present",
                paint(anstyle::AnsiColor::Green, already)
            )
        } else {
            String::new()
        },
        out_path.display(),
    );
    Ok(interrupted)
}

/// Write the collection, appending to an existing file when resuming. The
/// header is rewritten either way, so its level count always describes the
/// file as a whole rather than the levels this run happened to fetch.
fn write_output(
    path: &Path,
    site: &dyn Site,
    collection: &Collection,
    kept: &[(Level, String)],
    append: bool,
) -> anyhow::Result<()> {
    let blocks = render_blocks(kept);
    let body = if append && path.exists() {
        let existing =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut body = strip_header(&existing).trim_end().to_owned();
        if !body.is_empty() && !blocks.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&blocks);
        body
    } else {
        blocks
    };
    let levels = body
        .lines()
        .filter(|line| line.starts_with("; URL:"))
        .count();
    let text = format!("{}{body}", render_header(site, collection, levels));
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Drop the leading `;` comment block (and the blank line after it), keeping
/// the level blocks byte for byte.
fn strip_header(content: &str) -> &str {
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if !(trimmed.is_empty() || trimmed.starts_with(';')) {
            return &content[offset..];
        }
        offset += line.len();
    }
    ""
}

/// Level ids already recorded in an output file, read back from its `; URL:`
/// lines by the site that wrote them. A missing file simply has none.
fn existing_ids(site: &dyn Site, path: &Path) -> anyhow::Result<HashSet<String>> {
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| line.strip_prefix("; URL: "))
        .filter_map(|url| site.id_from_url(url))
        .collect())
}

/// The three `;` header lines, followed by the blank line that separates them
/// from the first level block.
fn render_header(site: &dyn Site, collection: &Collection, levels: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "; {} - {} ({})",
        site.name(),
        collection.name,
        collection.page
    );
    let _ = writeln!(out, "; {levels} XSB levels");
    let _ = writeln!(out, "; boards XSB cannot express are left out\n");
    out
}

/// Render the level blocks: each is the XSB map followed by `Title:` /
/// `Author:` / `URL:` metadata lines, which soukoban parses as level metadata.
fn render_blocks(kept: &[(Level, String)]) -> String {
    let mut out = String::new();
    for (index, (level, board)) in kept.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(board);
        out.push('\n');
        let _ = writeln!(out, "Title: {}", level.title);
        let _ = writeln!(out, "Author: {}", level.author);
        if let Some(date) = &level.published {
            let _ = writeln!(out, "; Published: {date}");
        }
        let _ = writeln!(out, "; URL: {}", level.url);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sites::sokobanonline::SokobanOnline;
    use crate::test_support::{
        Action, MockServer, Response, wire_detail, wire_detail_in, wire_listing,
        wire_listing_loose, wire_pack_counted, wire_pack_item, wire_pack_item_counted,
        wire_puzzle_item,
    };
    use std::sync::atomic::AtomicUsize;

    fn site() -> SokobanOnline {
        SokobanOnline::new(
            Duration::ZERO,
            Duration::from_secs(5),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn fixture() -> (Collection, Level) {
        (
            Collection {
                name: "t1024".into(),
                page: "https://www.sokobanonline.com/play/community/t1024".into(),
                total: Some(1),
                groups: Vec::new(),
                own: Vec::new(),
            },
            Level {
                id: "1".into(),
                title: "corner01".into(),
                author: "T1024".into(),
                collection_name: "t1024".into(),
                url: "https://www.sokobanonline.com/play/community/t1024/1_corner01".into(),
                published: Some("2023-08-18".into()),
                notes: vec!["classic".into()],
            },
        )
    }

    #[test]
    fn output_blocks_carry_metadata() {
        let (collection, level) = fixture();
        let out = format!(
            "{}{}",
            render_header(&site(), &collection, 1),
            render_blocks(&[(level, "####".into())])
        );
        // map first, metadata after
        assert!(out.contains("\n####\nTitle: corner01\n"));
        assert!(out.contains("Author: T1024\n"));
        // publish date as a comment, before the URL comment
        let published = out.find("; Published: 2023-08-18\n").expect("publish date");
        let url = out.find("; URL:").expect("url comment");
        assert!(published < url);
        assert!(
            out.contains("; URL: https://www.sokobanonline.com/play/community/t1024/1_corner01\n")
        );
        // the header names the site and the collection
        assert!(
            out.contains(
                "; Sokoban Online - t1024 (https://www.sokobanonline.com/play/community/t1024)\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn emitted_block_parses_as_soukoban_level() {
        let (collection, level) = fixture();
        let out = format!(
            "{}{}",
            render_header(&site(), &collection, 1),
            render_blocks(&[(level, "#####\n#@$.#\n#####".into())])
        );
        let block = out
            .split("\n\n")
            .nth(1)
            .expect("one block after the header");
        let level: soukoban::level::Level = block.parse().expect("block parses as a Level");
        assert_eq!(
            level.metadata().get("title").map(String::as_str),
            Some("corner01")
        );
        assert_eq!(
            level.metadata().get("author").map(String::as_str),
            Some("T1024")
        );
    }

    #[test]
    fn default_output_joins_path_segments() {
        let collection = |path: &str, scope: Scope| {
            default_output(&Target::Collection(CollectionRef {
                path: path.into(),
                scope,
            }))
        };
        assert_eq!(collection("t1024", Scope::All), PathBuf::from("t1024.txt"));
        assert_eq!(
            collection("t1024/corner-set", Scope::All),
            PathBuf::from("t1024-corner-set.txt")
        );
        assert_eq!(collection("a/b/c", Scope::All), PathBuf::from("a-b-c.txt"));
        // the unnamed collection never shares a file with the whole author
        assert_eq!(
            collection("t1024", Scope::Page),
            PathBuf::from("t1024-page.txt")
        );
        // a level names itself, so it never collides with its collection's file
        assert_eq!(
            default_output(&Target::Level(
                LevelRef::new("135023", "obyzz011/remakes", "135023")
                    .with_path("obyzz011/remakes/135023_morch")
            )),
            PathBuf::from("obyzz011-remakes-135023_morch.txt")
        );
    }

    #[test]
    fn subcommands_keep_their_own_options() {
        let cli = Cli::parse_from(["sokofetch", "info", "obyzz011"]);
        match cli.command {
            Command::Info(args) => assert_eq!(args.connection.collection, "obyzz011"),
            Command::Fetch(_) => panic!("expected the info subcommand"),
        }

        let cli = Cli::parse_from([
            "sokofetch",
            "fetch",
            "a/b",
            "-o",
            "x.txt",
            "--limit",
            "5",
            "--resume",
        ]);
        match cli.command {
            Command::Fetch(args) => {
                assert_eq!(args.output.as_deref(), Some(Path::new("x.txt")));
                assert_eq!(args.limit, Some(5));
                assert!(args.resume);
            }
            Command::Info(_) => panic!("expected the fetch subcommand"),
        }

        // output options belong to `fetch` only, and a subcommand is required
        assert!(Cli::try_parse_from(["sokofetch", "info", "x", "-o", "y"]).is_err());
        assert!(Cli::try_parse_from(["sokofetch", "x"]).is_err());
        // both subcommands cover the whole collection: there is nothing to opt in
        assert!(Cli::try_parse_from(["sokofetch", "info", "x", "--ungrouped"]).is_err());
        assert!(Cli::try_parse_from(["sokofetch", "fetch", "x", "--only-ungrouped"]).is_err());
    }

    #[test]
    fn skip_breakdown_renders_only_nonzero_counts() {
        assert_eq!(SkipBreakdown::default().breakdown(), "");
        let mut breakdown = SkipBreakdown {
            unsupported: 2,
            ..SkipBreakdown::default()
        };
        assert_eq!(breakdown.breakdown(), " (2 not expressible in XSB)");
        breakdown.locked = 3;
        breakdown.invalid = 4;
        assert_eq!(
            breakdown.breakdown(),
            " (2 not expressible in XSB, 3 locked, 4 rejected by the XSB parser)"
        );
    }

    // ---- end-to-end tests against a local mock of the API ----

    /// A unique temp directory per test, for output files.
    fn scratch_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sokobanonline-test-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn connection(collection: &str) -> Connection {
        Connection {
            collection: collection.to_owned(),
            delay: 0.0,
            timeout: 5.0,
        }
    }

    /// `fetch` arguments pointed at `out`, with every option at its default.
    fn fetch_args(collection: &str, out: &Path) -> FetchArgs {
        FetchArgs {
            connection: connection(collection),
            output: Some(out.to_path_buf()),
            limit: None,
            verbose: false,
            resume: false,
        }
    }

    fn fetch_cmd(collection: &str, out: &Path) -> Command {
        Command::Fetch(fetch_args(collection, out))
    }

    fn info_cmd(collection: &str) -> Command {
        Command::Info(InfoArgs {
            connection: connection(collection),
        })
    }

    /// A mock server + a site pointed at it + a fresh stop flag.
    fn mock(
        handler: impl Fn(usize, &str) -> Action + Send + Sync + 'static,
    ) -> (Arc<MockServer>, SokobanOnline, Arc<AtomicBool>) {
        let server = MockServer::start();
        server.set_handler(handler);
        let stop = Arc::new(AtomicBool::new(false));
        let site = SokobanOnline::test_client(&server.base, Arc::clone(&stop));
        (server, site, stop)
    }

    /// The listing response for a pack holding exactly `ids` as puzzles.
    fn listing_of(ids: &[i64]) -> Action {
        Action::Respond(Response::json(
            200,
            wire_listing(
                Some(wire_pack_counted("t1024", ids.len() as u64)),
                ids.iter().map(|id| wire_puzzle_item(*id)).collect(),
                ids.len() as u64,
            ),
        ))
    }

    fn detail_action(id: i64) -> Action {
        Action::Respond(Response::json(
            200,
            wire_detail(id, "classic", "#####\n#@$.#\n#####", false),
        ))
    }

    /// Route `/puzzles/<id>` to the detail of that id, mocking the listing
    /// endpoint to hold exactly `ids`.
    fn fetch_handler(
        ids: &'static [i64],
    ) -> impl Fn(usize, &str) -> Action + Send + Sync + 'static {
        move |_call, target| match target {
            t if t.starts_with("/packs/by-path/") => listing_of(ids),
            t if t.starts_with("/puzzles/") => {
                let id: i64 = t
                    .rsplit('/')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                detail_action(id)
            }
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        }
    }

    #[test]
    fn run_fetches_details_and_writes_the_collection() {
        let dir = scratch_dir("fetch");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(fetch_handler(&[11, 12]));
        let code = run(&site, fetch_cmd("t1024", &out), &stop).unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(
            server.calls(),
            [
                "/packs/by-path/community/t1024?page=1",
                "/puzzles/11",
                "/puzzles/12"
            ]
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; Sokoban Online - t1024 ("));
        assert!(text.contains("; 2 XSB levels"));
        assert!(text.contains("Title: p11\n"));
        assert!(text.contains("Title: p12\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_defaults_the_output_path_from_the_collection() {
        let (_server, site, stop) = mock(fetch_handler(&[11]));
        let mut args = fetch_args("a/b", Path::new("unused.txt"));
        args.output = None;
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        let text = std::fs::read_to_string("a-b.txt").expect("default output file");
        assert!(text.contains("Title: p11\n"));
        let _ = std::fs::remove_file("a-b.txt");
    }

    /// One valid, one modern, one guest-locked and one unparseable puzzle.
    fn skip_fixture_handler() -> impl Fn(usize, &str) -> Action + Send + Sync + 'static {
        let details = [
            (1, "classic", "#####\n#@$.#\n#####", false),
            (2, "modern", "##", false),
            (3, "classic", "#####\n#@$.#\n#####", true),
            (4, "classic", "#####\n#$$.#\n# @ #\n#####", false),
        ];
        move |_call, target| match target {
            t if t.starts_with("/packs/by-path/") => listing_of(&[1, 2, 3, 4]),
            t if t.starts_with("/puzzles/") => {
                let id: i64 = t
                    .rsplit('/')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                match details.iter().find(|(key, ..)| *key == id) {
                    Some((id, kind, design, locked)) => Action::Respond(Response::json(
                        200,
                        wire_detail(*id, kind, design, *locked),
                    )),
                    None => Action::Respond(Response::plain(500, "unexpected id")),
                }
            }
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        }
    }

    #[test]
    fn run_skips_modern_guest_locked_and_unparseable_boards() {
        let dir = scratch_dir("skips");
        let out = dir.join("out.txt");
        let (_server, site, stop) = mock(skip_fixture_handler());
        let code = run(&site, fetch_cmd("t1024", &out), &stop).unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 1 XSB levels"));
        assert!(text.contains("Title: p1\n"));
        assert!(!text.contains("Title: p2\n"));
        assert!(!text.contains("Title: p3\n"));
        assert!(!text.contains("Title: p4\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_verbose_mode_reports_each_skip() {
        let dir = scratch_dir("verbose");
        let out = dir.join("out.txt");
        let (_server, site, stop) = mock(skip_fixture_handler());
        let mut args = fetch_args("t1024", &out);
        args.verbose = true;
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 1 XSB levels"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_reports_failed_detail_fetches_and_still_writes_the_file() {
        let dir = scratch_dir("failed");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(|_call, target| match target {
            t if t.starts_with("/packs/by-path/") => listing_of(&[1]),
            _ => Action::Respond(Response::json(
                503,
                r#"{"data":{"error":{"message":"upstream exploded"}}}"#,
            )),
        });
        let code = run(&site, fetch_cmd("t1024", &out), &stop).unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
        let detail_calls = server
            .calls()
            .iter()
            .filter(|t| t.starts_with("/puzzles/"))
            .count();
        assert_eq!(detail_calls, 4, "503 is retried across all attempts");
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 0 XSB levels"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_limit_only_fetches_the_first_n_puzzles() {
        let dir = scratch_dir("limit");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(fetch_handler(&[11, 12]));
        let mut args = fetch_args("t1024", &out);
        args.limit = Some(1);
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            ["/packs/by-path/community/t1024?page=1", "/puzzles/11"]
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("Title: p11\n"));
        assert!(!text.contains("Title: p12"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_interrupted_while_listing_exits_130() {
        let (server, site, stop) = mock(|_call, _target| listing_of(&[1]));
        stop.store(true, Ordering::Relaxed);
        let code = run(&site, fetch_cmd("t1024", Path::new("unused.txt")), &stop).unwrap();
        assert_eq!(code, ExitCode::from(130));
        assert!(server.calls().is_empty(), "no request must be made");
    }

    #[test]
    fn run_interrupted_between_details_writes_the_partial_collection() {
        let dir = scratch_dir("interrupted");
        let out = dir.join("out.txt");
        let server = MockServer::start();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        server.set_handler(move |_call, target| match target {
            t if t.starts_with("/packs/by-path/") => listing_of(&[1, 2]),
            _ => {
                // the first detail fetch succeeds but pulls the brake
                flag.store(true, Ordering::Relaxed);
                detail_action(1)
            }
        });
        // delay zero: the flag set while serving item 1 must not trip the
        // post-response sleep, so the loop-top check for item 2 is what stops
        let site = SokobanOnline::with_base(
            &server.base,
            Duration::ZERO,
            Duration::from_secs(10),
            Duration::ZERO,
            Arc::clone(&stop),
        );
        let code = run(&site, fetch_cmd("t1024", &out), &stop).unwrap();
        assert_eq!(code, ExitCode::from(130));
        let detail_calls = server
            .calls()
            .iter()
            .filter(|t| t.starts_with("/puzzles/"))
            .count();
        assert_eq!(detail_calls, 1, "the stop flag halts after item 1");
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 1 XSB levels"));
        assert!(text.contains("Title: p1\n"));
        assert!(!text.contains("Title: p2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_interrupted_by_a_failing_detail_fetch_writes_nothing_kept() {
        let dir = scratch_dir("interrupted-failed");
        let out = dir.join("out.txt");
        let server = MockServer::start();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        server.set_handler(move |_call, target| match target {
            t if t.starts_with("/packs/by-path/") => listing_of(&[1, 2]),
            _ => {
                // fail the first detail fetch and pull the brake with it
                flag.store(true, Ordering::Relaxed);
                Action::Respond(Response::plain(500, "boom"))
            }
        });
        let site = SokobanOnline::test_client(&server.base, Arc::clone(&stop));
        let code = run(&site, fetch_cmd("t1024", &out), &stop).unwrap();
        assert_eq!(code, ExitCode::from(130));
        let detail_calls = server
            .calls()
            .iter()
            .filter(|t| t.starts_with("/puzzles/"))
            .count();
        assert_eq!(detail_calls, 1, "the stop flag halts the retry loop");
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 0 XSB levels"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_propagates_fetch_errors_when_not_interrupted() {
        let dir = scratch_dir("propagate");
        let out = dir.join("out.txt");
        let (server, site, stop) =
            mock(|_call, _target| Action::Respond(Response::json(404, "{}")));
        let err = run(&site, fetch_cmd("ghost", &out), &stop).unwrap_err();
        assert!(err.to_string().contains("author/pack not found"), "{err:#}");
        assert_eq!(server.calls().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_list_mode_prints_groups_without_fetching_details() {
        let (server, site, stop) = mock(|_call, _target| {
            Action::Respond(Response::json(
                200,
                wire_listing(
                    // no `loose` bundle: nothing sits on the page
                    Some(wire_pack_counted("t1024", 6)),
                    vec![
                        wire_pack_item("community/t1024/corner-set"),
                        wire_pack_item("community/t1024/box-world"),
                    ],
                    2,
                ),
            ))
        });
        assert_eq!(
            run(&site, info_cmd("t1024"), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(server.calls().len(), 1, "no puzzle details are fetched");
    }

    #[test]
    fn run_list_mode_reports_direct_puzzle_counts() {
        let (server, site, stop) = mock(|_call, _target| listing_of(&[11]));
        assert_eq!(
            run(&site, info_cmd("t1024"), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(server.calls().len(), 1);
    }

    #[test]
    fn run_validates_delay_timeout_collection_and_empty_packs() {
        let dir = scratch_dir("validation");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(|_call, _target| listing_of(&[]));

        let mut args = fetch_args("t1024", &out);
        args.connection.delay = -1.0;
        assert!(
            run(&site, Command::Fetch(args), &stop)
                .unwrap_err()
                .to_string()
                .contains("--delay must not be negative")
        );

        let mut args = fetch_args("t1024", &out);
        args.connection.timeout = 0.0;
        assert!(
            run(&site, Command::Fetch(args), &stop)
                .unwrap_err()
                .to_string()
                .contains("--timeout must be positive")
        );

        assert!(
            run(&site, fetch_cmd("no such", &out), &stop)
                .unwrap_err()
                .to_string()
                .contains("invalid collection path")
        );

        // an empty listing surfaces as "the pack holds no puzzles"
        assert!(
            run(&site, fetch_cmd("t1024", &out), &stop)
                .unwrap_err()
                .to_string()
                .contains("holds no puzzles")
        );

        assert_eq!(
            server.calls().len(),
            1,
            "only the empty-listing case reaches the network"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- levels that sit directly on the page (docs/api/sokobanonline.md §3.2.1) ----

    /// A container page whose sub-pack holds `11`, and `99` sitting on the
    /// page itself, outside every sub-pack.
    fn page_levels_handler() -> impl Fn(usize, &str) -> Action + Send + Sync + 'static {
        |_call, target| match target {
            "/packs/by-path/community/t1024?page=1" => Action::Respond(Response::json(
                200,
                wire_listing_loose(
                    Some(wire_pack_counted("t1024", 2)),
                    vec![wire_pack_item_counted("community/t1024/corner-set", 1)],
                    1,
                    vec![wire_puzzle_item(99)],
                ),
            )),
            "/packs/by-path/community/t1024/corner-set?page=1" => Action::Respond(Response::json(
                200,
                wire_listing(
                    Some(wire_pack_counted("corner-set", 1)),
                    vec![wire_puzzle_item(11)],
                    1,
                ),
            )),
            t if t.starts_with("/puzzles/") => {
                let id: i64 = t
                    .rsplit('/')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                detail_action(id)
            }
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        }
    }

    #[test]
    fn run_fetches_the_levels_that_live_on_the_page() {
        let dir = scratch_dir("page-levels");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(page_levels_handler());
        assert_eq!(
            run(&site, fetch_cmd("t1024", &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            [
                "/packs/by-path/community/t1024?page=1",
                "/packs/by-path/community/t1024/corner-set?page=1",
                "/puzzles/11",
                "/puzzles/99"
            ]
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 2 XSB levels"));
        assert!(text.contains("Title: p11\n"));
        assert!(text.contains("Title: p99\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_a_container_without_ungrouped_levels_fetches_only_its_sub_packs() {
        let dir = scratch_dir("no-page-levels");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(|_call, target| match target {
            "/packs/by-path/community/t1024?page=1" => Action::Respond(Response::json(
                200,
                wire_listing(
                    Some(wire_pack_counted("t1024", 1)),
                    vec![wire_pack_item_counted("community/t1024/corner-set", 1)],
                    1,
                ),
            )),
            "/packs/by-path/community/t1024/corner-set?page=1" => Action::Respond(Response::json(
                200,
                wire_listing(
                    Some(wire_pack_counted("corner-set", 1)),
                    vec![wire_puzzle_item(11)],
                    1,
                ),
            )),
            t if t.starts_with("/puzzles/") => detail_action(11),
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        });
        assert_eq!(
            run(&site, fetch_cmd("t1024", &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            [
                "/packs/by-path/community/t1024?page=1",
                "/packs/by-path/community/t1024/corner-set?page=1",
                "/puzzles/11"
            ],
            "the listing alone tells the whole collection"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_unnamed_collection_fetches_only_the_levels_on_the_page() {
        let dir = scratch_dir("unnamed");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(page_levels_handler());
        assert_eq!(
            run(&site, fetch_cmd("t1024/", &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            ["/packs/by-path/community/t1024?page=1", "/puzzles/99"],
            "the sub-pack is neither listed nor fetched"
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 1 XSB levels"));
        assert!(text.contains("Title: p99\n"));
        assert!(!text.contains("Title: p11"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_a_pack_with_a_trailing_slash_is_still_that_pack() {
        let dir = scratch_dir("pack-slash");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(fetch_handler(&[11, 12]));
        assert_eq!(
            run(&site, fetch_cmd("t1024/corner-set/", &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            [
                "/packs/by-path/community/t1024/corner-set?page=1",
                "/puzzles/11",
                "/puzzles/12"
            ],
            "a pack holds no sub-packs, so the empty name adds nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn info_unnamed_collection_lists_only_the_levels_on_the_page() {
        let (server, site, stop) = mock(page_levels_handler());
        assert_eq!(
            run(&site, info_cmd("t1024/"), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            ["/packs/by-path/community/t1024?page=1"],
            "no sub-pack listing and no puzzle details"
        );
    }

    /// A file holding just level `11`, as an earlier run would have left it.
    const RESUME_FILE: &str = "; Sokoban Online - t1024 (/play/community/t1024)
; 1 XSB levels
; classic puzzles only - modern puzzles are excluded

#####
#@$.#
#####
Title: p11
Author: T1024
; URL: https://www.sokobanonline.com/play/community/t1024/11_p11
";

    #[test]
    fn run_resume_only_fetches_the_levels_missing_from_the_file() {
        let dir = scratch_dir("resume");
        let out = dir.join("out.txt");
        std::fs::write(&out, RESUME_FILE).expect("seed output file");
        let (server, site, stop) = mock(page_levels_handler());
        let mut args = fetch_args("t1024", &out);
        args.resume = true;
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server
                .calls()
                .iter()
                .filter(|t| t.starts_with("/puzzles/"))
                .count(),
            1,
            "only the missing level is fetched: {:?}",
            server.calls()
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert_eq!(
            text.matches("Title: p11\n").count(),
            1,
            "the level already in the file is not duplicated"
        );
        assert!(text.contains("Title: p99\n"));
        // the header describes the file, not the levels this run happened to add
        assert!(text.contains("; 2 XSB levels"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_resume_reports_when_the_file_is_already_complete() {
        let dir = scratch_dir("resume-complete");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(page_levels_handler());
        assert_eq!(
            run(&site, fetch_cmd("t1024", &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        let first = std::fs::read_to_string(&out).expect("output file");

        let calls_before = server.calls().len();
        let mut args = fetch_args("t1024", &out);
        args.resume = true;
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls().len() - calls_before,
            2,
            "the two listings, no detail fetch: {:?}",
            &server.calls()[calls_before..]
        );
        assert_eq!(
            std::fs::read_to_string(&out).expect("output file"),
            first,
            "a complete file is left untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_resume_without_an_existing_file_fetches_everything() {
        let dir = scratch_dir("resume-fresh");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(page_levels_handler());
        let mut args = fetch_args("t1024", &out);
        args.resume = true;
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server
                .calls()
                .iter()
                .filter(|t| t.starts_with("/puzzles/"))
                .count(),
            2
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 2 XSB levels"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- a single level, addressed by its own path ----

    const MORCH: &str = "obyzz011/remakes/135023_morch";

    /// Route `/puzzles/135023` to the detail of `Morch`, which names its pack.
    fn level_handler(kind: &'static str, design: &'static str) -> impl Fn(usize, &str) -> Action {
        move |_call, target| match target {
            "/puzzles/135023" => Action::Respond(Response::json(
                200,
                wire_detail_in(
                    135023,
                    "Morch",
                    kind,
                    design,
                    "community/obyzz011/remakes",
                    "Remakes",
                ),
            )),
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        }
    }

    #[test]
    fn run_fetches_one_level_with_a_single_request() {
        let dir = scratch_dir("one-level");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(level_handler("classic", "#####\n#@$.#\n#####"));
        assert_eq!(
            run(&site, fetch_cmd(MORCH, &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            server.calls(),
            ["/puzzles/135023"],
            "no listing is needed: the detail names the level and its pack"
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        // the header names the pack the detail reported, whose page path the
        // API omits and the tool derives from the slug
        assert!(
            text.contains(
                "; Sokoban Online - Remakes \
                 (https://www.sokobanonline.com/play/community/obyzz011/remakes)"
            ),
            "{text}"
        );
        assert!(text.contains("; 1 XSB levels"));
        assert!(text.contains("Title: Morch\n"));
        assert!(text.contains("Author: obyzz011\n"));
        assert!(text.contains(
            "; URL: https://www.sokobanonline.com/play/community/obyzz011/remakes/135023\n"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_a_missing_level_fails_instead_of_writing_an_empty_file() {
        let dir = scratch_dir("one-level-missing");
        let out = dir.join("out.txt");
        let (server, site, stop) =
            mock(|_call, _target| Action::Respond(Response::json(404, "{}")));
        let err = run(&site, fetch_cmd(MORCH, &out), &stop).unwrap_err();
        assert!(
            err.to_string()
                .contains("level not found: obyzz011/remakes/135023_morch"),
            "{err:#}"
        );
        assert_eq!(server.calls(), ["/puzzles/135023"], "404 is not retried");
        assert!(!out.exists(), "nothing is written for a level that is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_a_modern_level_is_reported_as_skipped() {
        let dir = scratch_dir("one-level-modern");
        let out = dir.join("out.txt");
        let (_server, site, stop) = mock(level_handler("modern", "____"));
        assert_eq!(
            run(&site, fetch_cmd(MORCH, &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(text.contains("; 0 XSB levels"));
        assert!(!text.contains("Title: Morch"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_resume_leaves_a_level_already_in_the_file_alone() {
        let dir = scratch_dir("one-level-resume");
        let out = dir.join("out.txt");
        let (server, site, stop) = mock(level_handler("classic", "#####\n#@$.#\n#####"));
        assert_eq!(
            run(&site, fetch_cmd(MORCH, &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        let first = std::fs::read_to_string(&out).expect("output file");

        let mut args = fetch_args(MORCH, &out);
        args.resume = true;
        assert_eq!(
            run(&site, Command::Fetch(args), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            std::fs::read_to_string(&out).expect("output file"),
            first,
            "the level is not duplicated"
        );
        assert_eq!(server.calls().len(), 2, "one request per run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn info_describes_one_level() {
        let (server, site, stop) = mock(level_handler("classic", "#####\n#@$.#\n#####"));
        assert_eq!(
            run(&site, info_cmd(MORCH), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(server.calls(), ["/puzzles/135023"]);
    }

    #[test]
    fn info_a_missing_level_names_the_path() {
        let (_server, site, stop) =
            mock(|_call, _target| Action::Respond(Response::json(404, "{}")));
        let err = run(&site, info_cmd(MORCH), &stop).unwrap_err();
        assert!(
            err.to_string()
                .contains("level not found: obyzz011/remakes/135023_morch"),
            "{err:#}"
        );
    }

    #[test]
    fn a_level_whose_detail_omits_the_pack_falls_back_to_its_path() {
        let dir = scratch_dir("one-level-no-pack");
        let out = dir.join("out.txt");
        // wire_detail carries no `pack` at all
        let (_server, site, stop) = mock(|_call, target| match target {
            "/puzzles/135023" => detail_action(135023),
            _ => Action::Respond(Response::plain(500, "unexpected target")),
        });
        assert_eq!(
            run(&site, fetch_cmd(MORCH, &out), &stop).unwrap(),
            ExitCode::SUCCESS
        );
        let text = std::fs::read_to_string(&out).expect("output file");
        assert!(
            text.contains("; Sokoban Online - remakes (https://www.sokobanonline.com/play/community/obyzz011/remakes)"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
