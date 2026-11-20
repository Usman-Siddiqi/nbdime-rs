//! Pygments' `MarkdownLexer`: a `RegexLexer` with the states `root` and `inline`.
//!
//! The patterns are Python `re` patterns run with `re.MULTILINE`; `\s` and `\w` are spelled out
//! as the character sets of Python's `re` (see `py_tables`) and everything else is syntax that
//! both engines read alike.

use std::fmt::Write;
use std::sync::OnceLock;

use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use regex_automata::{Anchored, Input};

use crate::prettyprint::py_tables::{LEXER_ALIASES, SPACE, WORD};
use crate::prettyprint::PrettyPrintError;

use super::terminal256::Token;

type Tokens = Vec<(Token, String)>;

/// What a rule does with its match.
#[derive(Clone)]
enum Action {
    /// The whole match is one token.
    Whole(Token),
    /// `bygroups(...)`: each group, in order, is a token or is lexed as `inline`.
    Groups(&'static [Part]),
    /// `_handle_codeblock`.
    CodeBlock,
}

#[derive(Clone, Copy)]
enum Part {
    Token(Token),
    Inline,
}

#[derive(Clone)]
struct Rule {
    regex: Regex,
    action: Action,
}

struct Lexer {
    root: Vec<Rule>,
    inline: Vec<Rule>,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Root,
    Inline,
}

/// The tokens of `text` (already preprocessed) from the `root` state.
pub(super) fn tokenize(text: &str) -> Result<Tokens, PrettyPrintError> {
    static LEXER: OnceLock<Lexer> = OnceLock::new();
    let lexer = LEXER.get_or_init(Lexer::new);
    let mut tokens = Vec::new();
    lexer.lex(text, State::Root, &mut tokens)?;
    Ok(tokens)
}

/// The members of a character class (without its brackets) for an inclusive range table.
fn class_body(table: &[u32]) -> String {
    let mut body = String::new();
    for pair in table.chunks_exact(2) {
        let _ = write!(body, "\\x{{{:X}}}-\\x{{{:X}}}", pair[0], pair[1]);
    }
    body
}

impl Lexer {
    fn new() -> Lexer {
        let space = class_body(&SPACE);
        let word = class_body(&WORD);
        let rule = |pattern: &str, action: Action| {
            let pattern = pattern.replace("@S@", &space).replace("@W@", &word);
            let regex = Regex::builder()
                .syntax(syntax::Config::new().multi_line(true))
                .build(&pattern)
                .expect("the Markdown lexer patterns are valid");
            Rule { regex, action }
        };
        use Part::{Inline, Token as T};
        use Token::*;

        let inline = vec![
            rule(r"\\.", Action::Whole(Text)),
            rule(
                r"([^`]?)(`[^`\n]+`)",
                Action::Groups(&[T(Text), T(StringBacktick)]),
            ),
            rule(
                r"([^\*]?)(\*\*\*[^* \n][^*\n]*\*\*\*)",
                Action::Groups(&[T(Text), T(EmphStrong)]),
            ),
            rule(
                r"([^_]?)(___[^_ \n][^_\n]*___)",
                Action::Groups(&[T(Text), T(EmphStrong)]),
            ),
            rule(
                r"([^\*]?)(\*\*[^* \n][^*\n]*\*\*)",
                Action::Groups(&[T(Text), T(Strong)]),
            ),
            rule(
                r"([^_]?)(__[^_ \n][^_\n]*__)",
                Action::Groups(&[T(Text), T(Strong)]),
            ),
            rule(
                r"([^\*]?)(\*[^* \n][^*\n]*\*)",
                Action::Groups(&[T(Text), T(Emph)]),
            ),
            rule(
                r"([^_]?)(_[^_ \n][^_\n]*_)",
                Action::Groups(&[T(Text), T(Emph)]),
            ),
            rule(
                r"([^~]?)(~~[^~ \n][^~\n]*~~)",
                Action::Groups(&[T(Text), T(Deleted)]),
            ),
            rule(r"[@#][@W@/:\-]+", Action::Whole(NameEntity)),
            rule(
                r"(!?\[)([^\]]+)(\])(\()([^)]+)(\))",
                Action::Groups(&[
                    T(Text),
                    T(NameTag),
                    T(Text),
                    T(Text),
                    T(NameAttribute),
                    T(Text),
                ]),
            ),
            rule(
                r"(\[)([^\]]+)(\])(\[)([^\]]*)(\])",
                Action::Groups(&[T(Text), T(NameTag), T(Text), T(Text), T(NameLabel), T(Text)]),
            ),
            rule(
                r"^([@S@]*\[)([^\]]*)(\]:[@S@]*)(.+)",
                Action::Groups(&[T(Text), T(NameLabel), T(Text), T(NameAttribute)]),
            ),
            rule(r"[^\\@S@]+", Action::Whole(Text)),
            rule(r".", Action::Whole(Text)),
        ];

        let mut root = vec![
            rule(r"(^#[^#].+)(\n)", Action::Groups(&[T(Heading), T(Text)])),
            rule(
                r"(^#{2,6}[^#].+)(\n)",
                Action::Groups(&[T(Subheading), T(Text)]),
            ),
            rule(
                r"^(.+)(\n)(=+)(\n)",
                Action::Groups(&[T(Heading), T(Text), T(Heading), T(Text)]),
            ),
            rule(
                r"^(.+)(\n)(-+)(\n)",
                Action::Groups(&[T(Subheading), T(Text), T(Subheading), T(Text)]),
            ),
            rule(
                r"^([@S@]*)([*\-] )(\[[ xX]\])( .+\n)",
                Action::Groups(&[T(Whitespace), T(Keyword), T(Keyword), Inline]),
            ),
            rule(
                r"^([@S@]*)([*\-])([@S@])(.+\n)",
                Action::Groups(&[T(Whitespace), T(Keyword), T(Whitespace), Inline]),
            ),
            rule(
                r"^([@S@]*)([0-9]+\.)( .+\n)",
                Action::Groups(&[T(Whitespace), T(Keyword), Inline]),
            ),
            rule(
                r"^([@S@]*>[@S@])(.+\n)",
                Action::Groups(&[T(Keyword), T(Emph)]),
            ),
            rule(
                r"^([@S@]*```\n(?s:.)*?^[@S@]*```$\n)",
                Action::Whole(StringBacktick),
            ),
            rule(
                r"^(?P<initial>[@S@]*```)(?P<lang>[@W@\-]+)(?P<afterlang>(?P<whitespace>[[@S@]&&[^\n]]+)(?P<extra>.*))?(?P<newline>\n)(?P<code>(?s:.)*?)(?P<terminator>^[@S@]*```$\n)",
                Action::CodeBlock,
            ),
        ];
        // `include('inline')` appends the same rules to the `root` state.
        root.extend(inline.iter().cloned());
        Lexer { root, inline }
    }

