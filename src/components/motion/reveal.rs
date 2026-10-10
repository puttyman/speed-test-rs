//! Result reveal (SPEC §7.3): when a run finishes, tiles settle in turn
//! (translate + fade, staggered 60 ms), badges flip in and chart lines draw.
//!
//! The motion itself is CSS (`style/components/_motion.scss`): an element
//! with class `reveal` animates while an ancestor has class `is-revealed`,
//! delayed by `--reveal-i × 60 ms`. Only `transform` and `opacity` animate.

/// Inline style giving an element its place in the stagger.
pub fn reveal_style(index: usize) -> String {
    format!("--reveal-i:{index}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn style() {
        assert_eq!(super::reveal_style(3), "--reveal-i:3");
    }
}
