//! Parsing of patterns in the syntax of Python's `re` module (the subset Pygments' lexers use:
//! no conditionals and no atomic groups).

use super::class::{fold, Category, CharClass, Member};
use super::{Flags, PatternError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Assertion {
    /// `^` with MULTILINE.
    LineStart,
    /// `$` with MULTILINE.
    LineEnd,
    /// `\A`, and `^` without MULTILINE.
    TextStart,
    /// `\Z`.
    TextEnd,
    /// `$` without MULTILINE: the end, or before a final newline.
    TextEndOrFinalNewline,
    WordBoundary,
    NotWordBoundary,
}

#[derive(Debug)]
pub(super) enum Node {
    Empty,
    Char {
        c: char,
        ignore_case: bool,
    },
    Any {
        dot_all: bool,
    },
    Class(CharClass),
    Assert(Assertion),
    /// A capturing group when `index` is set.
    Group {
        index: Option<usize>,
        node: Box<Node>,
    },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
    Look {
        ahead: bool,
        negative: bool,
        node: Box<Node>,
    },
    Backref(usize),
}

pub(super) struct Parsed {
    pub node: Node,
    pub groups: usize,
    pub names: Vec<(String, usize)>,
}

pub(super) fn parse(pattern: &str, flags: Flags) -> Result<Parsed> {
    let mut parser = Parser {
        chars: pattern.chars().collect(),
        pos: 0,
        flags,
        groups: 0,
        closed: Vec::new(),
        names: Vec::new(),
    };
    parser.global_flags()?;
    let node = parser.alternation()?;
    if parser.pos < parser.chars.len() {
        return Err(parser.error("unbalanced parenthesis"));
    }
    Ok(Parsed {
        node,
        groups: parser.groups,
        names: parser.names,
    })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    flags: Flags,
    groups: usize,
    /// Groups whose closing parenthesis has been read (the only ones a backreference may name).
    closed: Vec<usize>,
    names: Vec<(String, usize)>,
}

type Result<T> = std::result::Result<T, PatternError>;

impl Parser {
    fn error(&self, message: &str) -> PatternError {
        PatternError(format!("{message} at position {}", self.pos))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        self.pos += c.is_some() as usize;
        c
    }

    fn starts_with(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(i, c)| self.peek_at(i) == Some(c))
    }

    /// `(?flags)` at the very start applies to the whole pattern.
    fn global_flags(&mut self) -> Result<()> {
        if !self.starts_with("(?") {
            return Ok(());
        }
        let save = self.pos;
        self.pos += 2;
        if self.peek().is_some_and(|c| "aiLmsux".contains(c)) {
            let mut flags = self.flags;
            self.flag_letters(&mut flags, true)?;
            if self.eat(')') {
                self.flags = flags;
                return self.global_flags();
            }
        }
        self.pos = save;
        Ok(())
    }

    /// Reads flag letters, `a i L m s u x`, into `flags`; `-` starts the letters to clear.
    fn flag_letters(&mut self, flags: &mut Flags, allow_clear: bool) -> Result<()> {
        let mut value = true;
        while let Some(c) = self.peek() {
            let slot = match c {
                'i' => &mut flags.ignore_case,
                'm' => &mut flags.multiline,
                's' => &mut flags.dot_all,
                'x' => &mut flags.verbose,
                'a' | 'u' | 'L' => {
                    self.pos += 1;
                    continue;
                }
                '-' if allow_clear && value => {
                    value = false;
                    self.pos += 1;
                    continue;
                }
                _ => break,
            };
            *slot = value;
            self.pos += 1;
        }
        Ok(())
    }

    fn skip_verbose(&mut self) {
        if !self.flags.verbose {
            return;
        }
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else if c == '#' {
                while let Some(c) = self.next() {
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn alternation(&mut self) -> Result<Node> {
        let mut alternatives = vec![self.concatenation()?];
        while self.eat('|') {
            alternatives.push(self.concatenation()?);
        }
        Ok(if alternatives.len() == 1 {
            alternatives.pop().expect("one alternative")
        } else {
            Node::Alt(alternatives)
        })
    }

    fn concatenation(&mut self) -> Result<Node> {
        let mut items = Vec::new();
        loop {
            self.skip_verbose();
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => items.push(self.term()?),
            }
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().expect("one item"),
            _ => Node::Concat(items),
        })
    }

    fn term(&mut self) -> Result<Node> {
        let mut node = self.atom()?;
        loop {
            self.skip_verbose();
            let Some((min, max)) = self.quantifier()? else {
                return Ok(node);
            };
            if matches!(node, Node::Assert(_)) {
                return Err(self.error("nothing to repeat"));
            }
            if matches!(node, Node::Repeat { .. }) {
                return Err(self.error("multiple repeat"));
            }
            let greedy = !self.eat('?');
            if self.peek() == Some('+') {
                return Err(self.error("possessive quantifiers are not supported"));
            }
            node = Node::Repeat {
                node: Box::new(node),
                min,
                max,
                greedy,
            };
        }
    }

    /// `*`, `+`, `?` or `{m,n}` at the current position, consumed. A brace that does not start
    /// a valid quantifier is an ordinary character.
    fn quantifier(&mut self) -> Result<Option<(u32, Option<u32>)>> {
        match self.peek() {
            Some('*') => {
                self.pos += 1;
                Ok(Some((0, None)))
            }
            Some('+') => {
                self.pos += 1;
                Ok(Some((1, None)))
            }
            Some('?') => {
                self.pos += 1;
                Ok(Some((0, Some(1))))
            }
            Some('{') => {
                let save = self.pos;
                self.pos += 1;
                let min = self.number();
                let (min, max) = if self.eat(',') {
                    (min.unwrap_or(0), self.number())
                } else if let Some(min) = min {
                    (min, Some(min))
                } else {
                    self.pos = save;
                    return Ok(None);
                };
                if !self.eat('}') {
                    self.pos = save;
                    return Ok(None);
                }
                if max.is_some_and(|max| max < min) {
                    return Err(self.error("min repeat greater than max repeat"));
                }
                Ok(Some((min, max)))
            }
            _ => Ok(None),
        }
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse().ok()
    }

    fn atom(&mut self) -> Result<Node> {
        let flags = self.flags;
        let c = self.next().expect("an atom follows");
        Ok(match c {
            '(' => return self.group(),
            '[' => return self.class(),
            '.' => Node::Any {
                dot_all: flags.dot_all,
            },
            '^' => Node::Assert(if flags.multiline {
                Assertion::LineStart
            } else {
                Assertion::TextStart
            }),
            '$' => Node::Assert(if flags.multiline {
                Assertion::LineEnd
            } else {
                Assertion::TextEndOrFinalNewline
            }),
            '\\' => return self.escape(),
            '*' | '+' | '?' => return Err(self.error("nothing to repeat")),
            '{' => {
                self.pos -= 1;
                if self.quantifier()?.is_some() {
                    return Err(self.error("nothing to repeat"));
                }
                self.pos += 1;
                self.literal('{')
            }
            c => self.literal(c),
        })
    }

    fn literal(&self, c: char) -> Node {
        Node::Char {
            c: if self.flags.ignore_case { fold(c) } else { c },
            ignore_case: self.flags.ignore_case,
        }
    }

    fn group(&mut self) -> Result<Node> {
        let outer = self.flags;
        let node = if self.eat('?') {
            match self.next() {
                Some(':') => self.body(None)?,
                Some('P') => match self.next() {
                    Some('<') => {
                        let name = self.name('>')?;
                        if self.names.iter().any(|(n, _)| *n == name) {
                            return Err(self.error("redefinition of group name"));
                        }
                        self.groups += 1;
                        let index = self.groups;
                        self.names.push((name, index));
                        self.body(Some(index))?
                    }
                    Some('=') => {
                        let name = self.name(')')?;
                        let index = self
                            .names
                            .iter()
                            .find(|(n, _)| *n == name)
                            .map(|&(_, i)| i)
                            .ok_or_else(|| self.error("unknown group name"))?;
                        return self.backref(index);
                    }
                    _ => return Err(self.error("unknown extension ?P")),
                },
                Some('=') => return self.lookaround(true, false),
                Some('!') => return self.lookaround(true, true),
                Some('<') => match self.next() {
                    Some('=') => return self.lookaround(false, false),
                    Some('!') => return self.lookaround(false, true),
                    _ => return Err(self.error("unknown extension ?<")),
                },
                Some('#') => {
                    while let Some(c) = self.next() {
                        if c == ')' {
                            return Ok(Node::Empty);
                        }
                    }
                    return Err(self.error("missing ), unterminated comment"));
                }
                Some(c) if "aiLmsux-".contains(c) => {
                    self.pos -= 1;
                    let mut flags = self.flags;
                    self.flag_letters(&mut flags, true)?;
                    if self.peek() == Some(')') {
                        return Err(self.error("global flags not at the start of the expression"));
                    }
                    if !self.eat(':') {
                        return Err(self.error("unknown flag"));
                    }
                    self.flags = flags;
                    self.body(None)?
                }
                _ => return Err(self.error("unknown extension")),
            }
        } else {
            self.groups += 1;
            let index = self.groups;
            self.body(Some(index))?
        };
        self.flags = outer;
        Ok(node)
    }

    /// The alternation of a group up to its closing parenthesis.
    fn body(&mut self, index: Option<usize>) -> Result<Node> {
        let node = self.alternation()?;
        if !self.eat(')') {
            return Err(self.error("missing ), unterminated subpattern"));
        }
        if let Some(index) = index {
            self.closed.push(index);
        }
        Ok(Node::Group {
            index,
            node: Box::new(node),
        })
    }

    fn lookaround(&mut self, ahead: bool, negative: bool) -> Result<Node> {
        let outer = self.flags;
        let node = self.alternation()?;
        if !self.eat(')') {
            return Err(self.error("missing ), unterminated subpattern"));
        }
        self.flags = outer;
        Ok(Node::Look {
            ahead,
            negative,
            node: Box::new(node),
        })
    }

    fn name(&mut self, end: char) -> Result<String> {
        let mut name = String::new();
        loop {
            match self.next() {
                Some(c) if c == end => break,
                Some(c) if c.is_alphanumeric() || c == '_' => name.push(c),
                _ => return Err(self.error("bad character in group name")),
            }
        }
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(self.error("bad group name"));
        }
        Ok(name)
    }

    fn backref(&mut self, index: usize) -> Result<Node> {
        if !self.closed.contains(&index) {
            return Err(self.error("cannot refer to an open group"));
        }
        Ok(Node::Backref(index))
    }

    fn hex(&mut self, digits: usize) -> Result<char> {
        let mut value = 0u32;
        for _ in 0..digits {
            let digit = self
                .next()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.error("incomplete escape"))?;
            value = value * 16 + digit;
        }
        char::from_u32(value).ok_or_else(|| self.error("bad escape"))
    }

    fn octal(&mut self, first: char, max_more: usize) -> Result<char> {
        let mut value = first.to_digit(8).expect("an octal digit");
        for _ in 0..max_more {
            match self.peek().and_then(|c| c.to_digit(8)) {
                Some(d) => {
                    value = value * 8 + d;
                    self.pos += 1;
                }
                None => break,
            }
        }
        if value > 0o377 {
            return Err(self.error("octal escape value outside of range 0-0o377"));
        }
        Ok(char::from_u32(value).expect("below 256"))
    }

    /// The control characters and code points that escapes name the same way in and out of
    /// classes.
    fn common_escape(&mut self, c: char) -> Result<Option<char>> {
        Ok(Some(match c {
            'a' => '\x07',
            'f' => '\x0c',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'v' => '\x0b',
            'x' => self.hex(2)?,
            'u' => self.hex(4)?,
            'U' => self.hex(8)?,
            _ => return Ok(None),
        }))
    }

    fn escape(&mut self) -> Result<Node> {
        let c = self
            .next()
            .ok_or_else(|| self.error("bad escape (end of pattern)"))?;
        if let Some(c) = self.common_escape(c)? {
            return Ok(self.literal(c));
        }
        let class = |category, negated| {
            Node::Class(CharClass::new(
                false,
                self.flags.ignore_case,
                &[Member::Category(category, negated)],
            ))
        };
        Ok(match c {
            'A' => Node::Assert(Assertion::TextStart),
            'Z' => Node::Assert(Assertion::TextEnd),
            'b' => Node::Assert(Assertion::WordBoundary),
            'B' => Node::Assert(Assertion::NotWordBoundary),
            'd' => class(Category::Digit, false),
            'D' => class(Category::Digit, true),
            's' => class(Category::Space, false),
            'S' => class(Category::Space, true),
            'w' => class(Category::Word, false),
            'W' => class(Category::Word, true),
            '0' => {
                let c = self.octal('0', 2)?;
                self.literal(c)
            }
            '1'..='9' => {
                if let (Some(d2), Some(d3)) = (self.peek(), self.peek_at(1)) {
                    if c.is_digit(8) && d2.is_digit(8) && d3.is_digit(8) {
                        let c = self.octal(c, 2)?;
                        return Ok(self.literal(c));
                    }
                }
                let mut index = c.to_digit(10).expect("a digit") as usize;
                if let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                    index = index * 10 + d as usize;
                    self.pos += 1;
                }
                return self.backref(index);
            }
            c if c.is_ascii_alphabetic() => return Err(self.error("bad escape")),
            c => self.literal(c),
        })
    }

    fn class(&mut self) -> Result<Node> {
        let negated = self.eat('^');
        let mut members = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.next() else {
                return Err(self.error("unterminated character set"));
            };
            if c == ']' && !first {
                break;
            }
            first = false;
            let low = self.class_atom(c)?;
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c != ']') {
                self.pos += 1;
                let c = self.next().expect("a range end");
                let high = self.class_atom(c)?;
                match (low, high) {
                    (ClassAtom::Char(lo), ClassAtom::Char(hi)) if lo <= hi => {
                        members.push(Member::Range(lo, hi));
                    }
                    _ => return Err(self.error("bad character range")),
                }
            } else {
                members.push(match low {
                    ClassAtom::Char(c) => Member::Range(c, c),
                    ClassAtom::Category(category, negated) => Member::Category(category, negated),
                });
            }
        }
        Ok(Node::Class(CharClass::new(
            negated,
            self.flags.ignore_case,
            &members,
        )))
    }

    fn class_atom(&mut self, c: char) -> Result<ClassAtom> {
        if c != '\\' {
            return Ok(ClassAtom::Char(c));
        }
        let c = self
            .next()
            .ok_or_else(|| self.error("bad escape (end of pattern)"))?;
        if let Some(c) = self.common_escape(c)? {
            return Ok(ClassAtom::Char(c));
        }
        Ok(match c {
            'b' => ClassAtom::Char('\x08'),
            'd' => ClassAtom::Category(Category::Digit, false),
            'D' => ClassAtom::Category(Category::Digit, true),
            's' => ClassAtom::Category(Category::Space, false),
            'S' => ClassAtom::Category(Category::Space, true),
            'w' => ClassAtom::Category(Category::Word, false),
            'W' => ClassAtom::Category(Category::Word, true),
            '0'..='7' => ClassAtom::Char(self.octal(c, 2)?),
            c if c.is_ascii_alphanumeric() => return Err(self.error("bad escape")),
            c => ClassAtom::Char(c),
        })
    }
}

