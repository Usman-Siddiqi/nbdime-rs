//! Brute-force longest common subsequence under an arbitrary compare predicate.
//!
//! The predicate need not be an equivalence relation, so the result depends on the exact
//! backtracking order of the dynamic programme; it is reproduced here.

use std::convert::Infallible;

use crate::diff_format::{Diff, SequenceDiffBuilder};
use crate::value::Value;

/// The value of a result that cannot be an error.
pub(super) fn never_fails<T>(result: Result<T, Infallible>) -> T {
    result.unwrap_or_else(|never| match never {})
}

/// `G[x][y] == compare(a[x], b[y])` as a bit matrix, one row of whole words per item of `a`.
struct CompareGrid {
    bits: Vec<u64>,
    words_per_row: usize,
}

impl CompareGrid {
    fn new<T, E>(
        a: &[T],
        b: &[T],
        compare: &impl Fn(&T, &T) -> Result<bool, E>,
    ) -> Result<Self, E> {
        let words_per_row = b.len().div_ceil(64);
        let mut bits = vec![0u64; a.len() * words_per_row];
        for (ax, row) in a.iter().zip(bits.chunks_exact_mut(words_per_row.max(1))) {
            for (y, by) in b.iter().enumerate() {
                if compare(ax, by)? {
                    row[y / 64] |= 1 << (y % 64);
                }
            }
        }
        Ok(CompareGrid {
            bits,
            words_per_row,
        })
    }

    fn row(&self, x: usize) -> &[u64] {
        &self.bits[x * self.words_per_row..(x + 1) * self.words_per_row]
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.row(x)[y / 64] >> (y % 64) & 1 == 1
    }
}

/// The table `R[x][y] = llcs(a[..x], b[..y])` in the bit-parallel form of Allison and Dix:
/// bit `y` of row `x` is clear exactly when `R[x][y + 1] == R[x][y] + 1`. The rows follow from
/// the rows of the compare grid with one addition each, 64 columns at a time, and are the
/// same for any compare predicate as the plain dynamic programme.
struct LcsTable {
    rows: Vec<u64>,
    words_per_row: usize,
}

impl LcsTable {
    fn new(grid: &CompareGrid, n: usize) -> Self {
        let words_per_row = grid.words_per_row;
        let mut rows = vec![u64::MAX; (n + 1) * words_per_row];
        for x in 1..=n {
            let (done, rest) = rows.split_at_mut(x * words_per_row);
            let prev = &done[(x - 1) * words_per_row..];
            let next = &mut rest[..words_per_row];
            let mut carry = false;
            for ((next, &v), &matches) in next.iter_mut().zip(prev).zip(grid.row(x - 1)) {
                let (sum, c1) = v.overflowing_add(v & matches);
                let (sum, c2) = sum.overflowing_add(u64::from(carry));
                carry = c1 || c2;
                *next = sum | (v & !matches);
            }
        }
        LcsTable {
            rows,
            words_per_row,
        }
    }

    /// `R[x][y]`.
    fn get(&self, x: usize, y: usize) -> u32 {
        let row = &self.rows[x * self.words_per_row..];
        let (full, rest) = (y / 64, y % 64);
        let mut ones: u32 = row[..full].iter().map(|w| w.count_ones()).sum();
        if rest > 0 {
            ones += (row[full] & ((1u64 << rest) - 1)).count_ones();
        }
        y as u32 - ones
    }

    /// `R[x][y] - R[x][y - 1]`, for `y >= 1`.
    fn step_left(&self, x: usize, y: usize) -> u32 {
        let word = self.rows[x * self.words_per_row + (y - 1) / 64];
        1 - (word >> ((y - 1) % 64) & 1) as u32
    }
}

/// Index pairs `(i, j)` of one longest common subsequence of `a` and `b`, ascending.
///
/// `compare(a[i], b[j])` holds for every returned pair.
pub fn lcs_indices<T>(a: &[T], b: &[T], compare: impl Fn(&T, &T) -> bool) -> Vec<(usize, usize)> {
    never_fails(try_lcs_indices(a, b, |x, y| Ok(compare(x, y))))
}

