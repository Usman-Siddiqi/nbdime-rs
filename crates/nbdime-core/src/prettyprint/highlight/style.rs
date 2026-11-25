//! Pygments' style machinery (`pygments.style.StyleMeta`) and its default style.

use super::token::TokenType;
use super::token::TokenType as T;

/// What a style says about a token type after inheritance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TokenStyle {
    /// `RRGGBB`.
    pub color: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub background: Option<String>,
}

/// `DefaultStyle.styles`.
const DEFAULT_STYLE: &[(TokenType, &str)] = &[
    (T::Whitespace, "#bbbbbb"),
    (T::Comment, "italic #3D7B7B"),
    (T::CommentPreproc, "noitalic #9C6500"),
    (T::Keyword, "bold #008000"),
    (T::KeywordPseudo, "nobold"),
    (T::KeywordType, "nobold #B00040"),
    (T::Operator, "#666666"),
    (T::OperatorWord, "bold #AA22FF"),
    (T::NameBuiltin, "#008000"),
    (T::NameFunction, "#0000FF"),
    (T::NameClass, "bold #0000FF"),
    (T::NameNamespace, "bold #0000FF"),
    (T::NameException, "bold #CB3F38"),
    (T::NameVariable, "#19177C"),
    (T::NameConstant, "#880000"),
    (T::NameLabel, "#767600"),
    (T::NameEntity, "bold #717171"),
    (T::NameAttribute, "#687822"),
    (T::NameTag, "bold #008000"),
    (T::NameDecorator, "#AA22FF"),
    (T::String, "#BA2121"),
    (T::StringDoc, "italic"),
    (T::StringInterpol, "bold #A45A77"),
    (T::StringEscape, "bold #AA5D1F"),
    (T::StringRegex, "#A45A77"),
    (T::StringSymbol, "#19177C"),
    (T::StringOther, "#008000"),
    (T::Number, "#666666"),
    (T::GenericHeading, "bold #000080"),
    (T::GenericSubheading, "bold #800080"),
    (T::GenericDeleted, "#A00000"),
    (T::GenericInserted, "#008400"),
    (T::GenericError, "#E40000"),
    (T::GenericEmph, "italic"),
    (T::GenericStrong, "bold"),
    (T::GenericEmphStrong, "bold italic"),
    (T::GenericPrompt, "bold #000080"),
    (T::GenericOutput, "#717171"),
    (T::GenericTraceback, "#04D"),
    (T::Error, "border:#FF0000"),
];

/// `colorformat`: `#rrggbb` and `#rgb` as `rrggbb`.
fn color(text: &str) -> Option<String> {
    let hex = text.strip_prefix('#')?;
    match hex.len() {
        6 => Some(hex.to_owned()),
        3 => Some(hex.chars().flat_map(|c| [c, c]).collect()),
        _ => None,
    }
}

/// The style of every token type, indexed by `TokenType::ALL`: a type starts from its parent's
/// style and applies its own definition on top.
pub(super) fn default_styles() -> Vec<TokenStyle> {
    let mut styles: Vec<TokenStyle> = Vec::with_capacity(TokenType::ALL.len());
    for &ttype in TokenType::ALL {
        let definition = DEFAULT_STYLE
            .iter()
            .find(|(t, _)| *t == ttype)
            .map_or("", |(_, d)| *d);
        let words: Vec<&str> = definition.split_whitespace().collect();
        let inherited = match ttype.parent() {
            Some(parent) if !words.contains(&"noinherit") => {
                let at = TokenType::ALL.iter().position(|t| *t == parent);
                at.map(|i| styles[i].clone()).unwrap_or_default()
            }
            _ => TokenStyle::default(),
        };
        let mut style = inherited;
        for word in words {
            match word {
                "noinherit" => {}
                "bold" => style.bold = true,
                "nobold" => style.bold = false,
                "italic" => style.italic = true,
                "noitalic" => style.italic = false,
                "underline" => style.underline = true,
                "nounderline" => style.underline = false,
                "roman" | "sans" | "mono" => {}
                w if w.starts_with("bg:") => style.background = color(&w[3..]),
                w if w.starts_with("border:") => {}
                w => style.color = color(w),
            }
        }
        styles.push(style);
    }
    styles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(ttype: TokenType) -> TokenStyle {
        let at = TokenType::ALL.iter().position(|t| *t == ttype).unwrap();
        default_styles().swap_remove(at)
    }

    #[test]
    fn children_inherit_and_override() {
        let keyword = style(T::KeywordNamespace);
        assert_eq!(
            (keyword.color.as_deref(), keyword.bold),
            (Some("008000"), true)
        );
        let pseudo = style(T::KeywordPseudo);
        assert_eq!(
            (pseudo.color.as_deref(), pseudo.bold),
            (Some("008000"), false)
        );
        let doc = style(T::StringDoc);
        assert_eq!((doc.color.as_deref(), doc.italic), (Some("BA2121"), true));
        let nested = style(T::StringDouble);
        assert_eq!(
            (nested.color.as_deref(), nested.italic),
            (Some("BA2121"), false)
        );
        assert_eq!(style(T::Text), TokenStyle::default());
    }

    #[test]
    fn short_colours_expand() {
        assert_eq!(style(T::GenericTraceback).color.as_deref(), Some("0044DD"));
    }
}
