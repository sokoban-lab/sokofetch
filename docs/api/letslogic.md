# Let's Logic — Level Data Profile

> Reverse-engineered + empirically measured data profile. Data source: `https://www.letslogic.com`  
> ⚠️ `robots.txt` is a **whitelist** here, not a blacklist — level and collection pages are
> explicitly disallowed. See §8 before writing any code against this.

## 1. TL;DR

| Question                                  | Answer                                                                     |
| ----------------------------------------- | -------------------------------------------------------------------------- |
| Does a level page contain XSB text?        | **No.** The board is a string of digits                                    |
| Where is the board?                        | `data-level-map` attribute on `div.plate`                                  |
| Encoding                                   | one char per cell, row-major, `0`–`7`, `len == width × height`             |
| Losslessly convertible to XSB?             | **Yes, 100%** — plain classic Sokoban, no mechanism tiles (§3, §4)         |
| Auth / API key needed to read a level?     | No                                                                         |
| Rendering                                  | Server-side PHP, plain HTML; no SPA, no JSON read API                      |
| Official API                               | 4 `POST` endpoints behind a member key; no XSB export (§6)                 |

The only literal `xsb` on a level page is the **author name `xht2xsb`** (the HTML→XSB
converter the levels were imported with). It is a coincidence, not data.

## 2. Page Structure

`GET /level/{id}` returns the whole page, board included. Everything about the level hangs
off one element:

```html
<div class="plate" id="divPlate89411"
     data-level-id="89411"
     data-level-map-height="7"
     data-level-map-width="7"
     data-level-map-sha1="0e490512f7a35345bc1a487b77d39deccc9cb37b"
     data-level-map="7111777114111110030411032311140301711004177111117">
```

| Attribute                | Meaning                                                       |
| ------------------------ | ------------------------------------------------------------- |
| `data-level-id`          | Level id (global, e.g. 89411)                                 |
| `data-level-map-width`   | Columns                                                       |
| `data-level-map-height`  | Rows                                                          |
| `data-level-map-sha1`    | Content hash of the map string                                |
| `data-level-map`         | **The board**, `width × height` characters, row-major         |

Also on the page, all of it *rendered*, none of it data:

- `/images/levels/0-{id}-80.png` and `0-{id}-200.png` — PNG previews of the board.
- `<a title="Level 957" id="level_title_89411">` — the name, of the form `Level {n}`.
- `Site Move Record` / `Site Push Record` — the site's best known solution, e.g. `14 / 10`.

The `/level/{id}/play` page carries **no** board data: it ships an empty `<div id="divGrid">`
and a skin (`Images[0..7]`, `/images/skins/0/{n}.png`) and rebuilds the grid from the map
string client-side. Fetch the info page, not the play page.

## 3. The Digit Encoding

`data-level-map` holds exactly `width × height` characters, read row-major. The site's own
level editor uses the identical convention, so the alphabet is recoverable from its source
rather than guessed:

```js
// /js/letslogic.js — the editor's grid, and its "Level from Text" importer
this.grid[y][x] = this.map.charAt((y * this.width) + x);
...
mapData.replace(/ /gi,'0').replace(/_/gi,'0').replace(/-/gi,'0')
       .replace(/#/gi,'1').replace(/@/gi,'2').replace(/\$/gi,'3')
       .replace(/\./gi,'4').replace(/\*/gi,'5').replace(/\+/gi,'6');
```

| Digit | `0` | `1` | `2` | `3` | `4` | `5`   | `6`     | `7`   |
| ----- | --- | --- | --- | --- | --- | ----- | ------- | ----- |
| XSB   | ` ` | `#` | `@` | `$` | `.` | `*`   | `+`     | —     |
| Means | floor | wall | player | box | goal | box on goal | player on goal | **outside** |

`7` is not a tile: the editor flood-fills it in from the border so that cells outside the
level are never drawn. It maps to nothing in XSB — drop it, or leave it as `' '`.

There are **only eight tile types** (`Images[0..7]`, one per digit). The site has no rails,
ice, holes, doors, magnets or portals — unlike Sokoban Online's `modern` format
([sokobanonline.md](sokobanonline.md) §4), nothing here has to be degraded or excluded.

## 4. Digit → XSB, and Proof That It Works

Mapping is one-to-one, so conversion is a `translate` and nothing else:

