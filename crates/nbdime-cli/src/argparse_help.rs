//! The usage and help texts of Python 3.13's `argparse.HelpFormatter` and `textwrap`, so that
//! what the commands print for `--help` and after a usage error is what upstream prints.

use crate::argparse::{Action, Kind, Nargs, Parser};

/// `shutil.get_terminal_size().columns`: `COLUMNS` if it is a positive integer, else the width of
/// the terminal on standard output, else 80.
fn terminal_columns() -> usize {
    let from_env = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|columns| *columns > 0);
    if let Some(columns) = from_env {
        return usize::try_from(columns).unwrap_or(usize::MAX);
    }
    stdout_columns().unwrap_or(80)
}

#[allow(unsafe_code)]
fn stdout_columns() -> Option<usize> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes one `winsize` through the pointer, which is valid for the call.
    let status = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) };
    (status == 0 && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `[^\d\W]`: a word character that is not a digit.
fn is_letter(c: char) -> bool {
    is_word_char(c) && !c.is_ascii_digit()
}

/// `[\w!"'&.,?]`.
fn is_word_punct(c: char) -> bool {
    is_word_char(c) || matches!(c, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

/// `textwrap`'s whitespace: `\t\n\x0b\x0c\r` and the space.
fn is_wrap_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

/// Splits text the way `TextWrapper.wordsep_re` does: runs of whitespace, em-dashes between
/// words, and words, broken after a hyphen inside a hyphenated word.
fn split_chunks(text: &[char]) -> Vec<String> {
    let at =
        |i: isize| -> Option<char> { usize::try_from(i).ok().and_then(|i| text.get(i).copied()) };
    let dashes_then_word = |from: usize| -> bool {
        let run = text[from..].iter().take_while(|c| **c == '-').count();
        run >= 2 && text.get(from + run).is_some_and(|c| is_word_char(*c))
    };
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let start = i;
        if is_wrap_space(text[i]) {
            while i < text.len() && is_wrap_space(text[i]) {
                i += 1;
            }
        } else if text[i] == '-'
            && at(i as isize - 1).is_some_and(is_word_punct)
            && dashes_then_word(i)
        {
            while i < text.len() && text[i] == '-' {
                i += 1;
            }
        } else {
            let mut end = i + 1;
            loop {
                let j = end as isize;
                let hyphenated = at(j) == Some('-')
                    && ((at(j - 2).is_some_and(is_letter) && at(j - 1).is_some_and(is_letter))
                        || (at(j - 3).is_some_and(is_letter)
                            && at(j - 2) == Some('-')
                            && at(j - 1).is_some_and(is_letter)))
                    && at(j + 1).is_some_and(is_letter)
                    && (at(j + 2).is_some_and(is_letter)
                        || (at(j + 2) == Some('-') && at(j + 3).is_some_and(is_letter)));
                if hyphenated {
                    end += 1;
                    break;
                }
                if end >= text.len() || is_wrap_space(text[end]) {
                    break;
                }
                if at(j - 1).is_some_and(is_word_punct) && dashes_then_word(end) {
                    break;
                }
                end += 1;
            }
            i = end;
        }
        chunks.push(text[start..i].iter().collect());
    }
    chunks
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `textwrap.wrap(text, width, initial_indent=..., subsequent_indent=...)` with the defaults of
/// `TextWrapper` otherwise.
pub(crate) fn wrap(
    text: &str,
    width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
) -> Vec<String> {
    let munged: Vec<char> = text
        .chars()
        .map(|c| if is_wrap_space(c) { ' ' } else { c })
        .collect();
    let mut chunks = split_chunks(&munged);
    chunks.reverse();
    let mut lines: Vec<String> = Vec::new();
    while !chunks.is_empty() {
        let mut current: Vec<String> = Vec::new();
        let mut current_len = 0;
        let indent = if lines.is_empty() {
            initial_indent
        } else {
            subsequent_indent
        };
        let line_width = width.saturating_sub(char_len(indent));
        if chunks.last().is_some_and(|c| c.trim().is_empty()) && !lines.is_empty() {
            chunks.pop();
        }
        while let Some(last) = chunks.last() {
            let len = char_len(last);
            if current_len + len <= line_width {
                current_len += len;
                current.push(chunks.pop().unwrap_or_default());
            } else {
                break;
            }
        }
        if chunks.last().is_some_and(|c| char_len(c) > line_width) {
            let space_left = if line_width < 1 {
                1
            } else {
                line_width.saturating_sub(current_len)
            };
            if space_left > 0 {
                let chunk: Vec<char> = chunks
                    .last()
                    .map(|c| c.chars().collect())
                    .unwrap_or_default();
                let mut end = space_left.min(chunk.len());
                if chunk.len() > space_left {
                    let hyphen = chunk[..space_left].iter().rposition(|c| *c == '-');
                    if let Some(h) =
                        hyphen.filter(|h| *h > 0 && chunk[..*h].iter().any(|c| *c != '-'))
                    {
                        end = h + 1;
                    }
                }
                current.push(chunk[..end].iter().collect());
                if let Some(last) = chunks.last_mut() {
                    *last = chunk[end..].iter().collect();
                }
            } else if current.is_empty() {
                current.push(chunks.pop().unwrap_or_default());
            }
        }
        if current.last().is_some_and(|c| c.trim().is_empty()) {
            current.pop();
        }
        if !current.is_empty() {
            lines.push(format!("{indent}{}", current.concat()));
        }
    }
    lines
}

/// `HelpFormatter._whitespace_matcher.sub(' ', text).strip()`: runs of ASCII whitespace become
/// one space.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim().to_owned()
}

/// What an action is shown as: the pieces of `argparse`'s formatting that depend on it.
struct View<'a>(&'a Action);

impl View<'_> {
    fn is_positional(&self) -> bool {
        self.0.option_strings.is_empty()
    }

    /// `action.nargs == 0`.
    fn takes_no_value(&self) -> bool {
        !self.is_positional() && self.0.kind != Kind::Store
    }

    fn metavar(&self, default: &str) -> String {
        match &self.0.choices {
            Some(choices) => format!("{{{}}}", choices.join(",")),
            None => default.to_owned(),
        }
    }

    fn format_args(&self, default: &str) -> String {
        let metavar = self.metavar(default);
        match self.0.kind {
            Kind::Positional(Nargs::One) | Kind::Store => metavar,
            Kind::Positional(Nargs::Optional) => format!("[{metavar}]"),
            Kind::Positional(Nargs::ZeroOrMore) => format!("[{metavar} ...]"),
            Kind::Positional(Nargs::Parser) => format!("{metavar} ..."),
            _ => String::new(),
        }
    }

    fn invocation(&self) -> String {
        if self.is_positional() {
            return self.metavar(&self.0.dest);
        }
        if self.takes_no_value() {
            return self.0.option_strings.join(", ");
        }
        let args = self.format_args(&self.0.dest.to_uppercase());
        format!("{} {args}", self.0.option_strings.join(", "))
    }
}

