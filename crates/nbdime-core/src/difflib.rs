//! `difflib.SequenceMatcher` as nbdime uses it: no junk predicate, and the autojunk heuristic
//! only where nbdime leaves it on (the line diffs of the built-in text renderer).
//!
//! Matching blocks, opcodes and the three ratios reproduce CPython's results exactly, including
//! tie-breaking between equally long matches.

use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

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
    let mut available: HashMap<&T, isize> = HashMap::new();
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

/// [`quick_ratio`] of two sequences that are already sorted, which needs no hashing.
pub fn quick_ratio_sorted<T: Ord>(a_sorted: &[T], b_sorted: &[T]) -> f64 {
    let (mut i, mut j, mut matches) = (0, 0, 0);
    while i < a_sorted.len() && j < b_sorted.len() {
        match a_sorted[i].cmp(&b_sorted[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                matches += 1;
                i += 1;
                j += 1;
            }
        }
    }
    calculate_ratio(matches, a_sorted.len() + b_sorted.len())
}

/// Positions of each element in a sequence: the part of a [`SequenceMatcher`] that depends on
/// `b` alone, which can be built once and shared by matchers with the same `b`.
///
/// Uses the standard randomly seeded hasher, as the elements may come from untrusted input.
pub struct PositionIndex<T>(HashMap<T, Vec<usize>>);

impl<T: Eq + Hash + Clone> PositionIndex<T> {
    pub fn new(sequence: &[T]) -> Self {
        let mut positions: HashMap<T, Vec<usize>> = HashMap::new();
        for (j, elt) in sequence.iter().enumerate() {
            positions.entry(elt.clone()).or_default().push(j);
        }
        PositionIndex(positions)
    }

    /// The index of `SequenceMatcher(autojunk=True)`: with 200 or more elements, those that make
    /// up more than 1% of the sequence (plus one) are left out.
    pub fn without_popular(sequence: &[T]) -> Self {
        let mut index = Self::new(sequence);
        let n = sequence.len();
        if n >= 200 {
            let limit = n / 100 + 1;
            index.0.retain(|_, positions| positions.len() <= limit);
        }
        index
    }

    /// Ascending positions of `elt`.
    fn get(&self, elt: &T) -> Option<&[usize]> {
        self.0.get(elt).map(Vec::as_slice)
    }
}

/// Compares two sequences of hashable elements.
///
/// The index of `b` is built lazily, so [`SequenceMatcher::real_quick_ratio`] and
/// [`SequenceMatcher::quick_ratio`] cost nothing beyond their own work.
pub struct SequenceMatcher<'a, T> {
    a: &'a [T],
    b: &'a [T],
    shared_index: Option<&'a PositionIndex<T>>,
    own_index: OnceCell<PositionIndex<T>>,
    autojunk: bool,
    matching_blocks: OnceCell<Vec<Match>>,
}

impl<'a, T: Eq + Hash + Clone> SequenceMatcher<'a, T> {
    pub fn new(a: &'a [T], b: &'a [T]) -> Self {
        SequenceMatcher {
            a,
            b,
            shared_index: None,
            own_index: OnceCell::new(),
            autojunk: false,
            matching_blocks: OnceCell::new(),
        }
    }

    /// `SequenceMatcher(None, a, b)` with Python's default `autojunk=True`.
    pub fn with_autojunk(a: &'a [T], b: &'a [T]) -> Self {
        SequenceMatcher {
            autojunk: true,
            ..Self::new(a, b)
        }
    }

    /// A matcher that uses a prebuilt index of `b`.
    pub fn with_index(a: &'a [T], b: &'a [T], index: &'a PositionIndex<T>) -> Self {
        SequenceMatcher {
            shared_index: Some(index),
            ..Self::new(a, b)
        }
    }

