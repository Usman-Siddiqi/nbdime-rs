//! Brute-force longest common subsequence under an arbitrary compare predicate.
//!
//! The predicate need not be an equivalence relation, so the result depends on the exact
//! backtracking order of the dynamic programme; it is reproduced here.

use crate::diff_format::{Diff, SequenceDiffBuilder};
use crate::value::Value;

/// `G[x][y] == compare(a[x], b[y])` as a bit matrix.
struct CompareGrid {
    bits: Vec<u64>,
    cols: usize,
}

impl CompareGrid {
    fn new<T>(a: &[T], b: &[T], compare: &impl Fn(&T, &T) -> bool) -> Self {
        let cols = b.len();
        let mut bits = vec![0u64; (a.len() * cols).div_ceil(64)];
        for (x, ax) in a.iter().enumerate() {
            for (y, by) in b.iter().enumerate() {
                if compare(ax, by) {
                    let bit = x * cols + y;
                    bits[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
        CompareGrid { bits, cols }
    }

    fn get(&self, x: usize, y: usize) -> bool {
        let bit = x * self.cols + y;
        self.bits[bit / 64] >> (bit % 64) & 1 == 1
    }
}

/// Index pairs `(i, j)` of one longest common subsequence of `a` and `b`, ascending.
///
/// `compare(a[i], b[j])` holds for every returned pair.
pub fn lcs_indices<T>(a: &[T], b: &[T], compare: impl Fn(&T, &T) -> bool) -> Vec<(usize, usize)> {
    // The backtrack below takes the diagonal at (N, M) whenever the last items compare equal, and
    // the table entries it then reads only depend on the prefixes, so a common suffix can be cut
    // off and appended without changing the result. A common prefix cannot be treated this way.
    let (mut n, mut m) = (a.len(), b.len());
    while n > 0 && m > 0 && compare(&a[n - 1], &b[m - 1]) {
        n -= 1;
        m -= 1;
    }
    let mut pairs = if n > 0 && m > 0 {
        lcs_indices_without_suffix(&a[..n], &b[..m], &compare)
    } else {
        Vec::new()
    };
    pairs.extend((n..a.len()).zip(m..b.len()));
    pairs
}

fn lcs_indices_without_suffix<T>(
    a: &[T],
    b: &[T],
    compare: &impl Fn(&T, &T) -> bool,
) -> Vec<(usize, usize)> {
    let grid = CompareGrid::new(a, b, compare);
    let (n, m) = (a.len(), b.len());
    let width = m + 1;

    // r[x * width + y] == llcs(a[..x], b[..y])
    let mut r = vec![0u32; (n + 1) * width];
    for x in 1..=n {
        for y in 1..=m {
            r[x * width + y] = if grid.get(x - 1, y - 1) {
                r[(x - 1) * width + y - 1] + 1
            } else {
                r[(x - 1) * width + y].max(r[x * width + y - 1])
            };
        }
    }

    let mut pairs = Vec::with_capacity(r[n * width + m] as usize);
    let (mut x, mut y) = (n, m);
    while x > 0 && y > 0 {
        if grid.get(x - 1, y - 1) {
            x -= 1;
            y -= 1;
            pairs.push((x, y));
        } else if r[x * width + y] == r[(x - 1) * width + y] {
            x -= 1;
        } else {
            y -= 1;
        }
    }
    pairs.reverse();
    pairs
}

/// The diff turning `a` into `b` given the index pairs of their common subsequence.
pub fn diff_from_lcs(a: &[Value], b: &[Value], pairs: &[(usize, usize)]) -> Diff {
    let mut di = SequenceDiffBuilder::new();
    // Items consumed from `a` and `b` so far.
    let (mut x, mut y) = (0, 0);
    for &(i, j) in pairs {
        if i > x {
            di.removerange(x, i - x);
        }
        if j > y {
            di.addrange(x, Value::List(b[y..j].to_vec()));
        }
        x = i + 1;
        y = j + 1;
    }
    if x < a.len() {
        di.removerange(x, a.len() - x);
    }
    if y < b.len() {
        di.addrange(x, Value::List(b[y..].to_vec()));
    }
    di.validated()
}

/// Shallow diff of two lists by brute-force LCS under `compare`.
pub fn diff_sequence_bruteforce(
    a: &[Value],
    b: &[Value],
    compare: impl Fn(&Value, &Value) -> bool,
) -> Diff {
    diff_from_lcs(a, b, &lcs_indices(a, b, compare))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::{diff_to_value, DiffEntry};

    fn eq(a: &char, b: &char) -> bool {
        a == b
    }

    fn lcs(a: &str, b: &str) -> Vec<(usize, usize)> {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        lcs_indices(&a, &b, eq)
    }

    #[test]
    fn backtrack_prefers_decrementing_a() {
        // Both (0, 1) and (1, 0) are LCS pairs; stepping x first picks b's later "a".
        assert_eq!(lcs("a", "aa"), [(0, 1)]);
        assert_eq!(lcs("aa", "a"), [(1, 0)]);
        assert_eq!(lcs("abab", "ba"), [(1, 0), (2, 1)]);
        assert_eq!(lcs("abcabba", "cbabac"), [(1, 1), (3, 2), (5, 3), (6, 4)]);
    }

    #[test]
    fn suffix_stripping_keeps_results() {
        fn reference(a: &[char], b: &[char]) -> Vec<(usize, usize)> {
            lcs_indices_without_suffix(a, b, &eq)
        }
        let strings = ["", "a", "ab", "aab", "abab", "baab", "abcabba", "cbabac"];
        for s in strings {
            for t in strings {
                let (a, b): (Vec<char>, Vec<char>) = (s.chars().collect(), t.chars().collect());
                let expected = if a.is_empty() || b.is_empty() {
                    Vec::new()
                } else {
                    reference(&a, &b)
                };
                assert_eq!(lcs_indices(&a, &b, eq), expected, "{s:?} vs {t:?}");
            }
        }
    }

    #[test]
    fn python_equality_aligns_numbers() {
        let a = [Value::from(1), Value::Bool(true), Value::Float(2.0)];
        let b = [Value::Float(1.0), Value::from(2)];
        let d = diff_sequence_bruteforce(&a, &b, |x, y| x.py_eq(y));
        assert_eq!(d, [DiffEntry::removerange(0, 1)]);
    }

    #[test]
    fn diff_orders_addrange_before_removerange() {
        let a = [Value::from("x"), Value::from("y")];
        let b = [Value::from("z")];
        let d = diff_sequence_bruteforce(&a, &b, |p, q| p.py_eq(q));
        assert_eq!(
            diff_to_value(&d),
            crate::json::parse(
                r#"[{"op":"addrange","key":0,"valuelist":["z"]},{"op":"removerange","key":0,"length":2}]"#
            )
            .unwrap()
        );
    }

    #[test]
    fn empty_inputs() {
        let a = [Value::from(1)];
        assert_eq!(
            diff_sequence_bruteforce(&a, &[], |x, y| x.py_eq(y)),
            [DiffEntry::removerange(0, 1)]
        );
        assert_eq!(
            diff_sequence_bruteforce(&[], &a, |x, y| x.py_eq(y)),
            [DiffEntry::addrange(0, Value::List(a.to_vec()))]
        );
        assert!(diff_sequence_bruteforce(&[], &[], |x, y| x.py_eq(y)).is_empty());
    }
}
