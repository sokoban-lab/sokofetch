# sokofetch

> [!WARNING]  
> The site's `robots.txt` disallows `/api/`: the API is not meant to be crawled,
> so keep fetches one-off, low-volume, and personal-use. Do not redistribute the fetched levels;
> the copyright belongs to their authors.

A CLI tool for fetching levels from a Sokoban website and exporting them as an [XSB] level
collection.

## Usage

```sh
sokofetch fetch <url>                     # Pass a page URL in place of a path.
sokofetch fetch <username>                # Fetch the whole author, sub-packs merged.
sokofetch fetch <username>/               # Fetch only the levels sitting on the page.
sokofetch fetch <username>/<pack>         # Fetch one sub-pack.
sokofetch fetch <username>/<pack>/<level> # Fetch one level, in a single request.
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
