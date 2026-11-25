//! Approximate string comparison, used to align similar lines.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::difflib::{
    quick_ratio, quick_ratio_counted, real_quick_ratio, sorted_char_counts, PositionIndex,
    SequenceMatcher,
};
use crate::py::py_len;
use crate::value::{Hashable, Value};

use super::DiffError;

const THRESHOLD: f64 = 0.7;

/// Ratios remembered per string; later pairs are computed again when asked for again.
const MAX_CACHED_RATIOS: usize = 2048;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A string with the derived data that [`PreparedStr::is_similar`] needs, computed on first use
/// and kept, so that comparing one string against many does the work once per string rather
/// than once per pair. The ratio of each pair is kept as well: alignment asks about the same
/// pair under several predicates and thresholds.
pub(super) struct PreparedStr<'a> {
    id: u64,
    text: &'a str,
    len: usize,
    /// `SequenceMatcher(self, other).ratio()` by the `id` of `other`.
    ratios: RefCell<HashMap<u64, f64>>,
    chars: OnceCell<Vec<char>>,
    char_counts: OnceCell<Vec<(char, usize)>>,
    positions: OnceCell<PositionIndex<char>>,
}

impl<'a> PreparedStr<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        PreparedStr {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            text,
            len: py_len(text),
            ratios: RefCell::new(HashMap::new()),
            chars: OnceCell::new(),
            char_counts: OnceCell::new(),
            positions: OnceCell::new(),
        }
    }

    pub(super) fn text(&self) -> &'a str {
        self.text
    }

    /// Length in code points.
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(super) fn chars(&self) -> &[char] {
        self.chars.get_or_init(|| self.text.chars().collect())
    }

    fn char_counts(&self) -> &[(char, usize)] {
        self.char_counts
            .get_or_init(|| sorted_char_counts(self.chars()))
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
        if quick_ratio_counted(self.char_counts(), y.char_counts(), self.len, y.len) < threshold {
            return false;
        }
        self.ratio(y) > threshold
    }

    fn ratio(&self, y: &PreparedStr) -> f64 {
        if let Some(&ratio) = self.ratios.borrow().get(&y.id) {
            return ratio;
        }
        let ratio = SequenceMatcher::with_index(self.chars(), y.chars(), y.positions()).ratio();
        let mut ratios = self.ratios.borrow_mut();
        if ratios.len() < MAX_CACHED_RATIOS {
            ratios.insert(y.id, ratio);
        }
        ratio
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

/// The `strings_approximate` predicate as upstream applies it to two arbitrary values: the
/// operands are strings or lists (compared item by item), and anything else is a `TypeError`
/// there, so it is an error here.
pub(super) fn compare_values_approximate(
    x: &Value,
    y: &Value,
    maxlen: Option<usize>,
) -> Result<bool, DiffError> {
    // Upstream tests `bool(x) != bool(y)` first, so operands that differ in truthiness never
    // reach the type error.
    if x.truthy() != y.truthy() {
        return Ok(false);
    }
    match (x, y) {
        (Value::Str(x), Value::Str(y)) => Ok(compare_strings_approximate(x, y, THRESHOLD, maxlen)),
        (Value::List(x), Value::List(y)) => compare_lists_approximate(x, y, maxlen),
        _ => Err(DiffError::ApproximateOperands),
    }
}

fn hashable_items(items: &[Value]) -> Result<Vec<Hashable<'_>>, DiffError> {
    items
        .iter()
        .map(|item| item.as_hashable().ok_or(DiffError::UnhashableItem))
        .collect()
}

