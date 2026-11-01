# sokofetch

> [!WARNING]  
> Neither site invites a crawler. Sokoban Online's `robots.txt` disallows `/api/`, and Let's
> Logic's is a whitelist that excludes `/level/` and `/collection/` — the very pages this tool
> reads. Keep fetches one-off, low-volume and personal-use, and do not redistribute the levels;
> the copyright belongs to their authors.

A CLI tool for fetching levels from Sokoban websites and exporting them as an [XSB] level
collection.

## Usage

```sh
# Sokoban Online
sokofetch fetch <url>                     # Pass a page URL in place of a path.
sokofetch fetch <username>                # Fetch the whole author, sub-packs merged.
sokofetch fetch <username>/               # Fetch only the levels sitting on the page.
sokofetch fetch <username>/<pack>         # Fetch one sub-pack.
sokofetch fetch <username>/<pack>/<level> # Fetch one level, in a single request.

# Let's Logic
sokofetch fetch level/89411               # Fetch one level.
sokofetch fetch collection/632            # Fetch a collection, one request per 100 levels.
```

## AI Disclosure

This project is developed entirely with LLM assistance. Reliability is verified by unit
tests only, without human code review.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.

[XSB]: https://sokobano.de/wiki/index.php?title=Level_format
