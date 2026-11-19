//! `difflib.SequenceMatcher` as nbdime uses it: no junk predicate and no autojunk heuristic.
//!
//! Matching blocks, opcodes and the three ratios reproduce CPython's results exactly, including
//! tie-breaking between equally long matches.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::ops::Range;

/// Multiplicative hasher for the small, trusted element types of a sequence index (the default
/// SipHash dominates the cost of comparing short strings).
#[derive(Default)]
struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }

    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

/// A block of equal elements: `a[a..a + size] == b[b..b + size]`.
///
/// The ordering is lexicographic on `(a, b, size)`, as Python sorts the equivalent tuples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Match {
    pub a: usize,
    pub b: usize,
    pub size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

impl Tag {
    pub fn as_str(self) -> &'static str {
        match self {
            Tag::Replace => "replace",
            Tag::Delete => "delete",
            Tag::Insert => "insert",
            Tag::Equal => "equal",
        }
    }
}

/// One step turning `a[a]` into `b[b]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opcode {
    pub tag: Tag,
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// `_calculate_ratio`: `2 * matches / length`, or 1.0 for two empty sequences.
fn calculate_ratio(matches: usize, length: usize) -> f64 {
    if length > 0 {
        2.0 * matches as f64 / length as f64
    } else {
        1.0
    }
}

/// `SequenceMatcher.real_quick_ratio` from the two lengths alone.
pub fn real_quick_ratio(len_a: usize, len_b: usize) -> f64 {
    calculate_ratio(len_a.min(len_b), len_a + len_b)
}

/// `SequenceMatcher.quick_ratio`: an upper bound on the ratio from element counts alone.
pub fn quick_ratio<T: Eq + Hash>(a: &[T], b: &[T]) -> f64 {
    let mut available: FastMap<&T, isize> = FastMap::default();
    for x in b {
        *available.entry(x).or_insert(0) += 1;
    }
    let mut matches = 0;
    for x in a {
        if let Some(count) = available.get_mut(x) {
            if *count > 0 {
                matches += 1;
            }
            *count -= 1;
        }
    }
    calculate_ratio(matches, a.len() + b.len())
}

/// Compares two sequences of hashable elements.
///
/// The index of `b` is built lazily, so [`SequenceMatcher::real_quick_ratio`] and
/// [`SequenceMatcher::quick_ratio`] cost nothing beyond their own work.
pub struct SequenceMatcher<'a, T> {
    a: &'a [T],
    b: &'a [T],
    b2j: OnceCell<FastMap<&'a T, Vec<usize>>>,
    matching_blocks: OnceCell<Vec<Match>>,
}

impl<'a, T: Eq + Hash> SequenceMatcher<'a, T> {
    pub fn new(a: &'a [T], b: &'a [T]) -> Self {
        SequenceMatcher {
            a,
            b,
            b2j: OnceCell::new(),
            matching_blocks: OnceCell::new(),
        }
    }

