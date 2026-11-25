//! Pygments' `MarkdownLexer`: a `RegexLexer` with the states `root` and `inline`.

use std::sync::OnceLock;

use crate::prettyprint::highlight::lexer::{
    by_groups, callback, rule, token, using_this_in, LexerDef, Matched, RegexLexer, RuleDef, Token,
};
use crate::prettyprint::highlight::pyre::Flags;
use crate::prettyprint::highlight::token::TokenType as T;
use crate::prettyprint::highlight::{find_lexer, Lookup};
use crate::prettyprint::PrettyPrintError;

/// The aliases `MarkdownLexer` is registered under.
pub(in crate::prettyprint::highlight) const ALIASES: [&str; 2] = ["markdown", "md"];

pub(in crate::prettyprint::highlight) fn lexer() -> &'static RegexLexer {
    static LEXER: OnceLock<RegexLexer> = OnceLock::new();
    LEXER.get_or_init(|| RegexLexer::new(&definition()))
}

/// `MarkdownLexer._handle_codeblock`: the code of a fenced block is lexed with the lexer its
/// language names. A language whose lexer is not ported is left unhighlighted.
fn handle_codeblock<'t>(
    _lexer: &RegexLexer,
    m: &Matched<'_, 't>,
    depth: usize,
    out: &mut Vec<Token<'t>>,
) -> Result<(), PrettyPrintError> {
    let group = |name| m.named(name).unwrap_or_default();
    out.push((T::StringBacktick, group("initial")));
    let lang = group("lang");
    out.push((T::StringBacktick, lang));
    if m.named("afterlang").is_some() {
        out.push((T::Whitespace, group("whitespace")));
        out.push((T::Text, group("extra")));
    }
    out.push((T::Whitespace, group("newline")));
    let code = group("code");
    match find_lexer(lang.trim()) {
        Lookup::Ported(lexer) => lexer.tokens(code, &["root"], depth + 1, out)?,
        // No lexer for this language: handle it like it was a code block.
        Lookup::NotFound => out.push((T::String, code)),
        Lookup::Unported => out.push((T::Text, code)),
    }
    out.push((T::StringBacktick, group("terminator")));
    Ok(())
}

