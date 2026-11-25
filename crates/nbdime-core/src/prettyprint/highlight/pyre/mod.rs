//! A backtracking regular expression engine with the semantics of Python's `re` on `str`
//! patterns, which Pygments' lexers are written in: Unicode `\w`, `\s`, `\d` and `\b`, `^` and
//! `$` at line boundaries with MULTILINE, lookarounds, backreferences, verbose patterns and
//! lazy quantifiers that backtrack the way `sre` does (leftmost, first alternative first).
//!
//! Positions are byte offsets into the text. [`Regex::match_at`] is `pattern.match(text, pos)`.

mod class;
mod compile;
mod exec;
mod parse;

use compile::Program;
use exec::{Backtrack, UNSET};

/// The flags of `re` that patterns can carry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub verbose: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid pattern: {0}")]
pub struct PatternError(pub String);

pub struct Regex {
    program: Program,
    #[cfg(test)]
    groups: usize,
    names: Vec<(String, usize)>,
}

/// Working memory of matches, reusable across patterns of up to any size; after a successful
/// match it holds the spans of the groups.
#[derive(Default)]
pub struct Scratch {
    slots: Vec<usize>,
    stack: Vec<Backtrack>,
    dirty: bool,
}

impl Scratch {
    /// The span of group `index` (0 is the whole match) after a successful match.
    pub fn group(&self, index: usize) -> Option<(usize, usize)> {
        let start = *self.slots.get(2 * index)?;
        let end = *self.slots.get(2 * index + 1)?;
        (start != UNSET && end != UNSET).then_some((start, end))
    }
}

impl Regex {
    pub fn new(pattern: &str, flags: Flags) -> Result<Regex, PatternError> {
        let parsed = parse::parse(pattern, flags)?;
        let program = compile::compile(&parsed.node, parsed.groups)?;
        Ok(Regex {
            program,
            #[cfg(test)]
            groups: parsed.groups,
            names: parsed.names,
        })
    }

    /// The number of capturing groups.
    #[cfg(test)]
    pub fn group_count(&self) -> usize {
        self.groups
    }

    /// The index of the group called `name`.
    pub fn group_index(&self, name: &str) -> Option<usize> {
        self.names.iter().find(|(n, _)| n == name).map(|&(_, i)| i)
    }

