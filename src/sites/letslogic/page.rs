//! Read the level cards out of a Let's Logic page.
//!
//! `/level/{id}` and `/collection/{id}` render the same `div.plate`: the board
//! is in that element's `data-level-map*` attributes and the facts this tool
//! keeps — name, collection, authors, records — are in the markup around it
//! (docs/api/letslogic.md §2). There is no JSON to ask for, so this is a
//! scanner for that one shape rather than a general HTML parser: it looks for
//! the plates, treats everything up to the next plate as that card's block, and
//! pulls the few values it needs out of the block.

/// One level as a page renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub id: u64,
    /// `Level 957`: a position in the collection, not a name the author chose.
    pub name: String,
    pub collection_id: Option<u64>,
    pub collection_name: String,
    pub width: usize,
    pub height: usize,
    /// The digit map, `width × height` characters.
    pub map: String,
    /// `Werner Ettinger + xht2xsb`; empty when the page shows nobody.
    pub authors: String,
    /// The site's best known solution, as `(moves, pushes)`.
    pub record: Option<(u64, u64)>,
}

/// Every card on the page, in document order.
pub fn cards(html: &str) -> Vec<Card> {
    // `data-level-map="` and not `data-level-map`: the width, height and sha1
    // attributes all share the prefix.
    let starts: Vec<usize> = html
        .match_indices("data-level-map=\"")
        .map(|(at, _)| html[..at].rfind('<').unwrap_or(at))
        .collect();
    let mut cards = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(html.len());
        if let Some(card) = card(&html[start..end]) {
            cards.push(card);
        }
    }
    cards
}

/// One plate block, or `None` when it is missing what a level cannot do without.
fn card(block: &str) -> Option<Card> {
    let tag_end = block.find('>')?;
    let tag = &block[..tag_end];
    let id = attr(tag, "data-level-id")?.trim().parse().ok()?;
    let width = attr(tag, "data-level-map-width")?.trim().parse().ok()?;
    let height = attr(tag, "data-level-map-height")?.trim().parse().ok()?;
    let map = attr(tag, "data-level-map")?.to_owned();
    let (collection_id, collection_name) = match link(block, "/collection/") {
        Some((id, name)) => (Some(id), name),
        None => (None, String::new()),
    };
    Some(Card {
        id,
        name: name(block, id),
        collection_id,
        collection_name,
        width,
        height,
        map,
        authors: authors(block),
        record: record(block),
    })
}

/// The value of `name` in one tag, e.g. `data-level-map-width="7"`.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// The first link to `prefix` in the block, as `(id, text)`: `/collection/632`
/// pointing at `Recreation`.
fn link(block: &str, prefix: &str) -> Option<(u64, String)> {
    let needle = format!("href=\"{prefix}");
    let at = block.find(&needle)? + needle.len();
    let rest = &block[at..];
    let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let text = element_text(rest)?;
    Some((id.parse().ok()?, text))
}

/// The level's name: the anchor's `title` when it has one, else its text.
fn name(block: &str, id: u64) -> String {
    let needle = format!("id=\"level_title_{id}\"");
    let Some(at) = block.find(&needle) else {
        return String::new();
    };
    let tag_start = block[..at].rfind('<').unwrap_or(at);
    let tag_end = block[at..].find('>').map_or(block.len(), |end| at + end);
    if let Some(title) = attr(&block[tag_start..tag_end], "title")
        && !title.trim().is_empty()
    {
        return text(title);
    }
    element_text(&block[tag_end..]).unwrap_or_default()
}

/// Who made the level: the footer's `by A + B`, without the `by` or the
/// publisher that follows it.
fn authors(block: &str) -> String {
    let Some(at) = block.find("class=\"plate-footer\"") else {
        return String::new();
    };
    let after = &block[at..];
    let Some(body) = after.find('>').map(|end| &after[end + 1..]) else {
        return String::new();
    };
    // The publisher sits in the same footer, in its own div.
    let byline = body.split("published by").next().unwrap_or(body);
    let byline = text(&strip_tags(byline));
    byline
        .strip_prefix("by ")
        .unwrap_or(&byline)
        .trim()
        .to_owned()
}

/// The site's best known solution, read from the `Site Move Record` cell.
fn record(block: &str) -> Option<(u64, u64)> {
    let at = block.find("Site Move Record")?;
    let cell = &block[at..];
    let cell = &cell[..cell.find("</td>").unwrap_or(cell.len())];
    let value = &cell[cell.find("class=\"val\"")?..];
    let numbers = numbers(value);
    (numbers.len() >= 2).then(|| (numbers[0], numbers[1]))
}

/// The text of the element starting at `html`, i.e. up to its closing tag.
fn element_text(html: &str) -> Option<String> {
    let start = html.find('>')? + 1;
    let end = html[start..].find('<')? + start;
    Some(text(&html[start..end]))
}

/// Everything between the tags, as text.
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for ch in html.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            other if !inside => out.push(other),
            _ => {}
        }
    }
    out
}

/// Collapse the whitespace an HTML body is full of and resolve the entities the
/// site writes into names.
fn text(raw: &str) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    joined
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The numbers in a fragment, in order: `1,191 <span></span> 284` is
/// `[1191, 284]`.
fn numbers(fragment: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in fragment.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if ch != ',' {
            if let Ok(number) = current.parse() {
                out.push(number);
            }
            current.clear();
        }
    }
    if let Ok(number) = current.parse() {
        out.push(number);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{wire_page, wire_plate};

    /// The real page's shape: a 7x7 board, one collection, two authors.
    const ROWS: [&str; 7] = [
        "7111777", "7114111", "1110030", "4110323", "1114030", "1711004", "1771111",
    ];

    #[test]
    fn a_plate_becomes_a_card() {
        let html = wire_plate(89411, "Level 957", 632, "Recreation", &ROWS);
        let cards = cards(&html);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.id, 89411);
        assert_eq!(card.name, "Level 957");
        assert_eq!(card.collection_id, Some(632));
        assert_eq!(card.collection_name, "Recreation");
        assert_eq!((card.width, card.height), (7, 7));
        assert_eq!(card.map, ROWS.concat());
        assert_eq!(card.authors, "Werner Ettinger + xht2xsb");
        // the site's record, not "Your Move Record" (which is empty here)
        assert_eq!(card.record, Some((1191, 284)));
    }

    #[test]
    fn a_page_yields_its_cards_in_order() {
        let html = wire_page(&[
            wire_plate(1, "Level 1", 632, "Recreation", &ROWS),
            wire_plate(2, "Level 2", 632, "Recreation", &ROWS),
        ]);
        let ids: Vec<u64> = cards(&html).iter().map(|card| card.id).collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn a_page_without_plates_has_no_cards() {
        assert!(cards("<html><body>nothing here</body></html>").is_empty());
    }

    #[test]
    fn a_plate_without_a_map_is_not_a_card() {
        let html = r#"<div class="plate" data-level-id="1" data-level-map-width="7"></div>"#;
        assert!(cards(html).is_empty());
    }

    #[test]
    fn entities_in_names_are_resolved() {
        assert_eq!(text("Werner &amp; Söhne"), "Werner & Söhne");
        assert_eq!(text("  Level   957 "), "Level 957");
    }

    #[test]
    fn numbers_ignore_the_thousands_separator() {
        assert_eq!(
            numbers(r#"class="val"> 1,191 <span></span> 284</span>"#),
            [1191, 284]
        );
        assert!(numbers(r#"class="val">-</span>"#).is_empty());
    }
}