    /// Element of `b` -> ascending positions in `b`.
    fn b2j(&self) -> &FastMap<&'a T, Vec<usize>> {
        self.b2j.get_or_init(|| {
            let mut b2j: FastMap<&T, Vec<usize>> = FastMap::default();
            for (j, elt) in self.b.iter().enumerate() {
                b2j.entry(elt).or_default().push(j);
            }
            b2j
        })
    }

    /// The longest match in `a[alo..ahi]` and `b[blo..bhi]`.
    ///
    /// Among equally long matches the one starting earliest in `a` wins, then the one starting
    /// earliest in `b`. With no match at all the result is `Match { a: alo, b: blo, size: 0 }`.
    pub fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        self.longest_match(&mut Scratch::new(self.b.len()), alo, ahi, blo, bhi)
    }

    fn longest_match(
        &self,
        scratch: &mut Scratch,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> Match {
        let b2j = self.b2j();
        let mut best = Match {
            a: alo,
            b: blo,
            size: 0,
        };
        // Without junk or popular elements the best block can never be extended on either side,
        // so the extension loops of the original are omitted.
        for i in alo..ahi {
            if let Some(js) = b2j.get(&self.a[i]) {
                let start = js.partition_point(|&j| j < blo);
                for &j in js[start..].iter().take_while(|&&j| j < bhi) {
                    // `prev[j]` holds the length of the match ending at `a[i - 1]` and `b[j - 1]`.
                    let k = scratch.prev[j] + 1;
                    scratch.next[j + 1] = k;
                    scratch.next_touched.push(j + 1);
                    if k > best.size {
                        best = Match {
                            a: i + 1 - k,
                            b: j + 1 - k,
                            size: k,
                        };
                    }
                }
            }
            scratch.advance();
        }
        scratch.reset();
        best
    }

    /// Maximal runs of equal elements in increasing order, followed by the sentinel
    /// `Match { a: len(a), b: len(b), size: 0 }`. Adjacent blocks are merged.
    pub fn matching_blocks(&self) -> &[Match] {
        self.matching_blocks.get_or_init(|| {
            let (la, lb) = (self.a.len(), self.b.len());
            let mut scratch = Scratch::new(lb);
            let mut queue = vec![(0, la, 0, lb)];
            let mut found = Vec::new();
            while let Some((alo, ahi, blo, bhi)) = queue.pop() {
                let m = self.longest_match(&mut scratch, alo, ahi, blo, bhi);
                if m.size > 0 {
                    found.push(m);
                    if alo < m.a && blo < m.b {
                        queue.push((alo, m.a, blo, m.b));
                    }
                    if m.a + m.size < ahi && m.b + m.size < bhi {
                        queue.push((m.a + m.size, ahi, m.b + m.size, bhi));
                    }
                }
            }
            found.sort_unstable();

            let mut blocks: Vec<Match> = Vec::with_capacity(found.len() + 1);
            for m in found {
                match blocks.last_mut() {
                    Some(last) if last.a + last.size == m.a && last.b + last.size == m.b => {
                        last.size += m.size;
                    }
                    _ => blocks.push(m),
                }
            }
            blocks.push(Match {
                a: la,
                b: lb,
                size: 0,
            });
            blocks
        })
    }

    /// Edit script turning `a` into `b`, covering both sequences without gaps.
    pub fn opcodes(&self) -> Vec<Opcode> {
        let mut opcodes = Vec::new();
        let (mut i, mut j) = (0, 0);
        for m in self.matching_blocks() {
            let tag = match (i < m.a, j < m.b) {
                (true, true) => Some(Tag::Replace),
                (true, false) => Some(Tag::Delete),
                (false, true) => Some(Tag::Insert),
                (false, false) => None,
            };
            if let Some(tag) = tag {
                opcodes.push(Opcode {
                    tag,
                    a: i..m.a,
                    b: j..m.b,
                });
            }
            i = m.a + m.size;
            j = m.b + m.size;
            if m.size > 0 {
                opcodes.push(Opcode {
                    tag: Tag::Equal,
                    a: m.a..i,
                    b: m.b..j,
                });
            }
        }
        opcodes
    }

    /// `2 * M / T` for `M` matched elements out of `T` in total; 1.0 when both are empty.
    pub fn ratio(&self) -> f64 {
        let matches = self.matching_blocks().iter().map(|m| m.size).sum();
        calculate_ratio(matches, self.a.len() + self.b.len())
    }

    pub fn quick_ratio(&self) -> f64 {
        quick_ratio(self.a, self.b)
    }

    pub fn real_quick_ratio(&self) -> f64 {
        real_quick_ratio(self.a.len(), self.b.len())
    }
}

/// Reusable buffers for the dynamic programme of [`SequenceMatcher::longest_match`].
///
/// Position `j + 1` of a buffer holds the length of the longest match ending at `b[j]`, so that
/// `prev[j]` is the entry for `b[j - 1]`. Only touched positions are cleared between rows, which
/// keeps each row proportional to its number of matches.
struct Scratch {
    prev: Vec<usize>,
    next: Vec<usize>,
    prev_touched: Vec<usize>,
    next_touched: Vec<usize>,
}

impl Scratch {
    fn new(len_b: usize) -> Self {
        Scratch {
            prev: vec![0; len_b + 1],
            next: vec![0; len_b + 1],
            prev_touched: Vec::new(),
            next_touched: Vec::new(),
        }
    }

    /// Makes the row just computed the previous one.
    fn advance(&mut self) {
        for &t in &self.prev_touched {
            self.prev[t] = 0;
        }
        self.prev_touched.clear();
        std::mem::swap(&mut self.prev, &mut self.next);
        std::mem::swap(&mut self.prev_touched, &mut self.next_touched);
    }