fn definition() -> LexerDef {
    use RuleDef::Include;
    let inline = || using_this_in("inline");
    let states = vec![
        (
            "root",
            vec![
                // heading with '#' prefix (atx-style)
                by_groups(
                    r"(^#[^#].+)(\n)",
                    vec![token(T::GenericHeading), token(T::Text)],
                ),
                // subheading with '#' prefix (atx-style)
                by_groups(
                    r"(^#{2,6}[^#].+)(\n)",
                    vec![token(T::GenericSubheading), token(T::Text)],
                ),
                // heading with '=' underlines (Setext-style)
                by_groups(
                    r"^(.+)(\n)(=+)(\n)",
                    vec![
                        token(T::GenericHeading),
                        token(T::Text),
                        token(T::GenericHeading),
                        token(T::Text),
                    ],
                ),
                // subheading with '-' underlines (Setext-style)
                by_groups(
                    r"^(.+)(\n)(-+)(\n)",
                    vec![
                        token(T::GenericSubheading),
                        token(T::Text),
                        token(T::GenericSubheading),
                        token(T::Text),
                    ],
                ),
                // task list
                by_groups(
                    r"^(\s*)([*-] )(\[[ xX]\])( .+\n)",
                    vec![
                        token(T::Whitespace),
                        token(T::Keyword),
                        token(T::Keyword),
                        inline(),
                    ],
                ),
                // bulleted list
                by_groups(
                    r"^(\s*)([*-])(\s)(.+\n)",
                    vec![
                        token(T::Whitespace),
                        token(T::Keyword),
                        token(T::Whitespace),
                        inline(),
                    ],
                ),
                // numbered list
                by_groups(
                    r"^(\s*)([0-9]+\.)( .+\n)",
                    vec![token(T::Whitespace), token(T::Keyword), inline()],
                ),
                // quote
                by_groups(
                    r"^(\s*>\s)(.+\n)",
                    vec![token(T::Keyword), token(T::GenericEmph)],
                ),
                // code block fenced by 3 backticks
                rule(r"^(\s*```\n[\w\W]*?^\s*```$\n)", T::StringBacktick),
                // code block with language
                callback(
                    r"(?x)
              ^(?P<initial>\s*```)
              (?P<lang>[\w\-]+)
              (?P<afterlang>
                 (?P<whitespace>[^\S\n]+)
                 (?P<extra>.*))?
              (?P<newline>\n)
              (?P<code>([\s\S])*?)
              (?P<terminator>^\s*```$\n)
              ",
                    handle_codeblock,
                ),
                Include("inline"),
            ],
        ),
        (
            "inline",
            vec![
                // escape
                rule(r"\\.", T::Text),
                // inline code
                by_groups(
                    r"([^`]?)(`[^`\n]+`)",
                    vec![token(T::Text), token(T::StringBacktick)],
                ),
                // bold-italics fenced by '***'
                by_groups(
                    r"([^\*]?)(\*\*\*[^* \n][^*\n]*\*\*\*)",
                    vec![token(T::Text), token(T::GenericEmphStrong)],
                ),
                // bold-italics fenced by '___'
                by_groups(
                    r"([^_]?)(___[^_ \n][^_\n]*___)",
                    vec![token(T::Text), token(T::GenericEmphStrong)],
                ),
                // bold fenced by '**'
                by_groups(
                    r"([^\*]?)(\*\*[^* \n][^*\n]*\*\*)",
                    vec![token(T::Text), token(T::GenericStrong)],
                ),
                // bold fenced by '__'
                by_groups(
                    r"([^_]?)(__[^_ \n][^_\n]*__)",
                    vec![token(T::Text), token(T::GenericStrong)],
                ),
                // italics fenced by '*'
                by_groups(
                    r"([^\*]?)(\*[^* \n][^*\n]*\*)",
                    vec![token(T::Text), token(T::GenericEmph)],
                ),
                // italics fenced by '_'
                by_groups(
                    r"([^_]?)(_[^_ \n][^_\n]*_)",
                    vec![token(T::Text), token(T::GenericEmph)],
                ),
                // strikethrough
                by_groups(
                    r"([^~]?)(~~[^~ \n][^~\n]*~~)",
                    vec![token(T::Text), token(T::GenericDeleted)],
                ),
                // mentions and topics (twitter and github stuff)
                rule(r"[@#][\w/:-]+", T::NameEntity),
                // (image?) links eg: ![Image of Yaktocat](https://octodex.github.com/images/yaktocat.png)
                by_groups(
                    r"(!?\[)([^]]+)(\])(\()([^)]+)(\))",
                    vec![
                        token(T::Text),
                        token(T::NameTag),
                        token(T::Text),
                        token(T::Text),
                        token(T::NameAttribute),
                        token(T::Text),
                    ],
                ),
                // reference-style links, e.g.:
                //   [an example][id]
                //   [id]: http://example.com/
                by_groups(
                    r"(\[)([^]]+)(\])(\[)([^]]*)(\])",
                    vec![
                        token(T::Text),
                        token(T::NameTag),
                        token(T::Text),
                        token(T::Text),
                        token(T::NameLabel),
                        token(T::Text),
                    ],
                ),
                by_groups(
                    r"^(\s*\[)([^]]*)(\]:\s*)(.+)",
                    vec![
                        token(T::Text),
                        token(T::NameLabel),
                        token(T::Text),
                        token(T::NameAttribute),
                    ],
                ),
                // general text, must come last!
                rule(r"[^\\\s]+", T::Text),
                rule(r".", T::Text),
            ],
        ),
    ];
    LexerDef {
        states,
        flags: Flags {
            multiline: true,
            ..Flags::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::prettyprint::highlight::colorize_source;

    fn highlight(text: &str) -> String {
        colorize_source(text, "markdown").unwrap()
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
    fn fenced_code_is_highlighted_with_the_lexer_of_its_language() {
        let python = highlight("```python\nx = 1\n```\n");
        assert_eq!(
            python,
            "\x1b[38;5;124m```\x1b[39m\x1b[38;5;124mpython\x1b[39m\nx \x1b[38;5;241m=\x1b[39m \
             \x1b[38;5;241m1\x1b[39m\n\x1b[38;5;124m```\x1b[39m\n"
        );
        assert_eq!(
            python.replace("python", "Python"),
            highlight("```Python\nx = 1\n```\n")
        );
    }

    #[test]
    fn code_in_a_language_that_is_not_ported_is_left_alone() {
        assert_eq!(
            highlight("```rust\nlet x = 1;\n```\n"),
            "\x1b[38;5;124m```\x1b[39m\x1b[38;5;124mrust\x1b[39m\nlet x = 1;\n\x1b[38;5;124m```\x1b[39m\n"
        );
        // Pygments has no lexer for this name: the code is a string.
        assert!(highlight("```nosuchlexer\nx\n```\n").contains("\x1b[38;5;124mx\x1b[39m"));
    }

    #[test]
    fn escapes_and_plain_text_have_no_style() {
        assert_eq!(
            highlight("\\* escaped\n   indented\n"),
            "\\* escaped\n   indented\n"
        );
    }
}