/// [`lcs_indices`] with a predicate that can fail; the first error is returned.
pub fn try_lcs_indices<T, E>(
    a: &[T],
    b: &[T],
    compare: impl Fn(&T, &T) -> Result<bool, E>,
) -> Result<Vec<(usize, usize)>, E> {
    // The backtrack below takes the diagonal at (N, M) whenever the last items compare equal, and
    // the table entries it then reads only depend on the prefixes, so a common suffix can be cut
    // off and appended without changing the result. A common prefix cannot be treated this way.
    let (mut n, mut m) = (a.len(), b.len());
    while n > 0 && m > 0 && compare(&a[n - 1], &b[m - 1])? {
        n -= 1;
        m -= 1;
    }
    let mut pairs = if n > 0 && m > 0 {
        lcs_indices_without_suffix(&a[..n], &b[..m], &compare)?
    } else {
        Vec::new()
    };
    pairs.extend((n..a.len()).zip(m..b.len()));
    Ok(pairs)
}

fn lcs_indices_without_suffix<T, E>(
    a: &[T],
    b: &[T],
    compare: &impl Fn(&T, &T) -> Result<bool, E>,
) -> Result<Vec<(usize, usize)>, E> {
    let grid = CompareGrid::new(a, b, compare)?;
    let (n, m) = (a.len(), b.len());
    let table = LcsTable::new(&grid, n);

    let mut length = table.get(n, m);
    let mut pairs = Vec::with_capacity(length as usize);
    let (mut x, mut y) = (n, m);
    // `length` is `R[x][y]` throughout.
    while x > 0 && y > 0 {
        if grid.get(x - 1, y - 1) {
            x -= 1;
            y -= 1;
            length -= 1;
            pairs.push((x, y));
        } else if length == table.get(x - 1, y) {
            x -= 1;
        } else {
            length -= table.step_left(x, y);
            y -= 1;
        }
    }
    pairs.reverse();
    Ok(pairs)
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

/// [`diff_sequence_bruteforce`] with a predicate that can fail.
pub fn try_diff_sequence_bruteforce<E>(
    a: &[Value],
    b: &[Value],
    compare: impl Fn(&Value, &Value) -> Result<bool, E>,
) -> Result<Diff, E> {
    Ok(diff_from_lcs(a, b, &try_lcs_indices(a, b, compare)?))
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

    /// The textbook dynamic programme with the same backtrack, on an arbitrary compare grid.
    fn reference_lcs(grid: &[Vec<bool>], m: usize) -> Vec<(usize, usize)> {
        let n = grid.len();
        let mut r = vec![vec![0u32; m + 1]; n + 1];
        for x in 1..=n {
            for y in 1..=m {
                r[x][y] = if grid[x - 1][y - 1] {
                    r[x - 1][y - 1] + 1
                } else {
                    r[x - 1][y].max(r[x][y - 1])
                };
            }
        }
        let (mut x, mut y, mut pairs) = (n, m, Vec::new());
        while x > 0 && y > 0 {
            if grid[x - 1][y - 1] {
                x -= 1;
                y -= 1;
                pairs.push((x, y));
            } else if r[x][y] == r[x - 1][y] {
                x -= 1;
            } else {
                y -= 1;
            }
        }
        pairs.reverse();
        pairs
    }

    #[test]
    fn bit_parallel_table_matches_the_dynamic_programme_on_arbitrary_relations() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        for round in 0..400u64 {
            let (n, m) = (1 + next(150) as usize, 1 + next(150) as usize);
            // From sparse to dense grids, including relations that are not equivalences.
            let density = 1 + round % 8;
            let grid: Vec<Vec<bool>> = (0..n)
                .map(|_| (0..m).map(|_| next(16) < density).collect())
                .collect();
            let compare = |x: &usize, y: &usize| Ok::<_, Infallible>(grid[*x][*y]);
            let (a, b): (Vec<usize>, Vec<usize>) = ((0..n).collect(), (0..m).collect());
            assert_eq!(
                never_fails(lcs_indices_without_suffix(&a, &b, &compare)),
                reference_lcs(&grid, m),
                "{n}x{m} at density {density}/16"
            );
        }
    }

    #[test]
    fn suffix_stripping_keeps_results() {
        fn reference(a: &[char], b: &[char]) -> Vec<(usize, usize)> {
            never_fails(lcs_indices_without_suffix(a, b, &|x, y| Ok(eq(x, y))))
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
    fn nan_inside_containers_aligns() {
        let list = |v: f64| Value::List(vec![Value::Float(v), Value::from(1)]);
        let a = [list(f64::NAN), Value::Float(f64::NAN)];
        let b = [list(f64::NAN), Value::Float(f64::NAN)];
        let d = diff_sequence_bruteforce(&a, &b, |x, y| x.py_eq(y));
        assert_eq!(
            crate::json::dumps(&diff_to_value(&d), &crate::json::DumpOptions::compact()),
            r#"[{"op":"addrange","key":1,"valuelist":[NaN]},{"op":"removerange","key":1,"length":1}]"#
        );
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
