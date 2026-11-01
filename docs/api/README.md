# API notes

One file per level source. Each records how the site stores a board, what it takes to read
it, and what the site's own rules allow — so the fetcher never has to re-derive any of it.

| Source                                         | Board on the site                        | XSB            | Crawl policy                    |
| ---------------------------------------------- | ---------------------------------------- | -------------- | ------------------------------- |
| [sokobanonline.md](sokobanonline.md)           | `design` string in a JSON REST API       | classic only   | `Disallow: /api/`, delay 5 s    |
| [letslogic.md](letslogic.md)                   | `data-level-map` digit string in HTML    | all levels     | whitelist, level pages disallowed |

The two are not equivalent: Sokoban Online's `modern` boards carry rails, ice, doors and
portals that XSB cannot express, so 392 of 459 sampled levels are skipped; Let's Logic only
has eight tile types and converts 1104 of 1104.

Only Sokoban Online is implemented (`src/api.rs`). The Let's Logic notes are research, and
its `robots.txt` currently forbids the pages a fetcher would need.
