//! Character classes with the semantics of Python's `re` on `str` patterns.

use crate::prettyprint::py_tables::{DIGIT, SPACE, WORD};

/// `\d`, `\s` and `\w`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Category {
    Digit,
    Space,
    Word,
}

/// Whether `c` lies in the inclusive ranges of `table` (flat `start, end` pairs, ascending).
pub(crate) fn in_table(table: &[u32], c: u32) -> bool {
    let (mut lo, mut hi) = (0, table.len() / 2);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if c < table[2 * mid] {
            hi = mid;
        } else if c > table[2 * mid + 1] {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

impl Category {
    pub(super) fn contains(self, c: char) -> bool {
        match self {
            Category::Digit => c.is_ascii_digit() || (!c.is_ascii() && in_table(&DIGIT, c as u32)),
            Category::Space => in_table(&SPACE, c as u32),
            Category::Word => is_word(c),
        }
    }
}

/// `\w`: Python's alphanumerics and the underscore.
pub(super) fn is_word(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || c == '_'
    } else {
        in_table(&WORD, c as u32)
    }
}

/// One member of a class: a range of code points or a category, possibly negated (`\S`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Member {
    Range(char, char),
    Category(Category, bool),
}

/// A set of characters, `[...]` or `\d` and friends.
#[derive(Clone, Debug)]
pub(super) struct CharClass {
    negated: bool,
    ignore_case: bool,
    ranges: Vec<(u32, u32)>,
    categories: Vec<(Category, bool)>,
    /// Membership of the ASCII characters, final negation included.
    ascii: u128,
}

fn simple_lower(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

fn simple_upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

impl CharClass {
    pub(super) fn new(negated: bool, ignore_case: bool, members: &[Member]) -> Self {
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        let mut categories = Vec::new();
        for member in members {
            match *member {
                Member::Range(lo, hi) => ranges.push((lo as u32, hi as u32)),
                Member::Category(category, negated) => categories.push((category, negated)),
            }
        }
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
        for (lo, hi) in ranges {
            match merged.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        let mut class = CharClass {
            negated,
            ignore_case,
            ranges: merged,
            categories,
            ascii: 0,
        };
        for code in 0..128u8 {
            if class.contains(char::from(code)) != negated {
                class.ascii |= 1 << code;
            }
        }
        class
    }

    fn contains_exactly(&self, c: char) -> bool {
        let code = c as u32;
        let (mut lo, mut hi) = (0, self.ranges.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (start, end) = self.ranges[mid];
            if code < start {
                hi = mid;
            } else if code > end {
                lo = mid + 1;
            } else {
                return true;
            }
        }
        self.categories
            .iter()
            .any(|&(category, negated)| category.contains(c) != negated)
    }

    /// Membership before the class is negated.
    fn contains(&self, c: char) -> bool {
        self.contains_exactly(c)
            || (self.ignore_case
                && (self.contains_exactly(simple_lower(c))
                    || self.contains_exactly(simple_upper(c))))
    }

    pub(super) fn matches(&self, c: char) -> bool {
        if c.is_ascii() {
            self.ascii >> (c as u32) & 1 == 1
        } else {
            self.contains(c) != self.negated
        }
    }
}

/// `c.lower()` for a character with a single-character lowercase form.
pub(super) fn fold(c: char) -> char {
    simple_lower(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_follow_python() {
        assert!(Category::Word.contains('_') && Category::Word.contains('é'));
        assert!(!Category::Word.contains('-'));
        assert!(Category::Digit.contains('٣') && !Category::Digit.contains('²'));
        assert!(Category::Space.contains('\u{a0}') && Category::Space.contains('\x1c'));
        assert!(!Category::Space.contains('\u{200b}'));
    }

    #[test]
    fn classes_merge_ranges_and_negate_categories() {
        let class = CharClass::new(
            false,
            false,
            &[
                Member::Range('a', 'c'),
                Member::Range('b', 'f'),
                Member::Category(Category::Space, true),
            ],
        );
        assert!(class.matches('e') && class.matches('x') && class.matches('é'));
        assert!(!class.matches(' ') && !class.matches('\n'));
        let negated = CharClass::new(true, false, &[Member::Range('a', 'f')]);
        assert!(negated.matches('z') && negated.matches('é') && !negated.matches('a'));
    }

    #[test]
    fn ignore_case_applies_to_ranges() {
        let class = CharClass::new(false, true, &[Member::Range('f', 't')]);
        assert!(class.matches('F') && class.matches('T') && !class.matches('E'));
    }
}
