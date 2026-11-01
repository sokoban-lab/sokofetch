//! Convert a sokobanonline classic `design` to XSB text.
//!
//! A classic design is already XSB (one char per cell), so the only work is
//! normalizing the `_`/`-` floor spellings to spaces. modern designs use a
//! 4-chars-per-cell encoding XSB cannot express; the caller filters them out
//! via `puzzletype`. Token tables: docs/api/sokobanonline.md §4.

use crate::xsb;

/// classic design -> XSB board, or the reason the board cannot enter the
/// collection.
pub fn to_xsb(design: &str) -> Result<String, String> {
    let board = design
        .split('\n')
        .map(|row| {
            row.trim_end_matches('\r')
                .replace(['_', '-'], " ")
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let board = board.trim_matches('\n');
    xsb::validate(board)?;
    Ok(board.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_are_normalized() {
        assert_eq!(to_xsb("#--#\n_$@.\n####").unwrap(), "#  #\n $@.\n####");
    }

    #[test]
    fn blank_edge_rows_are_trimmed() {
        assert_eq!(
            to_xsb("\n#####\n#@$.#\n#####\n").unwrap(),
            "#####\n#@$.#\n#####"
        );
    }

    #[test]
    fn blank_interior_rows_are_rejected() {
        assert!(to_xsb("#####\n     \n#@$.\n#####").is_err());
    }

    #[test]
    fn parser_rejects_coop_boards() {
        // two players
        assert!(to_xsb("#####\n#@$@#\n#####").is_err());
    }

    #[test]
    fn parser_rejects_decoy_boxes() {
        // two boxes, one goal
        assert!(to_xsb("#####\n#$$.#\n# @ #\n#####").is_err());
    }
}
