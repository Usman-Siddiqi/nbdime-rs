//! Syntax highlighting as `pygments.highlight(source, lexer, Terminal256Formatter())` does it:
//! `colorize_source` of `nbdime.prettyprint`.
//!
//! Lexers are ported one by one (see [`lexers`]); the regex-lexer machinery, token types,
//! style and terminal formatter are Pygments' own, so more lexers can be added as definitions.
//! A language Pygments has a lexer for but which is not ported here is left unhighlighted
//! (findings F-032).

mod lexer;
mod lexers;
mod pyre;
mod regexopt;
mod style;
mod terminal256;
mod token;

use lexer::RegexLexer;

use super::py_tables::LEXER_ALIASES;
use super::PrettyPrintError;

/// What `find_lexer_class_by_name` finds for a name.
pub(super) enum Lookup {
    Ported(&'static RegexLexer),
    /// Pygments has a lexer by this name which is not ported here.
    Unported,
    /// `ClassNotFound`.
    NotFound,
}

/// `find_lexer_class_by_name`: the lexer registered under `alias` (case-insensitively).
pub(super) fn find_lexer(alias: &str) -> Lookup {
    if alias.is_empty() {
        return Lookup::NotFound;
    }
    let alias = alias.to_lowercase();
    if lexers::python::ALIASES.contains(&alias.as_str()) {
        Lookup::Ported(lexers::python::lexer())
    } else if lexers::markdown::ALIASES.contains(&alias.as_str()) {
        Lookup::Ported(lexers::markdown::lexer())
    } else if LEXER_ALIASES.binary_search(&alias.as_str()).is_ok() {
        Lookup::Unported
    } else {
        Lookup::NotFound
    }
}

/// `colorize_source`: `source` highlighted for a 256-colour terminal with the lexer named
/// `lexer_name`, or unchanged when there is none.
pub(super) fn colorize_source(source: &str, lexer_name: &str) -> Result<String, PrettyPrintError> {
    let Lookup::Ported(lexer) = find_lexer(lexer_name) else {
        return Ok(source.to_owned());
    };
    let text = preprocess(source);
    let mut tokens = Vec::new();
    lexer.tokens(&text, &["root"], 0, &mut tokens)?;
    Ok(terminal256::format(&tokens))
}

/// `Lexer._preprocess_lexer_input` with the default options (`stripnl`, `ensurenl`).
fn preprocess(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut text = text.trim_matches('\n').to_owned();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocessing_normalizes_newlines_and_strips_the_outer_ones() {
        assert_eq!(preprocess("\n\na\r\nb\rc\n\n"), "a\nb\nc\n");
        assert_eq!(preprocess(""), "\n");
        assert_eq!(preprocess("\u{feff}x"), "x\n");
        assert_eq!(preprocess("\n\n\n"), "\n");
    }

    #[test]
    fn lexers_are_found_by_alias_without_regard_to_case() {
        for alias in [
            "python", "PY", "Python3", "pyi", "sage", "bazel", "starlark", "py3",
        ] {
            assert!(matches!(find_lexer(alias), Lookup::Ported(_)), "{alias}");
        }
        assert!(matches!(find_lexer("md"), Lookup::Ported(_)));
        assert!(matches!(find_lexer("rust"), Lookup::Unported));
        assert!(matches!(find_lexer("python2"), Lookup::Unported));
        for alias in ["", "ipython", "ipython3", "nosuchlexer"] {
            assert!(matches!(find_lexer(alias), Lookup::NotFound), "{alias:?}");
        }
    }

    #[test]
    fn sources_without_a_lexer_are_unchanged() {
        assert_eq!(
            colorize_source("x = 1\r\n", "ipython3").unwrap(),
            "x = 1\r\n"
        );
        assert_eq!(colorize_source("x = 1", "rust").unwrap(), "x = 1");
    }

    #[test]
    fn python_source_is_highlighted() {
        assert_eq!(
            colorize_source("def f(x):\n    return x + 1  # c\n", "python").unwrap(),
            "\x1b[38;5;28;01mdef\x1b[39;00m\x1b[38;5;250m \x1b[39m\x1b[38;5;21mf\x1b[39m(x):\n    \
             \x1b[38;5;28;01mreturn\x1b[39;00m x \x1b[38;5;241m+\x1b[39m \
             \x1b[38;5;241m1\x1b[39m  \x1b[38;5;66;03m# c\x1b[39;00m\n"
        );
    }
}