    fn index(&self) -> &PositionIndex<T> {
        self.shared_index.unwrap_or_else(|| {
            self.own_index.get_or_init(|| {
                if self.autojunk {
                    PositionIndex::without_popular(self.b)
                } else {
                    PositionIndex::new(self.b)
                }
            })
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
        let index = self.index();
        let mut best = Match {
            a: alo,
            b: blo,
            size: 0,
        };
        for i in alo..ahi {
            if let Some(js) = index.get(&self.a[i]) {
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
        // Elements left out of the index (popular ones) can still extend the block. Without any
        // the block is already maximal and neither loop moves.
        while best.a > alo && best.b > blo && self.a[best.a - 1] == self.b[best.b - 1] {
            best = Match {
                a: best.a - 1,
                b: best.b - 1,
                size: best.size + 1,
            };
        }
        while best.a + best.size < ahi
            && best.b + best.size < bhi
            && self.a[best.a + best.size] == self.b[best.b + best.size]
        {
            best.size += 1;
        }
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

    /// `get_grouped_opcodes(n)`: the opcodes in groups separated by runs of more than `2 * n`
    /// equal elements, with at most `n` elements of context around each change.
    pub fn grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push(Opcode {
                tag: Tag::Equal,
                a: 0..1,
                b: 0..1,
            });
        }
        if let Some(first) = codes.first_mut().filter(|c| c.tag == Tag::Equal) {
            first.a.start = first.a.start.max(first.a.end.saturating_sub(n));
            first.b.start = first.b.start.max(first.b.end.saturating_sub(n));
        }
        if let Some(last) = codes.last_mut().filter(|c| c.tag == Tag::Equal) {
            last.a.end = last.a.end.min(last.a.start + n);
            last.b.end = last.b.end.min(last.b.start + n);
        }
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for mut code in codes {
            if code.tag == Tag::Equal && code.a.len() > 2 * n {
                group.push(Opcode {
                    tag: Tag::Equal,
                    a: code.a.start..code.a.end.min(code.a.start + n),
                    b: code.b.start..code.b.end.min(code.b.start + n),
                });
                groups.push(std::mem::take(&mut group));
                code.a.start = code.a.start.max(code.a.end.saturating_sub(n));
                code.b.start = code.b.start.max(code.b.end.saturating_sub(n));
            }
            group.push(code);
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].tag == Tag::Equal) {
            groups.push(group);
        }
        groups
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

/// `difflib.unified_diff(a, b, lineterm=lineterm)` with empty file names and `n` lines of
/// context: the `---`/`+++` header and then each hunk, as one string per line.
pub fn unified_diff(a: &[&str], b: &[&str], n: usize, lineterm: &str) -> Vec<String> {
    let mut out = Vec::new();
    for group in SequenceMatcher::with_autojunk(a, b).grouped_opcodes(n) {
        if out.is_empty() {
            out.push(format!("--- {lineterm}"));
            out.push(format!("+++ {lineterm}"));
        }
        let (first, last) = (&group[0], &group[group.len() - 1]);
        out.push(format!(
            "@@ -{} +{} @@{lineterm}",
            format_range_unified(first.a.start, last.a.end),
            format_range_unified(first.b.start, last.b.end),
        ));
        for code in &group {
            if code.tag == Tag::Equal {
                out.extend(a[code.a.clone()].iter().map(|line| format!(" {line}")));
                continue;
            }
            if matches!(code.tag, Tag::Replace | Tag::Delete) {
                out.extend(a[code.a.clone()].iter().map(|line| format!("-{line}")));
            }
            if matches!(code.tag, Tag::Replace | Tag::Insert) {
                out.extend(b[code.b.clone()].iter().map(|line| format!("+{line}")));
            }
        }
    }
    out
}

/// `_format_range_unified`: `start,length` of a hunk side, with the line number alone for one
/// line and an empty range counted from the line before.
fn format_range_unified(start: usize, stop: usize) -> String {
    let length = stop - start;
    match length {
        1 => (start + 1).to_string(),
        0 => format!("{start},0"),
        _ => format!("{},{length}", start + 1),
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
    fn sorted_quick_ratio_agrees_with_hashed() {
        let sorted = |s: &str| {
            let mut v = chars(s);
            v.sort_unstable();
            v
        };
        for (x, y) in [("aaab", "abbb"), ("", ""), ("abc", ""), ("hello", "world")] {
            assert_eq!(
                quick_ratio_sorted(&sorted(x), &sorted(y)),
                quick_ratio(&chars(x), &chars(y))
            );
        }
    }

    #[test]
    fn shared_index_gives_the_same_matches() {
        let (a, b) = (chars("qabxcd"), chars("abycdf"));
        let index = PositionIndex::new(&b);
        let shared = SequenceMatcher::with_index(&a, &b, &index);
        let own = SequenceMatcher::new(&a, &b);
        assert_eq!(shared.matching_blocks(), own.matching_blocks());
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

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn unified_diff_hunks() {
        let a = lines("a\nb\nc\nd\ne\nf\ng\nh");
        let b = lines("a\nb\nX\nd\ne\nf\ng\nH\ni");
        assert_eq!(
            unified_diff(&a, &b, 3, ""),
            [
                "--- ",
                "+++ ",
                "@@ -1,8 +1,9 @@",
                " a",
                " b",
                "-c",
                "+X",
                " d",
                " e",
                " f",
                " g",
                "-h",
                "+H",
                "+i"
            ]
        );
        assert_eq!(
            unified_diff(&[], &["x"], 3, ""),
            ["--- ", "+++ ", "@@ -0,0 +1 @@", "+x"]
        );
        assert_eq!(
            unified_diff(&["x"], &[], 3, ""),
            ["--- ", "+++ ", "@@ -1 +0,0 @@", "-x"]
        );
        assert!(unified_diff(&a, &a, 3, "").is_empty());
        assert!(unified_diff(&[], &[], 3, "").is_empty());
    }

    #[test]
    fn unified_diff_splits_distant_changes_into_hunks_with_context() {
        let mut a = vec!["a"; 21];
        let mut b = a.clone();
        a[10] = "b";
        b[10] = "c";
        assert_eq!(
            unified_diff(&a, &b, 3, "\n"),
            [
                "--- \n",
                "+++ \n",
                "@@ -8,7 +8,7 @@\n",
                " a",
                " a",
                " a",
                "-b",
                "+c",
                " a",
                " a",
                " a"
            ]
        );
    }

    #[test]
    fn autojunk_leaves_popular_elements_out_of_the_index_but_extends_matches_over_them() {
        let a: Vec<&str> = ["a", "b"]
            .into_iter()
            .chain(["x"; 5])
            .chain(["c", "d"])
            .collect();
        let b: Vec<&str> = a.iter().copied().chain(["y"; 200]).collect();
        // Without autojunk the same blocks result; the popular `x` run is matched by extension.
        let matcher = SequenceMatcher::with_autojunk(&a, &b);
        assert_eq!(
            matcher.matching_blocks(),
            [
                Match {
                    a: 0,
                    b: 0,
                    size: 9
                },
                Match {
                    a: 9,
                    b: 209,
                    size: 0
                }
            ]
        );
        let a: Vec<&str> = ["z", "z", "a", "b"]
            .into_iter()
            .chain(["x"; 5])
            .chain(["c", "d", "z", "z"])
            .collect();
        let b: Vec<&str> = ["q", "a", "b"]
            .into_iter()
            .chain(["x"; 5])
            .chain(["c", "d"])
            .chain(["y"; 200])
            .collect();
        let matcher = SequenceMatcher::with_autojunk(&a, &b);
        assert_eq!(
            matcher.matching_blocks()[0],
            Match {
                a: 2,
                b: 1,
                size: 9
            }
        );
    }

    #[test]
    fn autojunk_makes_everything_popular_in_a_small_alphabet() {
        let a: Vec<char> = "fiahdacbfhdgibjdadge".chars().collect();
        let b: Vec<char> = "dcdgeafgccebfejjajfbefehfchhcaeafgaigfgjahacjdbdhfifiehbjfeagbdfijfcfeibefecbcehcabjigadjfehgcaahfdcjcgbcgfcagechjcihhfheehgcbgichfcbheiiifbfjaefieheefcjahiefeheiffefgfchfficicdfhebgcjjigejieadcjhjcdc".chars().collect();
        assert_eq!(b.len(), 200);
        assert_eq!(
            SequenceMatcher::with_autojunk(&a, &b).matching_blocks(),
            [Match {
                a: 20,
                b: 200,
                size: 0
            }]
        );
        assert!(SequenceMatcher::new(&a, &b).matching_blocks().len() > 1);
    }
}
