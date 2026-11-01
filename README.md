# sokofetch

> [!WARNING]  
> The site's `robots.txt` disallows `/api/`: the API is not meant to be crawled,
> so keep fetches one-off, low-volume, and personal-use. Do not redistribute the fetched levels;
> the copyright belongs to their authors.

A toolchain for fetching levels of an author (community pack) from [Sokoban Online]
and exporting them as an [XSB] level collection.

## Features

- **Fetch**: Downloads the pack listing and every puzzle detail, with request pacing
  and 429/5xx retry. `membercontainer` authors are fetched pack by pack.
- **XSB output**: An XSB collection with per-level metadata, validated by [soukoban].
- **Classic only**: Skips modern puzzles and boards the parser rejects
  (co-op, decoy boxes, ...).
- **Whole collection**: A collection is its sub-packs *and* the levels sitting directly
  on its page. The listing returns both in one response, so they are always included.
- **Resumable**: `--resume` keeps what an earlier run already wrote and fetches only
  the levels that appeared since.
- **Interruptible**: Ctrl+C still writes the levels fetched so far (exit code 130).

## Usage

```sh
sokofetch info  <username>              # what a collection holds, without fetching levels
sokofetch fetch <username>              # whole author (sub-packs merged) -> <username>.txt
sokofetch fetch <username>/<pack>       # one sub-pack -> <username>-<pack>.txt
sokofetch fetch <username>/             # only the levels on the page -> <username>-page.txt
sokofetch fetch <username> --resume     # only the levels missing from the existing file
sokofetch fetch <username> --limit 10   # small trial run first
sokofetch fetch <username> --delay 5    # more polite request pacing
sokofetch fetch <username> --verbose    # log every skipped level
```

A collection path is `<author>`, `<author>/<pack>`, or `<author>/` — the last names the
collection that has no name of its own, i.e. the levels sitting directly on the author's
page. A pack holds no sub-packs, so `<author>/<pack>/` is just that pack.

`--delay` and `--timeout` are accepted by both subcommands; `--output`, `--limit`,
`--verbose` and `--resume` only make sense for `fetch`, so `info` rejects them.

## Levels on the page

A Sokoban Online author page can hold sub-packs *and* its own levels at the same time. The
listing keeps the two apart: `data.items` holds the sub-packs, and `data.loose` holds the
levels that belong to no sub-pack, unpaginated, next to the first page of the listing
(docs/API.md §3.2.1). Both arrive in the same response, so those levels cost no extra request —
and unlike a sitemap they carry their names.

They are not a mode to opt into: `<author>` covers them, and `<author>/` addresses exactly
them.

```console
$ sokofetch info obyzz011
obyzz011 · 515 puzzles in 16 sub-packs · 6 on the page
  https://www.sokobanonline.com/play/community/obyzz011

  obyzz011/bridge             14  Bridge
  obyzz011/classic-9-boxes   195  Classic 9 boxes
  ...
  obyzz011/trees              10  Trees
  (page)                       6  levels directly on the page
                                  135462  136063  136072  136074  136097  136105
```

```console
$ sokofetch fetch obyzz011/
done: kept 6, skipped 0, failed 0 of 6 fetched -> obyzz011-page.txt
```

Topping up a collection fetched before some levels existed is just a re-run with
`--resume`: the levels already in the file are left alone and only the new ones are
fetched and appended. It only ever adds — levels deleted from the site stay in the file —
and levels XSB cannot represent are not recorded, so they are fetched again each run.

## AI Disclosure

This project is developed entirely with LLM assistance. Reliability is verified by unit
tests only, without human code review.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.

[Sokoban Online]: https://www.sokofetch.com
[soukoban]: https://github.com/ShenMian/soukoban
[XSB]: https://sokobano.de/wiki/index.php?title=Level_format
