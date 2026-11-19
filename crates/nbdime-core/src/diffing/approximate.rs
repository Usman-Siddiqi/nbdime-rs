//! Approximate string comparison, used to align similar lines.

use std::cell::OnceCell;

use crate::difflib::{quick_ratio_sorted, real_quick_ratio, PositionIndex, SequenceMatcher};
use crate::py::py_len;

/// A string with the derived data that [`PreparedStr::is_similar`] needs, computed on first use
/// and kept, so that comparing one string against many does the work once per string rather
/// than once per pair.
pub(super) struct PreparedStr<'a> {
    text: &'a str,
    len: usize,
    chars: OnceCell<Vec<char>>,
    sorted_chars: OnceCell<Vec<char>>,
    positions: OnceCell<PositionIndex<char>>,
}

impl<'a> PreparedStr<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        PreparedStr {
            text,
            len: py_len(text),
            chars: OnceCell::new(),
            sorted_chars: OnceCell::new(),
            positions: OnceCell::new(),
        }
    }

    pub(super) fn text(&self) -> &'a str {
        self.text
    }

    pub(super) fn chars(&self) -> &[char] {
        self.chars.get_or_init(|| self.text.chars().collect())
    }

    fn sorted_chars(&self) -> &[char] {
        self.sorted_chars.get_or_init(|| {
            let mut sorted = self.chars().to_vec();
            sorted.sort_unstable();
            sorted
        })
    }

    fn positions(&self) -> &PositionIndex<char> {
        self.positions
            .get_or_init(|| PositionIndex::new(self.chars()))
    }

    /// [`compare_strings_approximate`] with `self` as `x`; the index of `y` is the one that
    /// gets reused, so the string that varies fastest should be `self`.
    pub(super) fn is_similar(
        &self,
        y: &PreparedStr,
        threshold: f64,
        maxlen: Option<usize>,
    ) -> bool {
        if self.text.is_empty() != y.text.is_empty() {
            return false;
        }
        if self.text == y.text {
            return true;
        }
        if real_quick_ratio(self.len, y.len) < threshold {
            return false;
        }
        if maxlen.is_some_and(|maxlen| self.len > maxlen && y.len > maxlen) {
            return false;
        }
        if quick_ratio_sorted(self.sorted_chars(), y.sorted_chars()) < threshold {
            return false;
        }
        SequenceMatcher::with_index(self.chars(), y.chars(), y.positions()).ratio() > threshold
    }
}

/// Whether two strings are similar enough to be considered the same item.
///
/// Empty versus non-empty never matches and equal strings always do. Otherwise the cheap upper
/// bounds on the difflib ratio are checked against `threshold` before the full ratio, which must
/// then be strictly greater. With `maxlen`, strings that are both longer are only accepted when
/// equal.
pub fn compare_strings_approximate(
    x: &str,
    y: &str,
    threshold: f64,
    maxlen: Option<usize>,
) -> bool {
    PreparedStr::new(x).is_similar(&PreparedStr::new(y), threshold, maxlen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approximate_comparison_rules() {
        assert!(!compare_strings_approximate("", "x", 0.7, None));
        assert!(compare_strings_approximate("", "", 0.7, None));
        assert!(compare_strings_approximate("abc", "abc", 1.0, Some(0)));
        // ratio("ab", "ac") is exactly 0.5: passes the bounds, fails the strict final test.
        assert!(!compare_strings_approximate("ab", "ac", 0.5, None));
        assert!(compare_strings_approximate("ab", "ac", 0.4, None));
        // maxlen rejects long non-identical strings even when similar.
        assert!(compare_strings_approximate(
            "abcdefgh", "abcdefgx", 0.7, None
        ));
        assert!(!compare_strings_approximate(
            "abcdefgh",
            "abcdefgx",
            0.7,
            Some(5)
        ));
        assert!(compare_strings_approximate(
            "abcdefgh",
            "abcdefgx",
            0.7,
            Some(8)
        ));
    }

    #[test]
    fn prepared_strings_are_reusable_across_comparisons() {
        let x = PreparedStr::new("abcdefgh");
        let (similar, other) = (PreparedStr::new("abcdefgx"), PreparedStr::new("zzzzzzzz"));
        for _ in 0..2 {
            assert!(x.is_similar(&similar, 0.7, None));
            assert!(!x.is_similar(&other, 0.7, None));
        }
        assert_eq!(x.text(), "abcdefgh");
        assert_eq!(similar.chars().len(), 8);
    }

    #[test]
    fn lengths_count_code_points() {
        assert!(compare_strings_approximate(
            "\u{1F600}\u{1F600}\u{1F600}",
            "\u{1F600}\u{1F600}x",
            0.5,
            None
        ));
    }
}