/// `compare_strings_approximate` on two non-empty lists, the items taking the place of the
/// characters. Items must be hashable, as upstream hashes them to count occurrences.
fn compare_lists_approximate(
    x: &[Value],
    y: &[Value],
    maxlen: Option<usize>,
) -> Result<bool, DiffError> {
    if x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.py_eq_item(q)) {
        return Ok(true);
    }
    if real_quick_ratio(x.len(), y.len()) < THRESHOLD {
        return Ok(false);
    }
    let (x, y) = (hashable_items(x)?, hashable_items(y)?);
    if quick_ratio(&x, &y) < THRESHOLD {
        return Ok(false);
    }
    if maxlen.is_some_and(|maxlen| x.len() > maxlen && y.len() > maxlen) {
        return Ok(false);
    }
    Ok(SequenceMatcher::new(&x, &y).ratio() > THRESHOLD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn approx(x: &str, y: &str) -> Result<bool, DiffError> {
        compare_values_approximate(&parse(x).unwrap(), &parse(y).unwrap(), None)
    }

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
    fn a_pair_ratio_is_computed_once_and_serves_every_threshold() {
        let x = PreparedStr::new("abcdefgh");
        let y = PreparedStr::new("abcdefgx");
        // ratio 14 / 16 = 0.875, asked at thresholds on both sides of it.
        for _ in 0..2 {
            assert!(x.is_similar(&y, 0.7, None));
            assert!(!x.is_similar(&y, 0.95, None));
            assert!(x.is_similar(&y, 0.875 - f64::EPSILON, None));
            assert!(!x.is_similar(&y, 0.875, None));
        }
        assert_eq!(x.ratios.borrow().get(&y.id), Some(&0.875));
        assert_eq!(x.ratios.borrow().len(), 1);
    }

    #[test]
    fn ratios_are_remembered_for_a_bounded_number_of_partners() {
        let x = PreparedStr::new("abcdefgh");
        let partners: Vec<_> = (0..MAX_CACHED_RATIOS + 5)
            .map(|_| PreparedStr::new("abcdefgx"))
            .collect();
        for y in &partners {
            assert!(x.is_similar(y, 0.7, None));
        }
        assert_eq!(x.ratios.borrow().len(), MAX_CACHED_RATIOS);
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

    #[test]
    fn strings_and_lists_compare_by_items() {
        assert_eq!(approx(r#""abcdefgh""#, r#""abcdefgx""#), Ok(true));
        assert_eq!(
            approx(r#"["a","b","c","d"]"#, r#"["a","b","c","e"]"#),
            Ok(true)
        );
        assert_eq!(
            approx(r#"["a","b","c","d"]"#, r#"["w","x","y","z"]"#),
            Ok(false)
        );
        // ratio 2 * 1 / 4 = 0.5 is below the threshold.
        assert_eq!(approx("[1, 2]", "[1, 3]"), Ok(false));
        // 1, 1.0 and true are one item, so the lists are equal.
        assert_eq!(approx("[1, 1.0, true]", "[true, 1, 1.0]"), Ok(true));
        assert_eq!(approx("[NaN, 1]", "[NaN, 1]"), Ok(true));
    }

    #[test]
    fn maxlen_applies_to_lists_after_the_cheap_bounds() {
        let (x, y) = (
            parse(r#"["a","b","c","d"]"#).unwrap(),
            parse(r#"["a","b","c","e"]"#).unwrap(),
        );
        assert_eq!(compare_values_approximate(&x, &y, Some(3)), Ok(false));
        assert_eq!(compare_values_approximate(&x, &y, Some(4)), Ok(true));
        assert_eq!(compare_values_approximate(&x, &x, Some(0)), Ok(true));
    }

    #[test]
    fn operands_of_other_types_are_an_error() {
        assert_eq!(approx("1", "2"), Err(DiffError::ApproximateOperands));
        assert_eq!(approx("[1]", r#""a""#), Err(DiffError::ApproximateOperands));
        assert_eq!(
            approx(r#"{"a": 1}"#, r#"{"a": 2}"#),
            Err(DiffError::ApproximateOperands)
        );
        assert_eq!(approx("null", "null"), Err(DiffError::ApproximateOperands));
    }

    #[test]
    fn operands_that_differ_in_truthiness_never_match() {
        assert_eq!(approx("0", "1"), Ok(false));
        assert_eq!(approx("null", "true"), Ok(false));
        assert_eq!(approx("[]", "[1]"), Ok(false));
        assert_eq!(approx(r#""""#, r#""x""#), Ok(false));
    }

    #[test]
    fn unhashable_list_items_are_an_error() {
        assert_eq!(
            approx("[[1], 2, 3, 4]", "[[1], 2, 3, 5]"),
            Err(DiffError::UnhashableItem)
        );
        // Equal lists and lists of very different lengths never get as far as hashing.
        assert_eq!(approx("[[1], 2]", "[[1], 2]"), Ok(true));
        assert_eq!(approx("[[1]]", "[[1], 2, 3, 4]"), Ok(false));
    }
}
