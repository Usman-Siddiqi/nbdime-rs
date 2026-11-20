//! `Terminal256Formatter` with Pygments' default style, for the token types the Markdown lexer
//! produces.

/// The token types of the Markdown lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Text,
    Whitespace,
    Error,
    Keyword,
    Heading,
    Subheading,
    Emph,
    Strong,
    EmphStrong,
    Deleted,
    StringBacktick,
    String,
    NameEntity,
    NameTag,
    NameAttribute,
    NameLabel,
}

impl Token {
    /// The escape sequences that start and end a token of this type (`on`, `off`).
    fn style(self) -> (&'static str, &'static str) {
        match self {
            Token::Text | Token::Error => ("", ""),
            Token::Whitespace => ("\x1b[38;5;250m", "\x1b[39m"),
            Token::Keyword | Token::NameTag => ("\x1b[38;5;28;01m", "\x1b[39;00m"),
            Token::Heading => ("\x1b[38;5;18;01m", "\x1b[39;00m"),
            Token::Subheading => ("\x1b[38;5;90;01m", "\x1b[39;00m"),
            Token::Emph => ("\x1b[03m", "\x1b[00m"),
            Token::Strong => ("\x1b[01m", "\x1b[00m"),
            Token::EmphStrong => ("\x1b[01;03m", "\x1b[00m"),
            Token::Deleted | Token::StringBacktick | Token::String => {
                ("\x1b[38;5;124m", "\x1b[39m")
            }
            Token::NameEntity => ("\x1b[38;5;242;01m", "\x1b[39;00m"),
            Token::NameAttribute => ("\x1b[38;5;64m", "\x1b[39m"),
            Token::NameLabel => ("\x1b[38;5;100m", "\x1b[39m"),
        }
    }
}

/// `Terminal256Formatter.format_unencoded`: every line of a token is wrapped on its own, so
/// that the colours end at each newline.
pub(super) fn format(tokens: &[(Token, String)]) -> String {
    let mut out = String::new();
    for (token, value) in tokens {
        let (on, off) = token.style();
        let mut lines = value.split('\n').peekable();
        while let Some(line) = lines.next() {
            if !line.is_empty() {
                out.push_str(on);
                out.push_str(line);
                out.push_str(off);
            }
            if lines.peek().is_some() {
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_end_at_each_newline() {
        let tokens = [(Token::Heading, "# a\nb\n".to_owned())];
        assert_eq!(
            format(&tokens),
            "\x1b[38;5;18;01m# a\x1b[39;00m\n\x1b[38;5;18;01mb\x1b[39;00m\n"
        );
    }

    #[test]
    fn plain_tokens_are_written_as_they_are() {
        let tokens = [
            (Token::Text, "x y".to_owned()),
            (Token::Error, "\n".to_owned()),
        ];
        assert_eq!(format(&tokens), "x y\n");
    }
}
