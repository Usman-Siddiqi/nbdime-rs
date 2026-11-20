//! Syntax highlighting as `pygments.highlight(source, lexer, Terminal256Formatter())` does it,
//! for the one language the pretty-printer highlights in diffs and merges: Markdown.

mod markdown;
mod terminal256;

use super::PrettyPrintError;

/// The Markdown `source` of a cell, highlighted for a 256-colour terminal.
pub(super) fn colorize_markdown(source: &str) -> Result<String, PrettyPrintError> {
    let tokens = markdown::tokenize(&preprocess(source))?;
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
}