enum ClassAtom {
    Char(char),
    Category(Category, bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> Flags {
        Flags::default()
    }

    fn parse_ok(pattern: &str) -> Parsed {
        parse(pattern, flags()).unwrap_or_else(|e| panic!("{pattern:?}: {e}"))
    }

    #[test]
    fn groups_are_numbered_by_their_opening_parenthesis() {
        let parsed = parse_ok(r"(a)((?:b)(?P<n>c))");
        assert_eq!(parsed.groups, 3);
        assert_eq!(parsed.names, [("n".to_owned(), 3)]);
    }

    #[test]
    fn braces_are_literal_unless_they_make_a_quantifier() {
        assert!(matches!(
            parse_ok("a{,2}").node,
            Node::Repeat {
                min: 0,
                max: Some(2),
                ..
            }
        ));
        assert!(matches!(
            parse_ok("a{2,}").node,
            Node::Repeat {
                min: 2,
                max: None,
                ..
            }
        ));
        assert!(matches!(parse_ok("a{x}").node, Node::Concat(_)));
        assert!(matches!(parse_ok("{").node, Node::Char { c: '{', .. }));
        assert!(parse("{2}", flags()).is_err());
    }

    #[test]
    fn classes_take_a_leading_bracket_and_escapes() {
        parse_ok("[]{}:(),;[]");
        parse_ok(r"[^]]+");
        parse_ok(r"[\w\-/:]");
        parse_ok(r"[^\S\n]");
        assert!(parse(r"[a-\d]", flags()).is_err());
        assert!(parse(r"[z-a]", flags()).is_err());
        assert!(parse("[a", flags()).is_err());
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        for pattern in [
            "*a",
            "a**",
            "(a",
            "a)",
            r"\q",
            r"\1",
            "(?P<n>a)(?P<n>b)",
            "^*",
            "a*+",
        ] {
            assert!(parse(pattern, flags()).is_err(), "{pattern}");
        }
    }

    #[test]
    fn verbose_patterns_skip_whitespace_and_comments() {
        let parsed = parse_ok("(?x) a b # note\n c");
        let Node::Concat(items) = parsed.node else {
            panic!("a concatenation");
        };
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn global_flags_must_come_first() {
        assert!(parse("a(?i)", flags()).is_err());
        assert!(parse("(?i)a", flags()).is_ok());
        assert!(parse("(?s:a)b", flags()).is_ok());
    }
}