struct Formatter {
    width: usize,
    max_help_position: usize,
    action_max_length: usize,
}

const INDENT: usize = 2;

impl Formatter {
    fn new() -> Self {
        let width = terminal_columns().saturating_sub(2);
        Formatter {
            width,
            max_help_position: 24.min(width.saturating_sub(20).max(INDENT * 2)),
            action_max_length: 0,
        }
    }

    /// `_get_actions_usage_parts_with_split`: the pieces of the usage line, and where the
    /// positionals start in them.
    fn usage_parts(
        &self,
        parser: &Parser,
        actions: &[&Action],
        optionals: usize,
    ) -> (Vec<String>, usize) {
        let position_of = |action: &Action| actions.iter().position(|a| std::ptr::eq(*a, action));
        let mut inserts: Vec<(usize, usize, bool)> = Vec::new();
        let mut in_group = vec![false; actions.len()];
        for group in parser.exclusive_groups() {
            let members: Vec<&Action> = group
                .members
                .iter()
                .map(|i| &parser.actions()[*i])
                .collect();
            let Some(start) = members.first().and_then(|first| position_of(first)) else {
                continue;
            };
            let end = start + members.len();
            if end <= actions.len()
                && members
                    .iter()
                    .zip(&actions[start..end])
                    .all(|(a, b)| std::ptr::eq(*a, *b))
            {
                in_group[start..end].iter_mut().for_each(|g| *g = true);
                inserts.push((start, end, group.required));
            }
        }
        let mut parts: Vec<Option<String>> = actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let view = View(action);
                Some(if view.is_positional() {
                    let part = view.format_args(&action.dest);
                    if in_group[i] && part.starts_with('[') && part.ends_with(']') {
                        part[1..part.len() - 1].to_owned()
                    } else {
                        part
                    }
                } else {
                    let first = &action.option_strings[0];
                    let part = if view.takes_no_value() {
                        first.clone()
                    } else {
                        format!("{first} {}", view.format_args(&action.dest.to_uppercase()))
                    };
                    if in_group[i] {
                        part
                    } else {
                        format!("[{part}]")
                    }
                })
            })
            .collect();
        let mut separated = std::collections::HashSet::new();
        inserts.sort_unstable();
        for (start, end, required) in inserts.into_iter().rev() {
            let mut group_parts: Vec<String> =
                parts[start..end].iter().flatten().cloned().collect();
            let size = group_parts.len();
            let (open, close) = if required {
                if size > 1 {
                    ("(", ")")
                } else {
                    ("", "")
                }
            } else {
                ("[", "]")
            };
            group_parts[0] = format!("{open}{}", group_parts[0]);
            group_parts[size - 1] = format!("{}{close}", group_parts[size - 1]);
            for (i, part) in (start..).zip(&group_parts[..size - 1]) {
                if separated.insert(i) {
                    parts[i] = Some(format!("{part} |"));
                }
            }
            parts[start + size - 1] = Some(group_parts[size - 1].clone());
            for part in &mut parts[start + size..end] {
                *part = None;
            }
        }
        let split = parts[..optionals].iter().flatten().count();
        (parts.into_iter().flatten().collect(), split)
    }

    /// `_format_usage`, without the trailing blank line.
    fn usage(&self, parser: &Parser) -> String {
        let prefix = "usage: ";
        let prog = parser.prog.as_str();
        let mut optionals: Vec<&Action> = Vec::new();
        let mut positionals: Vec<&Action> = Vec::new();
        for action in parser.actions() {
            if action.option_strings.is_empty() {
                positionals.push(action);
            } else {
                optionals.push(action);
            }
        }
        let all: Vec<&Action> = optionals.iter().chain(&positionals).copied().collect();
        let (parts, _) = self.usage_parts(parser, &all, optionals.len());
        let action_usage = parts.join(" ");
        let mut usage = if action_usage.is_empty() {
            prog.to_owned()
        } else {
            format!("{prog} {action_usage}")
        };
        let text_width = self.width;
        if char_len(prefix) + char_len(&usage) > text_width {
            let (parts, pos_start) = self.usage_parts(parser, &all, optionals.len());
            let (opt_parts, pos_parts) = parts.split_at(pos_start);
            let lines = Self::wrap_usage(prog, opt_parts, pos_parts, prefix, text_width);
            usage = lines.join("\n");
        }
        format!("{prefix}{usage}\n")
    }

    fn wrap_usage(
        prog: &str,
        opt_parts: &[String],
        pos_parts: &[String],
        prefix: &str,
        text_width: usize,
    ) -> Vec<String> {
        let get_lines = |parts: &[String], indent: &str, prefix: Option<&str>| -> Vec<String> {
            let mut lines = Vec::new();
            let mut line: Vec<&str> = Vec::new();
            let indent_len = char_len(indent);
            let mut line_len = prefix.map_or(indent_len, char_len).saturating_sub(1);
            for part in parts {
                if line_len + 1 + char_len(part) > text_width && !line.is_empty() {
                    lines.push(format!("{indent}{}", line.join(" ")));
                    line.clear();
                    line_len = indent_len.saturating_sub(1);
                }
                line.push(part);
                line_len += char_len(part) + 1;
            }
            if !line.is_empty() {
                lines.push(format!("{indent}{}", line.join(" ")));
            }
            if prefix.is_some() {
                if let Some(first) = lines.first_mut() {
                    *first = first.chars().skip(indent_len).collect();
                }
            }
            lines
        };
        let with_prog = |parts: &[String]| -> Vec<String> {
            std::iter::once(prog.to_owned())
                .chain(parts.iter().cloned())
                .collect()
        };
        let prefix_len = char_len(prefix);
        // Python compares with `0.75 * text_width`.
        if (prefix_len + char_len(prog)) as f64 <= 0.75 * text_width as f64 {
            let indent = " ".repeat(prefix_len + char_len(prog) + 1);
            if !opt_parts.is_empty() {
                let mut lines = get_lines(&with_prog(opt_parts), &indent, Some(prefix));
                lines.extend(get_lines(pos_parts, &indent, None));
                lines
            } else if !pos_parts.is_empty() {
                get_lines(&with_prog(pos_parts), &indent, Some(prefix))
            } else {
                vec![prog.to_owned()]
            }
        } else {
            let indent = " ".repeat(prefix_len);
            let all: Vec<String> = opt_parts.iter().chain(pos_parts).cloned().collect();
            let mut lines = get_lines(&all, &indent, None);
            if lines.len() > 1 {
                lines = get_lines(opt_parts, &indent, None);
                lines.extend(get_lines(pos_parts, &indent, None));
            }
            let mut result = vec![prog.to_owned()];
            result.extend(lines);
            result
        }
    }

    fn fill_text(&self, parser: &Parser, text: &str, width: usize, indent: &str) -> String {
        if parser.raw_description {
            return text
                .split_inclusive('\n')
                .map(|line| format!("{indent}{line}"))
                .collect();
        }
        wrap(&collapse_whitespace(text), width, indent, indent).join("\n")
    }

    /// `_format_text`.
    fn text(&self, parser: &Parser, text: &str, current_indent: usize) -> String {
        let width = self.width.saturating_sub(current_indent).max(11);
        let indent = " ".repeat(current_indent);
        format!("{}\n\n", self.fill_text(parser, text, width, &indent))
    }

    /// `_format_action` for an action inside a section (indented by `INDENT`).
    fn action(&self, action: &Action) -> String {
        let help_position = (self.action_max_length + 2).min(self.max_help_position);
        let help_width = self.width.saturating_sub(help_position).max(11);
        let action_width = help_position.saturating_sub(INDENT + 2);
        let header = View(action).invocation();
        let mut out = String::new();
        let mut indent_first = 0;
        if action.help.is_empty() {
            out.push_str(&format!("{:INDENT$}{header}\n", ""));
        } else if char_len(&header) <= action_width {
            let pad = action_width - char_len(&header);
            out.push_str(&format!("{:INDENT$}{header}{}  ", "", " ".repeat(pad)));
        } else {
            out.push_str(&format!("{:INDENT$}{header}\n", ""));
            indent_first = help_position;
        }
        if !action.help.trim().is_empty() {
            let lines = wrap(&collapse_whitespace(action.help), help_width, "", "");
            for (i, line) in lines.iter().enumerate() {
                let indent = if i == 0 { indent_first } else { help_position };
                out.push_str(&format!("{}{line}\n", " ".repeat(indent)));
            }
        } else if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// `parser.format_usage()`.
pub(crate) fn usage(parser: &Parser) -> String {
    Formatter::new().usage(parser)
}

/// `parser.format_help()`.
pub(crate) fn help(parser: &Parser) -> String {
    let mut formatter = Formatter::new();
    // The sections: positionals, optionals, then each titled group in the order it was made.
    let mut sections: Vec<(&str, &str, Vec<&Action>)> = vec![
        ("positional arguments", "", Vec::new()),
        ("options", "", Vec::new()),
    ];
    for action in parser.actions() {
        let index = match action.group {
            Some((title, description)) => match sections.iter().position(|(t, _, _)| *t == title) {
                Some(i) => i,
                None => {
                    sections.push((title, description, Vec::new()));
                    sections.len() - 1
                }
            },
            None if action.option_strings.is_empty() => 0,
            None => 1,
        };
        sections[index].2.push(action);
    }
    for (_, _, actions) in &sections {
        for action in actions {
            let length = char_len(&View(action).invocation()) + INDENT;
            formatter.action_max_length = formatter.action_max_length.max(length);
        }
    }
    let mut out = String::from("\n");
    out.push_str(&format!("{}\n", formatter.usage(parser)));
    if !parser.description.is_empty() {
        out.push_str(&formatter.text(parser, parser.description, 0));
    }
    for (title, description, actions) in &sections {
        let mut items = String::new();
        if !description.is_empty() {
            items.push_str(&formatter.text(parser, description, INDENT));
        }
        for action in actions {
            items.push_str(&formatter.action(action));
        }
        if !items.is_empty() {
            out.push_str(&format!("\n{title}:\n{items}\n"));
        }
    }
    out.push('\n');
    // `_long_break_matcher.sub('\n\n', help)`, then `strip('\n') + '\n'`.
    let mut collapsed = String::with_capacity(out.len());
    let mut newlines = 0;
    for c in out.chars() {
        if c == '\n' {
            newlines += 1;
            if newlines <= 2 {
                collapsed.push(c);
            }
        } else {
            newlines = 0;
            collapsed.push(c);
        }
    }
    format!("{}\n", collapsed.trim_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(text: &str) -> Vec<String> {
        split_chunks(&text.chars().collect::<Vec<_>>())
    }

    #[test]
    fn words_split_after_hyphens_inside_hyphenated_words() {
        assert_eq!(
            chunks("Hello there -- you goof-ball, use the -b option!"),
            [
                "Hello", " ", "there", " ", "--", " ", "you", " ", "goof-", "ball,", " ", "use",
                " ", "the", " ", "-b", " ", "option!"
            ]
        );
        assert_eq!(chunks("'merge-strategy'"), ["'merge-", "strategy'"]);
        assert_eq!(chunks("a-b"), ["a-b"]);
        assert_eq!(chunks("well--known"), ["well", "--", "known"]);
    }

    #[test]
    fn wrapping_follows_textwrap() {
        let text = "the merge strategy to use for inputs (overrides 'merge-strategy' for inputs).";
        assert_eq!(
            wrap(text, 55, "", ""),
            [
                "the merge strategy to use for inputs (overrides 'merge-",
                "strategy' for inputs)."
            ]
        );
        assert_eq!(wrap("", 10, "", ""), Vec::<String>::new());
        assert_eq!(
            wrap("abcdefghij klm", 4, "", ""),
            ["abcd", "efgh", "ij", "klm"]
        );
        assert_eq!(wrap("aa bb cc", 5, "> ", "  "), ["> aa", "  bb", "  cc"]);
    }

    #[test]
    fn long_words_break_after_a_hyphen_when_they_can() {
        assert_eq!(
            wrap("x --option-name-here", 9, "", ""),
            ["x", "--option-", "name-here"]
        );
    }
}