```python
DIGIT_TO_XSB = {'0': ' ', '1': '#', '2': '@', '3': '$',
                '4': '.', '5': '*', '6': '+', '7': ' '}
```

Level 89411 (`7×7`) decodes to:

```
 ###
##.####
#  $ .#
# $@$##
#. $ #
##  .#
 #####
```

Two independent checks:

1. **Optimal solution.** BFS over `(player, boxes)` on that board returns **14 moves /
   10 pushes** — exactly the page's `Site Move Record` (14 / 10).
2. **Whole sample.** All 1104 levels of collection 632 decode with **exactly one player**
   and **box count == goal count** (1104/1104, zero rejections). Frequencies are consistent
   too: boxes `3`+`5` = 49540, goals `4`+`5`+`6` = 49540, players `2`+`6` = 1104.

## 5. Bulk Access

The collection listing is the cheap path — it embeds the board of **every** level it lists:

```
GET /collection/{collection_id}?page=N&items=M
GET /collections?page=N&items=50          # 681 collections, 14 pages
```

- `items` defaults to 10 and appears to be **uncapped**: `?page=1&items=2000` returned the
  entire collection 632 — 1104 levels, 3.75 MB, one request, every map present and
  `len(map) == width × height` on all 1104.
- The list is ordered by level id ascending. `?page=111` held 4 entries (110 × 10 + 4 = 1104).
- Per-level metadata on the same card: id, name (`Level {n}`), collection, width/height,
  map sha1, `Site Move Record` / `Site Push Record`, author(s), publisher, thumbnail path.
- ⚠️ Parse `div.plate[data-level-map]`, **not** `href="/level/{id}"`: the "showcase" strip at
  the top of the page links to levels from other collections and carries no map. On
  collection 632 there are 30 level links but only 10 plates on a default page.

So a whole collection costs one request per `items`-sized page — for 632, one request total.
That is far cheaper than the per-level route, and far cheaper than Sokoban Online, which
needs one request per level.

## 6. Official API

`/api` documents four endpoints, all `POST`, all requiring a per-member `key`
(from member preferences):

```
POST https://letslogic.com/api/v1/collections             PARAMS key
POST https://letslogic.com/api/v1/collection/{id}         PARAMS key
POST https://letslogic.com/api/v1/level/{id}              PARAMS key & solution   (submit)
POST https://letslogic.com/api/v1/records                 PARAMS key
```

Without a key both read endpoints answer `API Key invalid`, so the response shape is
**unverified** — it is not documented on the page. There is no documented export of any kind,
and nothing suggests it returns XSB.

Solutions are the one thing the site speaks as text: `/level/{id}/upload_lurd` and
`/level/{id}/upload_zip` (a zip of LURD). Boards stay digits.

## 7. Data Profile (collection 632 "Recreation", 1104 levels)

- **Ids** 88461–89564, contiguous. **Names** `Level 1` … `Level 1111` — 7 numbers are gone
  (288, 326, 332, 354, 369, 377, 959), i.e. deleted levels, so the name is a position and
  not an id.
- **Authors** all 1104 are `Werner Ettinger + xht2xsb`, published by `Mark` — the collection
  is one imported archive, not a mixed author page.
- **Sizes** 6×7 … 39×39, median 17×15 (area 42–1521 cells).
- **Digit frequencies**: `0` 99859, `1` 115545, `2` 925, `3` 34614, `4` 34435, `5` 14926,
  `6` 179, `7` 36657.
- **179 levels** start with the player already standing on a goal (`6`); the other 925 use `2`.
- **XSB-convertible**: 1104/1104. No filtering stage is needed — no mechanisms exist, and
  every level satisfies the loader's hard constraints.

## 8. Compliance

```
User-agent: *
Allow: /$
Allow: /api$
Allow: /faq$
Allow: /collections$
Disallow: /
```

This is a whitelist: only the home page, `/api`, `/faq` and `/collections` are allowed.
`/level/*` and `/collection/*` — the two paths any fetcher needs — are **disallowed**, which
is materially stricter than Sokoban Online's `Disallow: /api/` (see
[sokobanonline.md](sokobanonline.md) §7). There is no crawl path the site sanctions here:
the one `/api` exception is key-gated, and none of its documented endpoints is a board export.

Level copyrights belong to their authors; do not redistribute.