    /// `RegexLexer.get_tokens_unprocessed`, appending to `out`.
    fn lex(&self, text: &str, mut state: State, out: &mut Tokens) -> Result<(), PrettyPrintError> {
        let mut pos = 0;
        while pos < text.len() {
            let rules = match state {
                State::Root => &self.root,
                State::Inline => &self.inline,
            };
            let mut caps = None;
            let mut matched_rule = None;
            for rule in rules {
                let input = Input::new(text)
                    .span(pos..text.len())
                    .anchored(Anchored::Yes);
                let mut c = rule.regex.create_captures();
                rule.regex.search_captures(&input, &mut c);
                if c.is_match() {
                    caps = Some(c);
                    matched_rule = Some(rule);
                    break;
                }
            }
            let (Some(caps), Some(rule)) = (caps, matched_rule) else {
                // No rule matches: a newline resets the state, anything else is an error token.
                let c = text[pos..].chars().next().expect("pos is inside the text");
                if c == '\n' {
                    state = State::Root;
                    out.push((Token::Whitespace, "\n".into()));
                } else {
                    out.push((Token::Error, c.to_string()));
                }
                pos += c.len_utf8();
                continue;
            };
            let whole = caps.get_match().expect("a match").span();
            match &rule.action {
                Action::Whole(token) => out.push((*token, text[whole.start..whole.end].into())),
                Action::Groups(parts) => {
                    for (i, part) in parts.iter().enumerate() {
                        let Some(span) = caps.get_group(i + 1) else {
                            continue;
                        };
                        let data = &text[span.start..span.end];
                        match part {
                            Part::Token(token) => {
                                if !data.is_empty() {
                                    out.push((*token, data.into()));
                                }
                            }
                            Part::Inline => self.lex(data, State::Inline, out)?,
                        }
                    }
                }
                Action::CodeBlock => {
                    let group = |name: &str| {
                        caps.get_group_by_name(name)
                            .map(|span| &text[span.start..span.end])
                    };
                    let required = |name| group(name).unwrap_or_default();
                    out.push((Token::StringBacktick, required("initial").into()));
                    let lang = required("lang");
                    out.push((Token::StringBacktick, lang.into()));
                    if group("afterlang").is_some() {
                        out.push((Token::Whitespace, required("whitespace").into()));
                        out.push((Token::Text, required("extra").into()));
                    }
                    out.push((Token::Whitespace, required("newline").into()));
                    // A language Pygments has a lexer for would be highlighted with it.
                    if LEXER_ALIASES
                        .binary_search(&lang.to_lowercase().as_str())
                        .is_ok()
                    {
                        return Err(PrettyPrintError::Unsupported(format!(
                            "highlighting of {lang} code blocks in Markdown is not supported"
                        )));
                    }
                    out.push((Token::String, required("code").into()));
                    out.push((Token::StringBacktick, required("terminator").into()));
                }
            }
            pos = whole.end;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prettyprint::highlight::terminal256::format;

    fn highlight(text: &str) -> String {
        format(&tokenize(text).unwrap())
    }

    #[test]
    fn headings_and_emphasis() {
        assert_eq!(
            highlight("# Title\n\nSome *emph* and **strong** and ***both*** ~~gone~~ `code`\n"),
            "\x1b[38;5;18;01m# Title\x1b[39;00m\n\nSome \x1b[03m*emph*\x1b[00m and \
             \x1b[01m**strong**\x1b[00m and \x1b[01;03m***both***\x1b[00m \
             \x1b[38;5;124m~~gone~~\x1b[39m \x1b[38;5;124m`code`\x1b[39m\n"
        );
        assert_eq!(
            highlight("Heading\n=======\nSub\n---\n"),
            "\x1b[38;5;18;01mHeading\x1b[39;00m\n\x1b[38;5;18;01m=======\x1b[39;00m\n\
             \x1b[38;5;90;01mSub\x1b[39;00m\n\x1b[38;5;90;01m---\x1b[39;00m\n"
        );
    }

    #[test]
    fn lists_and_quotes_lex_their_text_as_inline() {
        assert_eq!(
            highlight("- item with `code`\n* [ ] task\n1. num\n> quote\n"),
            "\x1b[38;5;28;01m-\x1b[39;00m\x1b[38;5;250m \x1b[39mitem with \x1b[38;5;124m`code`\x1b[39m\n\
             \x1b[38;5;28;01m* \x1b[39;00m\x1b[38;5;28;01m[ ]\x1b[39;00m task\n\
             \x1b[38;5;28;01m1.\x1b[39;00m num\n\x1b[38;5;28;01m> \x1b[39;00m\x1b[03mquote\x1b[00m\n"
        );
    }

    #[test]
    fn links_entities_and_labels() {
        assert_eq!(
            highlight("[link](http://x) [ref][id]\n[id]: http://example.com\n@user #topic\n"),
            "[\x1b[38;5;28;01mlink\x1b[39;00m](\x1b[38;5;64mhttp://x\x1b[39m) \
             [\x1b[38;5;28;01mref\x1b[39;00m][\x1b[38;5;100mid\x1b[39m]\n\
             [\x1b[38;5;100mid\x1b[39m]: \x1b[38;5;64mhttp://example.com\x1b[39m\n\
             \x1b[38;5;242;01m@user\x1b[39;00m \x1b[38;5;242;01m#topic\x1b[39;00m\n"
        );
    }

    #[test]
    fn fenced_code_blocks() {
        assert_eq!(
            highlight("```\nplain\n```\n```foo bar\nx\n```\n"),
            "\x1b[38;5;124m```\x1b[39m\n\x1b[38;5;124mplain\x1b[39m\n\x1b[38;5;124m```\x1b[39m\n\
             \x1b[38;5;124m```\x1b[39m\x1b[38;5;124mfoo\x1b[39m\x1b[38;5;250m \x1b[39mbar\n\
             \x1b[38;5;124mx\x1b[39m\n\x1b[38;5;124m```\x1b[39m\n"
        );
    }

    #[test]
    fn languages_with_a_pygments_lexer_are_not_supported() {
        assert!(matches!(
            tokenize("```python\nx = 1\n```\n"),
            Err(PrettyPrintError::Unsupported(_))
        ));
        assert!(matches!(
            tokenize("```Python\nx = 1\n```\n"),
            Err(PrettyPrintError::Unsupported(_))
        ));
    }

    #[test]
    fn escapes_and_plain_text_have_no_style() {
        assert_eq!(
            highlight("\\* escaped\n   indented\n"),
            "\\* escaped\n   indented\n"
        );
    }

    #[test]
    fn newlines_that_no_rule_takes_become_whitespace_tokens() {
        let tokens = tokenize("a\n\nb\n").unwrap();
        assert_eq!(
            tokens,
            [
                (Token::Text, "a".into()),
                (Token::Whitespace, "\n".into()),
                (Token::Whitespace, "\n".into()),
                (Token::Text, "b".into()),
                (Token::Whitespace, "\n".into()),
            ]
        );
    }
}
