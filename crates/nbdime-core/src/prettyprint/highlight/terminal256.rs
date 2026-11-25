//! `Terminal256Formatter` with Pygments' default style.

use std::sync::OnceLock;

use super::style::{default_styles, TokenStyle};
use super::token::TokenType;

/// The xterm palette: 16 system colours, the 6x6x6 cube and 24 greys.
fn palette() -> [(i32, i32, i32); 256] {
    let mut colors = [(0, 0, 0); 256];
    let system = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    colors[..16].copy_from_slice(&system);
    let levels = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    for i in 0..216 {
        colors[16 + i] = (levels[(i / 36) % 6], levels[(i / 6) % 6], levels[i % 6]);
    }
    for i in 0..24 {
        let v = 8 + i as i32 * 10;
        colors[232 + i] = (v, v, v);
    }
    colors
}

/// `_closest_color` for the colour `rrggbb`; text that is not hexadecimal is black.
fn color_index(color: &str, palette: &[(i32, i32, i32); 256]) -> usize {
    let rgb = i64::from_str_radix(color, 16).unwrap_or(0);
    let (r, g, b) = (
        ((rgb >> 16) & 0xff) as i32,
        ((rgb >> 8) & 0xff) as i32,
        (rgb & 0xff) as i32,
    );
    let mut distance = 257 * 257 * 3;
    let mut best = 0;
    for (i, &(pr, pg, pb)) in palette.iter().enumerate() {
        let d = (r - pr).pow(2) + (g - pg).pow(2) + (b - pb).pow(2);
        if d < distance {
            best = i;
            distance = d;
        }
    }
    best
}

/// `EscapeSequence.color_string` and `reset_string`.
fn escapes(style: &TokenStyle, palette: &[(i32, i32, i32); 256]) -> (String, String) {
    let mut on = Vec::new();
    let mut off = Vec::new();
    if let Some(color) = &style.color {
        on.push(format!("38;5;{}", color_index(color, palette)));
        off.push("39");
    }
    if let Some(background) = &style.background {
        on.push(format!("48;5;{}", color_index(background, palette)));
        off.push("49");
    }
    if style.bold {
        on.push("01".to_owned());
    }
    if style.underline {
        on.push("04".to_owned());
    }
    if style.italic {
        on.push("03".to_owned());
    }
    if style.bold || style.underline || style.italic {
        off.push("00");
    }
    let wrap = |parts: Vec<String>| {
        if parts.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", parts.join(";"))
        }
    };
    (wrap(on), wrap(off.into_iter().map(str::to_owned).collect()))
}

/// The escape sequences that start and end each token type, indexed by `TokenType::ALL`.
fn sequences() -> &'static [(String, String)] {
    static SEQUENCES: OnceLock<Vec<(String, String)>> = OnceLock::new();
    SEQUENCES.get_or_init(|| {
        let palette = palette();
        default_styles()
            .iter()
            .map(|style| escapes(style, &palette))
            .collect()
    })
}

/// `Terminal256Formatter.format_unencoded`: the colours of a token end at each newline, so
/// that paging the output works.
pub(super) fn format(tokens: &[(TokenType, &str)]) -> String {
    let sequences = sequences();
    let mut out = String::new();
    for &(ttype, value) in tokens {
        if ttype == TokenType::Token {
            out.push_str(value);
            continue;
        }
        let at = TokenType::ALL
            .iter()
            .position(|t| *t == ttype)
            .expect("every token type is listed");
        let (on, off) = &sequences[at];
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
    fn colours_map_to_the_nearest_palette_entry() {
        let palette = palette();
        assert_eq!(color_index("000080", &palette), 18);
        assert_eq!(color_index("BA2121", &palette), 124);
        assert_eq!(color_index("bbbbbb", &palette), 250);
        assert_eq!(color_index("zzzzzz", &palette), 0);
    }

    #[test]
    fn colours_end_at_each_newline() {
        let tokens = [(TokenType::GenericHeading, "# a\nb\n")];
        assert_eq!(
            format(&tokens),
            "\x1b[38;5;18;01m# a\x1b[39;00m\n\x1b[38;5;18;01mb\x1b[39;00m\n"
        );
    }

    #[test]
    fn plain_tokens_are_written_as_they_are() {
        let tokens = [
            (TokenType::Text, "x y"),
            (TokenType::Error, "\n"),
            (TokenType::Token, "z"),
        ];
        assert_eq!(format(&tokens), "x y\nz");
    }

    #[test]
    fn styled_tokens_carry_their_attributes() {
        let tokens = [
            (TokenType::StringDoc, "\"\"\"d\"\"\""),
            (TokenType::GenericEmph, "e"),
            (TokenType::GenericEmphStrong, "s"),
        ];
        assert_eq!(
            format(&tokens),
            "\x1b[38;5;124;03m\"\"\"d\"\"\"\x1b[39;00m\x1b[03me\x1b[00m\x1b[01;03ms\x1b[00m"
        );
    }
}
