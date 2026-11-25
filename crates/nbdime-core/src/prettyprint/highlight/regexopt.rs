//! `pygments.regexopt.regex_opt`: a pattern that matches any of a list of literal words. The
//! pattern text is what Pygments builds, so matching preference between words that are
//! prefixes of each other stays the same.

use std::collections::BTreeSet;

/// `re.escape`.
fn escape(text: &[char]) -> String {
    let mut out = String::new();
    for &c in text {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn common_prefix(strings: &[Vec<char>]) -> &[char] {
    let (Some(min), Some(max)) = (strings.iter().min(), strings.iter().max()) else {
        return &[];
    };
    let len = min.iter().zip(max).take_while(|(a, b)| a == b).count();
    &min[..len]
}

fn make_charset(letters: &[Vec<char>]) -> String {
    let mut out = String::from("[");
    for letter in letters.iter().flatten() {
        if "[^\\-]".contains(*letter) {
            out.push('\\');
        }
        out.push(*letter);
    }
    out.push(']');
    out
}

/// `regex_opt_inner`: `strings` is sorted.
fn inner(strings: &[Vec<char>], open: &str) -> String {
    let close = if open.is_empty() { "" } else { ")" };
    let Some(first) = strings.first() else {
        return String::new();
    };
    if strings.len() == 1 {
        return format!("{open}{}{close}", escape(first));
    }
    if first.is_empty() {
        return format!("{open}{}?{close}", inner(&strings[1..], "(?:"));
    }
    if first.len() == 1 {
        let (one, rest): (Vec<_>, Vec<_>) = strings.iter().cloned().partition(|s| s.len() == 1);
        if one.len() > 1 {
            if !rest.is_empty() {
                return format!("{open}{}|{}{close}", inner(&rest, ""), make_charset(&one));
            }
            return format!("{open}{}{close}", make_charset(&one));
        }
    }
    let prefix = common_prefix(strings);
    if !prefix.is_empty() {
        let tails: Vec<Vec<char>> = strings.iter().map(|s| s[prefix.len()..].to_vec()).collect();
        return format!("{open}{}{}{close}", escape(prefix), inner(&tails, "(?:"));
    }
    let reversed: Vec<Vec<char>> = strings
        .iter()
        .map(|s| s.iter().rev().copied().collect())
        .collect();
    let suffix = common_prefix(&reversed);
    if !suffix.is_empty() {
        let mut heads: Vec<Vec<char>> = strings
            .iter()
            .map(|s| s[..s.len() - suffix.len()].to_vec())
            .collect();
        heads.sort();
        let suffix: Vec<char> = suffix.iter().rev().copied().collect();
        return format!("{open}{}{}{close}", inner(&heads, "(?:"), escape(&suffix));
    }
    // The strings that start like the first one, then all the others.
    let split = strings
        .iter()
        .position(|s| s[0] != first[0])
        .unwrap_or(strings.len());
    let groups = [&strings[..split], &strings[split..]];
    let parts: Vec<String> = groups
        .iter()
        .filter(|g| !g.is_empty())
        .map(|g| inner(g, ""))
        .collect();
    format!("{open}{}{close}", parts.join("|"))
}

/// `regex_opt(strings, prefix, suffix)`.
pub(super) fn regex_opt(words: &[&str], prefix: &str, suffix: &str) -> String {
    let sorted: BTreeSet<Vec<char>> = words.iter().map(|w| w.chars().collect()).collect();
    let sorted: Vec<Vec<char>> = sorted.into_iter().collect();
    format!("{prefix}{}{suffix}", inner(&sorted, "("))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_words_and_charsets() {
        assert_eq!(regex_opt(&["abs"], "", ""), "(abs)");
        assert_eq!(regex_opt(&["a", "b", "-"], "", ""), "([\\-ab])");
    }

    #[test]
    fn common_prefixes_and_suffixes_are_factored() {
        assert_eq!(regex_opt(&["abs", "all", "any"], "", ""), "(a(?:bs|ll|ny))");
        assert_eq!(regex_opt(&["ab", "cb"], "", ""), "((?:[ac])b)");
        assert_eq!(
            regex_opt(&["yield", "yield from"], "", "\\b"),
            "(yield(?:(?:\\ from)?))\\b"
        );
    }

    #[test]
    fn unrelated_words_are_alternatives() {
        assert_eq!(
            regex_opt(&["for", "in", "if"], "(?<!\\.)", ""),
            "(?<!\\.)(for|i(?:[fn]))"
        );
    }
}