    fn reset(&mut self) {
        for &t in &self.prev_touched {
            self.prev[t] = 0;
        }
        self.prev_touched.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn blocks(a: &str, b: &str) -> Vec<(usize, usize, usize)> {
        let (a, b) = (chars(a), chars(b));
        SequenceMatcher::new(&a, &b)
            .matching_blocks()
            .iter()
            .map(|m| (m.a, m.b, m.size))
            .collect()
    }

    #[test]
    fn longest_match_prefers_earliest() {
        let (a, b) = (chars(" abcd"), chars("abcd abcd"));
        let m = SequenceMatcher::new(&a, &b).find_longest_match(0, 5, 0, 9);
        assert_eq!(
            m,
            Match {
                a: 0,
                b: 4,
                size: 5
            }
        );

        let (a, b) = (chars("ab"), chars("c"));
        let m = SequenceMatcher::new(&a, &b).find_longest_match(0, 2, 0, 1);
        assert_eq!(
            m,
            Match {
                a: 0,
                b: 0,
                size: 0
            }
        );
    }

    #[test]
    fn ties_resolve_to_earliest_in_a_then_b() {
        // "ab" vs "acab": the longest block is "ab", matched against the later "ab".
        assert_eq!(blocks("ab", "acab"), [(0, 2, 2), (2, 4, 0)]);
        assert_eq!(blocks("aXa", "a"), [(0, 0, 1), (3, 1, 0)]);
    }

    #[test]
    fn matching_blocks_and_sentinel() {
        assert_eq!(blocks("abxcd", "abcd"), [(0, 0, 2), (3, 2, 2), (5, 4, 0)]);
        assert_eq!(blocks("", ""), [(0, 0, 0)]);
        assert_eq!(blocks("abc", ""), [(3, 0, 0)]);
    }

    #[test]
    fn adjacent_blocks_are_merged() {
        // The first match "b" splits the remainder into blocks that end up adjacent.
        let (a, b) = (chars("abab"), chars("abab"));
        let sm = SequenceMatcher::new(&a, &b);
        assert_eq!(
            sm.matching_blocks(),
            [
                Match {
                    a: 0,
                    b: 0,
                    size: 4
                },
                Match {
                    a: 4,
                    b: 4,
                    size: 0
                }
            ]
        );
    }

    #[test]
    fn opcodes_cover_both_sequences() {
        let (a, b) = (chars("qabxcd"), chars("abycdf"));
        let ops: Vec<_> = SequenceMatcher::new(&a, &b)
            .opcodes()
            .into_iter()
            .map(|o| (o.tag.as_str(), o.a, o.b))
            .collect();
        assert_eq!(
            ops,
            [
                ("delete", 0..1, 0..0),
                ("equal", 1..3, 0..2),
                ("replace", 3..4, 2..3),
                ("equal", 4..6, 3..5),
                ("insert", 6..6, 5..6),
            ]
        );
    }

    #[test]
    fn ratios() {
        let (a, b) = (chars("abcd"), chars("bcde"));
        let sm = SequenceMatcher::new(&a, &b);
        assert_eq!(sm.ratio(), 0.75);
        assert_eq!(sm.quick_ratio(), 0.75);
        assert_eq!(sm.real_quick_ratio(), 1.0);

        let empty: Vec<char> = Vec::new();
        let sm = SequenceMatcher::new(&empty, &empty);
        assert_eq!(
            (sm.ratio(), sm.quick_ratio(), sm.real_quick_ratio()),
            (1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn quick_ratio_counts_multiplicity() {
        let (a, b) = (chars("aaab"), chars("abbb"));
        // a: min(3, 1) = 1, b: min(1, 3) = 1.
        assert_eq!(quick_ratio(&a, &b), 2.0 * 2.0 / 8.0);
    }

    #[test]
    fn works_on_non_char_elements() {
        let a = ["x", "y", "z"];
        let b = ["y", "z", "w"];
        let sm = SequenceMatcher::new(&a, &b);
        assert_eq!(
            sm.matching_blocks()[0],
            Match {
                a: 1,
                b: 0,
                size: 2
            }
        );
    }
}
