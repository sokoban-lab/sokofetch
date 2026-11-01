//! The output format.
//!
//! Every site converts its own board encoding to XSB and hands the result to
//! [`validate`], so the constraints a loader enforces (standard alphabet,
//! exactly one player, boxes == goals) are checked in one place instead of
//! once per site.

/// Check that `board` is an XSB board a loader accepts.
pub fn validate(board: &str) -> Result<(), String> {
    // soukoban skips blank lines, but a blank row inside a level would read as
    // the level separator of the collection format.
    if board.lines().any(|line| line.trim().is_empty()) {
        return Err("board contains an empty row".into());
    }
    board
        .parse::<soukoban::map::Map>()
        .map(|_| ())
        .map_err(|err| format!("XSB parser rejected the board: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_plain_board() {
        assert!(validate("#####\n#@$.#\n#####").is_ok());
    }

    #[test]
    fn rejects_blank_rows() {
        assert!(validate("#####\n     \n#@$.\n#####").is_err());
    }

    #[test]
    fn rejects_coop_boards() {
        // two players
        assert!(validate("#####\n#@$@#\n#####").is_err());
    }

    #[test]
    fn rejects_decoy_boxes() {
        // two boxes, one goal
        assert!(validate("#####\n#$$.#\n# @ #\n#####").is_err());
    }
}
