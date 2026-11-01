# API notes

One file per level source. Each records how the site stores a board, what it takes to read
it, and what the site's own rules allow — so the fetcher never has to re-derive any of it.

| Source                               | Board on the site                     |
| ------------------------------------ | ------------------------------------- |
| [sokobanonline.md](sokobanonline.md) | `design` string in a JSON REST API    |
| [letslogic.md](letslogic.md)         | `data-level-map` digit string in HTML |

The two are not equivalent: Sokoban Online's `modern` boards carry rails, ice, doors and
portals that XSB cannot express, so 392 of 459 sampled levels are skipped; Let's Logic only
has eight tile types and converts 1104 of 1104.

Both are implemented, in `src/sites/sokobanonline/` and `src/sites/letslogic/`. What differs is
the cost and the courtesy: Sokoban Online needs one request per level and asks for a delay,
while Let's Logic embeds every board in its listing but puts the pages behind a `robots.txt`
whitelist that excludes them (see §8 of its note).

## Implementing another site

`src/sites/mod.rs` holds the `Site` trait and the site-neutral data model; `src/main.rs` never
mentions a host, a URL shape, or a board encoding. Adding a site means adding
`src/sites/<name>/` with an `impl Site`, converting its board to XSB and handing it to
`xsb::validate`, registering it in `sites::all`, and writing the note above — in that order.
