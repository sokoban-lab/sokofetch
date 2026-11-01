//! Decode a Let's Logic `data-level-map` into XSB.
//!
//! The map is one digit per cell, row-major, and the alphabet comes from the
//! site's own editor rather than from guesswork (docs/api/letslogic.md §3):
//! `0` floor, `1` wall, `2` player, `3` box, `4` goal, `5` box on goal, `6`
//! player on goal, `7` outside the level. There are no mechanism tiles, so the
//! conversion loses nothing — it is a table lookup plus a crop.

use crate::xsb;

/// `0`..=`7`, in the editor's own order.
const TILES: [char; 8] = [' ', '#', '@', '$', '.', '*', '+', ' '];

/// A `width`×`height` digit map as an XSB board, or the reason it is not one.
pub fn to_xsb(map: &str, width: usize, height: usize) -> Result<String, String> {
    if width == 0 || height == 0 {
        return Err("the map declares no size".into());
    }
    let digits: Vec<char> = map.chars().collect();
    if digits.len() != width * height {
        return Err(format!(
            "the map holds {} cells, but {width}x{height} needs {}",
            digits.len(),
            width * height
        ));
    }
    let rows: Vec<Vec<char>> = digits
        .chunks(width)
        .map(|row| row.iter().copied().map(tile).collect())
        .collect::<Result<_, _>>()?;
    let board = render(&rows);
    xsb::validate(&board)?;
    Ok(board)
}

/// One digit as the character XSB uses for that cell.
fn tile(digit: char) -> Result<char, String> {
    match digit {
        '0'..='7' => Ok(TILES[digit as usize - '0' as usize]),
        other => Err(format!("unknown tile {other:?} in the map")),
    }
}

/// Join the rows, cropped to the cells that are not outside the level.
///
/// `7` marks the padding the editor keeps so nothing is drawn beyond the
/// border, which means the declared rectangle is often larger than the level.
/// Cropping to the rest keeps the output free of blank rows (the collection
/// format reads those as level separators) and of trailing whitespace.
fn render(rows: &[Vec<char>]) -> String {
    let Some((top, bottom, left, right)) = bounds(rows) else {
        return String::new();
    };
    rows[top..=bottom]
        .iter()
        .map(|row| {
            row[left..=right]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The smallest box that holds every cell which is not empty space.
fn bounds(rows: &[Vec<char>]) -> Option<(usize, usize, usize, usize)> {
    let (mut top, mut bottom, mut left, mut right) = (usize::MAX, 0, usize::MAX, 0);
    for (y, row) in rows.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            if *cell == ' ' {
                continue;
            }
            top = top.min(y);
            bottom = bottom.max(y);
            left = left.min(x);
            right = right.max(x);
        }
    }
    (top != usize::MAX).then_some((top, bottom, left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_become_the_matching_tiles() {
        // 111 / 123 / 114 -> walls around a player, a box and its goal
        assert_eq!(to_xsb("111123114", 3, 3).unwrap(), "###\n#@$\n##.");
    }

    #[test]
    fn outside_cells_are_cropped_away() {
        // a 5x3 map whose level occupies the middle three columns
        let board = to_xsb("77777", 5, 3);
        assert!(board.is_err(), "a map of only outside cells is no level");

        let map = "77777".to_owned() + "71217" + "71347";
        assert_eq!(to_xsb(&map, 5, 3).unwrap(), "#@#\n#$.");

        // ... and the padding row below the level goes with it
        let map = "7111".to_owned() + "7121" + "7134" + "7777";
        assert_eq!(to_xsb(&map, 4, 4).unwrap(), "###\n#@#\n#$.");
    }

    #[test]
    fn a_map_of_the_wrong_size_is_rejected() {
        let err = to_xsb("112", 2, 2).unwrap_err();
        assert!(err.contains("3 cells"), "{err}");
    }

    #[test]
    fn unknown_digits_are_rejected() {
        let err = to_xsb("119", 3, 1).unwrap_err();
        assert!(err.contains("unknown tile"), "{err}");
    }

    #[test]
    fn a_board_without_a_player_is_rejected_by_the_shared_check() {
        // walls and a box, but nobody to push it
        let err = to_xsb("1131", 4, 1).unwrap_err();
        assert!(err.contains("XSB parser rejected"), "{err}");
    }

    #[test]
    fn an_empty_map_has_no_size() {
        assert!(to_xsb("", 0, 0).is_err());
    }
}