    /// Matches the pattern at `pos` (`pattern.match(text, pos)`), leaving the groups in
    /// `scratch`, and returns the end of the match.
    pub fn match_at(&self, text: &str, pos: usize, scratch: &mut Scratch) -> Option<usize> {
        let slots = self.program.slots;
        if scratch.dirty || scratch.slots.len() < slots {
            scratch.slots.clear();
            scratch.slots.resize(slots.max(scratch.slots.len()), UNSET);
            scratch.dirty = false;
        }
        let end = self
            .program
            .run(text, 0, pos, &mut scratch.slots, &mut scratch.stack)?;
        scratch.slots[0] = pos;
        scratch.slots[1] = end;
        scratch.dirty = true;
        Some(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> Flags {
        Flags {
            multiline: true,
            ..Flags::default()
        }
    }

    /// The groups of the match of `pattern` at `pos`, as text.
    fn groups(pattern: &str, text: &str, pos: usize) -> Option<Vec<Option<String>>> {
        groups_with(pattern, flags(), text, pos)
    }

    fn groups_with(
        pattern: &str,
        flags: Flags,
        text: &str,
        pos: usize,
    ) -> Option<Vec<Option<String>>> {
        let regex = Regex::new(pattern, flags).unwrap_or_else(|e| panic!("{pattern}: {e}"));
        let mut scratch = Scratch::default();
        regex.match_at(text, pos, &mut scratch)?;
        Some(
            (0..=regex.group_count())
                .map(|i| scratch.group(i).map(|(s, e)| text[s..e].to_owned()))
                .collect(),
        )
    }

    fn whole(pattern: &str, text: &str) -> Option<String> {
        groups(pattern, text, 0).map(|g| g[0].clone().unwrap())
    }

    #[test]
    fn alternatives_are_tried_in_order_and_backtrack() {
        assert_eq!(whole("a|ab", "ab").as_deref(), Some("a"));
        assert_eq!(whole("(?:a|ab)c", "abc").as_deref(), Some("abc"));
        assert_eq!(whole(r"(in|is|and|or|not)\b", "island"), None);
        assert_eq!(
            whole(r"(in|is|and|or|not)\b", "is x").as_deref(),
            Some("is")
        );
    }

    #[test]
    fn greedy_and_lazy_repetitions() {
        assert_eq!(whole("a*", "aaab").as_deref(), Some("aaa"));
        assert_eq!(whole("a*?b", "aaab").as_deref(), Some("aaab"));
        assert_eq!(whole("a+?", "aaa").as_deref(), Some("a"));
        assert_eq!(whole(r"a{,2}", "aaaa").as_deref(), Some("aa"));
        assert_eq!(whole(r"a{2,3}?", "aaaa").as_deref(), Some("aa"));
        assert_eq!(whole(r"[a-c]{2}x", "abx").as_deref(), Some("abx"));
        assert_eq!(whole("(?:ab)+", "ababa").as_deref(), Some("abab"));
        assert_eq!(whole(r"(?:\s|\\\s)+x", " \\ x").as_deref(), Some(" \\ x"));
        assert_eq!(
            whole(r#""""[\s\S]*?""""#, "\"\"\"a\n\"\"\"b\"\"\"").as_deref(),
            Some("\"\"\"a\n\"\"\"")
        );
    }

    #[test]
    fn a_greedy_repetition_gives_back_whole_characters() {
        assert_eq!(whole(".*é", "aéé").as_deref(), Some("aéé"));
        assert_eq!(whole(r"[^x]*é", "ééé!").as_deref(), Some("ééé"));
    }

    #[test]
    fn captures_keep_the_last_iteration_and_reset_on_backtracking() {
        let g = groups("(a)|(b)", "b", 0).unwrap();
        assert_eq!(g, [Some("b".into()), None, Some("b".into())]);
        let g = groups("(?:(a)|b)*", "ab", 0).unwrap();
        assert_eq!(g, [Some("ab".into()), Some("a".into())]);
        let g = groups("(a*)(a)", "aaa", 0).unwrap();
        assert_eq!(g[1].as_deref(), Some("aa"));
    }

    #[test]
    fn empty_loop_bodies_terminate() {
        assert_eq!(whole("(?:a?)*b", "aab").as_deref(), Some("aab"));
        assert_eq!(whole("(a*)*", "aa").as_deref(), Some("aa"));
        assert_eq!(whole("(?:)*", "x").as_deref(), Some(""));
    }

    #[test]
    fn anchors_respect_the_whole_text_and_multiline() {
        assert_eq!(
            groups("^b", "a\nb", 2).map(|g| g[0].clone()),
            Some(Some("b".into()))
        );
        assert!(groups("^b", "ab", 1).is_none());
        assert!(groups_with("^b", Flags::default(), "a\nb", 2).is_none());
        assert!(groups(r"\Ab", "ab", 1).is_none());
        assert_eq!(whole("a$", "a\nb").as_deref(), Some("a"));
        assert!(groups_with("a$", Flags::default(), "a\nb", 0).is_none());
        assert!(groups_with("a$", Flags::default(), "a\n", 0).is_some());
        assert!(groups(r"a\Z", "a\n", 0).is_none());
    }

    #[test]
    fn word_boundaries_are_unicode_aware() {
        assert_eq!(whole(r"é\b", "é.").as_deref(), Some("é"));
        assert!(groups(r"é\b", "éa", 0).is_none());
        assert!(groups(r"a\B", "ab", 0).is_some());
        assert!(groups(r"\B", "", 0).is_none());
        assert!(groups(r"\bx", "x", 0).is_some());
    }

    #[test]
    fn lookarounds() {
        assert_eq!(whole(r"a(?=b)", "ab").as_deref(), Some("a"));
        assert!(groups(r"a(?!b)", "ab", 0).is_none());
        assert!(groups(r"a(?!b)", "ac", 0).is_some());
        assert!(groups(r"(?<!\.)x", "x", 0).is_some());
        let regex = Regex::new(r"(?<!\.)abs\b", flags()).unwrap();
        let mut scratch = Scratch::default();
        assert!(regex.match_at(".abs", 1, &mut scratch).is_none());
        assert!(regex.match_at(" abs", 1, &mut scratch).is_some());
        assert!(regex.match_at("abs", 0, &mut scratch).is_some());
        let regex = Regex::new(r"(?<=ab)c", flags()).unwrap();
        assert!(regex.match_at("abc", 2, &mut scratch).is_some());
        assert!(regex.match_at("xbc", 2, &mut scratch).is_none());
        assert!(Regex::new("(?<=a|bc)x", flags()).is_err());
    }

    #[test]
    fn the_soft_keyword_lookahead() {
        let pattern = r"(^[ \t]*)(match|case)\b(?![ \t]*(?:[:,;=^&|@~)\]}]|(?:and|as)\b))";
        assert!(groups(pattern, "match x:", 0).is_some());
        assert!(groups(pattern, "match: int", 0).is_none());
        assert!(groups(pattern, "match and y", 0).is_none());
        assert!(groups(pattern, "  case _:", 0).is_some());
    }

    #[test]
    fn backreferences_and_named_groups() {
        assert_eq!(whole(r"(a+)b\1", "aabaa").as_deref(), Some("aabaa"));
        assert!(groups(r"(a+)b\1", "aaba", 1).is_some_and(|g| g[0].as_deref() == Some("aba")));
        let regex = Regex::new(r#"(?P<q>['"])x(?P=q)"#, flags()).unwrap();
        let mut scratch = Scratch::default();
        assert!(regex.match_at("'x'", 0, &mut scratch).is_some());
        assert!(regex.match_at("'x\"", 0, &mut scratch).is_none());
        assert_eq!(regex.group_index("q"), Some(1));
    }

    #[test]
    fn ignore_case_flag_and_scoped_flags() {
        let ci = Flags {
            ignore_case: true,
            ..flags()
        };
        assert!(groups_with("rb|br", ci, "BR", 0).is_some());
        assert!(groups_with("[ft]r", ci, "Fr", 0).is_some());
        assert!(groups("(?i)(r[ft]|[ft]r)(\"\"\")", "Rf\"\"\"", 0).is_some());
        assert!(groups("(?i:a)b", "Ab", 0).is_some());
        assert!(groups("(?i:a)b", "AB", 0).is_none());
        assert!(groups("(?i)[a-z]", "\u{130}", 0).is_some());
        assert!(groups("(?s:.)x", "\nx", 0).is_some());
        assert!(groups(".x", "\nx", 0).is_none());
    }

    #[test]
    fn verbose_patterns_with_named_groups_and_a_lazy_dot_all_body() {
        assert_eq!(
            whole("(?x)a\u{a0}b", "a\u{a0}b").as_deref(),
            Some("a\u{a0}b")
        );
        assert!(whole("(?x)a\u{a0}b", "ab").is_none());
        let pattern = "(?x)\n  ^(?P<initial>\\s*```)\n  (?P<lang>[\\w\\-]+)\n  (?P<afterlang>\n     (?P<whitespace>[^\\S\\n]+)\n     (?P<extra>.*))?\n  (?P<newline>\\n)\n  (?P<code>([\\s\\S])*?)\n  (?P<terminator>^\\s*```$\\n)\n  ";
        let regex = Regex::new(pattern, flags()).unwrap();
        let mut scratch = Scratch::default();
        let text = "```python x y\nprint(1)\n```\nrest";
        let end = regex.match_at(text, 0, &mut scratch).unwrap();
        assert_eq!(&text[end..], "rest");
        let group = |name: &str| {
            let (s, e) = scratch.group(regex.group_index(name).unwrap()).unwrap();
            &text[s..e]
        };
        assert_eq!(group("lang"), "python");
        assert_eq!(group("whitespace"), " ");
        assert_eq!(group("extra"), "x y");
        assert_eq!(group("code"), "print(1)\n");
    }

    #[test]
    fn long_inputs_do_not_recurse() {
        let text = "a".repeat(2_000_000);
        assert_eq!(whole(r"(?:a|b)*$", &text).map(|m| m.len()), Some(2_000_000));
        assert_eq!(whole(r"(a)*", &text).map(|m| m.len()), Some(2_000_000));
        assert_eq!(whole(r"[\s\S]*?$", &text).map(|m| m.len()), Some(2_000_000));
    }

    #[test]
    fn python_escapes() {
        assert_eq!(whole(r"\x41é\U0001f600", "Aé😀").as_deref(), Some("Aé😀"));
        assert_eq!(whole(r"\101\0", "A\0").as_deref(), Some("A\0"));
        assert_eq!(whole(r"\ \-\.", " -.").as_deref(), Some(" -."));
        assert_eq!(whole(r"[\x41-\x43]+", "ABCD").as_deref(), Some("ABC"));
        assert_eq!(whole(r"\d+", "٣4x").as_deref(), Some("٣4"));
    }

    #[test]
    fn matching_does_not_leak_captures_between_attempts() {
        let regex = Regex::new("(a)(b)?", flags()).unwrap();
        let mut scratch = Scratch::default();
        regex.match_at("ab", 0, &mut scratch).unwrap();
        assert!(scratch.group(2).is_some());
        regex.match_at("ac", 0, &mut scratch).unwrap();
        assert!(scratch.group(2).is_none());
    }
}
