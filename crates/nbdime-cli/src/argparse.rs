//! The part of Python 3.13's `argparse` that the nbdime commands use.
//!
//! Option prefix matching, combined single-dash flags, `--opt=value`, `--`, optional positionals
//! and the left-to-right way positionals are consumed around options follow `argparse`'s
//! algorithm, so the same command lines parse the same way. Help and usage texts are our own.

use std::fmt::Write as _;

use nbdime_core::path::decimal_digit;
use nbdime_core::prettyprint::repr_str;
use nbdime_core::{Map, Value};

/// How many command line strings a positional takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nargs {
    /// Exactly one (`nargs=None`).
    One,
    /// `nargs='?'`.
    Optional,
    /// `nargs='*'`.
    ZeroOrMore,
}

/// What an action does with the strings it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Help,
    Version,
    /// `--config`: list the config keys and exit.
    ConfigHelp,
    /// Stores its value, which must be one of `choices` if there are any.
    Store,
    /// Stores `true` (`store_true`).
    StoreTrue,
    /// Stores `false` (`store_false`).
    StoreFalse,
    /// `IgnorableAction`: `-s`/`--sources` store true, `-S`/`--ignore-sources` store false.
    Ignorable,
    /// A positional argument.
    Positional(Nargs),
}

#[derive(Clone, Debug)]
pub struct Action {
    pub kind: Kind,
    /// Empty for positionals.
    pub option_strings: Vec<String>,
    pub dest: String,
    pub default: Value,
    pub choices: Option<Vec<&'static str>>,
    pub help: &'static str,
}

impl Action {
    fn new(kind: Kind, option_strings: &[&str], dest: &str, default: Value) -> Self {
        Action {
            kind,
            option_strings: option_strings.iter().map(|s| (*s).to_owned()).collect(),
            dest: dest.to_owned(),
            default,
            choices: None,
            help: "",
        }
    }

    pub fn flag(kind: Kind, options: &[&str], dest: &str, default: Value) -> Self {
        Self::new(kind, options, dest, default)
    }

    pub fn store(options: &[&str], dest: &str, default: Value) -> Self {
        Self::new(Kind::Store, options, dest, default)
    }

    /// An `IgnorableAction` for the option strings `options` (`-s`, `--sources`): the positive
    /// and the negative spellings.
    pub fn ignorable(options: &[&str], dest: &str) -> Self {
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        for opt in options {
            if opt.len() == 2 {
                positive.push((*opt).to_owned());
                negative.push(opt.to_uppercase());
            } else {
                positive.push((*opt).to_owned());
                negative.push(format!("--ignore-{}", &opt[2..]));
            }
        }
        positive.extend(negative);
        Action {
            kind: Kind::Ignorable,
            option_strings: positive,
            dest: dest.to_owned(),
            default: Value::Null,
            choices: None,
            help: "",
        }
    }

    pub fn positional(name: &str, nargs: Nargs, default: Value) -> Self {
        Self::new(Kind::Positional(nargs), &[], name, default)
    }

    pub fn choices(mut self, choices: &[&'static str]) -> Self {
        self.choices = Some(choices.to_vec());
        self
    }

    pub fn help(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }

    fn is_positional(&self) -> bool {
        matches!(self.kind, Kind::Positional(_))
    }

    /// Number of option arguments: 0 for flags, 1 for options that take a value.
    fn takes_value(&self) -> bool {
        self.kind == Kind::Store
    }

    fn required(&self) -> bool {
        self.kind == Kind::Positional(Nargs::One)
    }

    fn display_name(&self) -> String {
        if self.is_positional() {
            self.dest.clone()
        } else {
            self.option_strings.join("/")
        }
    }
}

/// Why parsing stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// `parser.error(...)`: usage and message on stderr, exit status 2.
    Usage(String),
    /// An action that ends the program successfully (or, for `--config`, with status 1).
    Exit(ExitRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitRequest {
    Help,
    Version,
    ConfigHelp,
}

/// The parsed command line.
#[derive(Debug, Clone, Default)]
pub struct Namespace {
    pub values: Map,
    /// The value of the last `--log-level` given on the command line.
    pub log_level: Option<String>,
}

impl Namespace {
    pub fn get(&self, dest: &str) -> Option<&Value> {
        self.values.get(dest)
    }

    pub fn truthy(&self, dest: &str) -> bool {
        self.get(dest).is_some_and(Value::truthy)
    }

    /// The string value of `dest`, if it is one.
    pub fn str(&self, dest: &str) -> Option<&str> {
        self.get(dest).and_then(Value::as_str)
    }
}

pub struct Parser {
    pub prog: String,
    pub description: &'static str,
    actions: Vec<Action>,
    /// Defaults set with `set_defaults`; they override those of the actions.
    defaults: Map,
}

/// One interpretation of an argument string that looks like an option.
#[derive(Clone, Debug)]
struct OptionTuple {
    action: Option<usize>,
    option_string: String,
    /// `Some("=")` or `Some("")` (a single-dash flag followed by more characters).
    sep: Option<&'static str>,
    explicit_arg: Option<String>,
}

impl Parser {
    pub fn new(prog: &str, description: &'static str) -> Self {
        let mut parser = Parser {
            prog: prog.to_owned(),
            description,
            actions: Vec::new(),
            defaults: Map::new(),
        };
        parser.add(
            Action::flag(Kind::Help, &["-h", "--help"], "help", Value::Null)
                .help("show this help message and exit"),
        );
        parser
    }

    pub fn add(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// `set_defaults`: also the default of every action that stores to a key given here.
    pub fn set_defaults(&mut self, defaults: Map) {
        for (k, v) in defaults {
            for action in self.actions.iter_mut().filter(|a| a.dest == k) {
                action.default = v.clone();
            }
            self.defaults.insert(k, v);
        }
    }

    fn option_strings(&self) -> impl Iterator<Item = (&str, usize)> {
        self.actions
            .iter()
            .enumerate()
            .flat_map(|(i, a)| a.option_strings.iter().map(move |s| (s.as_str(), i)))
    }

    fn find_option(&self, s: &str) -> Option<usize> {
        self.option_strings().find(|(o, _)| *o == s).map(|(_, i)| i)
    }

    /// `_parse_optional`: `None` if `arg` is a positional.
    fn parse_optional(&self, arg: &str) -> Option<Vec<OptionTuple>> {
        let mut chars = arg.chars();
        let first = chars.next()?;
        if first != '-' {
            return None;
        }
        if let Some(i) = self.find_option(arg) {
            return Some(vec![OptionTuple {
                action: Some(i),
                option_string: arg.to_owned(),
                sep: None,
                explicit_arg: None,
            }]);
        }
        if arg.chars().count() == 1 {
            return None;
        }
        if let Some((option_string, explicit)) = arg.split_once('=') {
            if let Some(i) = self.find_option(option_string) {
                return Some(vec![OptionTuple {
                    action: Some(i),
                    option_string: option_string.to_owned(),
                    sep: Some("="),
                    explicit_arg: Some(explicit.to_owned()),
                }]);
            }
        }
        let tuples = self.option_tuples(arg);
        if !tuples.is_empty() {
            return Some(tuples);
        }
        if is_negative_number(arg) {
            return None;
        }
        if arg.contains(' ') {
            return None;
        }
        Some(vec![OptionTuple {
            action: None,
            option_string: arg.to_owned(),
            sep: None,
            explicit_arg: None,
        }])
    }

    /// `_get_option_tuples`: abbreviations of long options and `-xvalue` forms.
    fn option_tuples(&self, arg: &str) -> Vec<OptionTuple> {
        let mut result = Vec::new();
        let mut chars = arg.chars();
        let (c0, c1) = (chars.next(), chars.next());
        let (prefix, sep, explicit) = match arg.split_once('=') {
            Some((p, e)) => (p, Some("="), Some(e.to_owned())),
            None => (arg, None, None),
        };
        if c0 == Some('-') && c1 == Some('-') {
            for (option_string, i) in self.option_strings() {
                if option_string.starts_with(prefix) {
                    result.push(OptionTuple {
                        action: Some(i),
                        option_string: option_string.to_owned(),
                        sep,
                        explicit_arg: explicit.clone(),
                    });
                }
            }
        } else if c0 == Some('-') {
            let short: String = arg.chars().take(2).collect();
            let short_explicit: String = arg.chars().skip(2).collect();
            for (option_string, i) in self.option_strings() {
                if option_string == short {
                    result.push(OptionTuple {
                        action: Some(i),
                        option_string: option_string.to_owned(),
                        sep: Some(""),
                        explicit_arg: Some(short_explicit.clone()),
                    });
                } else if option_string.starts_with(prefix) {
                    result.push(OptionTuple {
                        action: Some(i),
                        option_string: option_string.to_owned(),
                        sep,
                        explicit_arg: explicit.clone(),
                    });
                }
            }
        }
        result
    }

    /// `parse_args`: unrecognized arguments are an error.
    pub fn parse_args(&self, args: &[String]) -> Result<Namespace, ParseError> {
        let (ns, extras) = self.parse_known_args(args)?;
        if !extras.is_empty() {
            return Err(ParseError::Usage(format!(
                "unrecognized arguments: {}",
                extras.join(" ")
            )));
        }
        Ok(ns)
    }

    pub fn parse_known_args(
        &self,
        arg_strings: &[String],
    ) -> Result<(Namespace, Vec<String>), ParseError> {
        let mut ns = Namespace::default();
        for action in &self.actions {
            let stores = !matches!(action.kind, Kind::Help | Kind::Version | Kind::ConfigHelp);
            if stores && !ns.values.contains_key(&action.dest) {
                ns.values
                    .insert(action.dest.clone(), action.default.clone());
            }
        }
        for (dest, value) in &self.defaults {
            ns.values.insert(dest.clone(), value.clone());
        }
        let mut state = ParseState::new(self, arg_strings, ns);
        state.run()?;
        Ok((state.ns, state.extras))
    }

    fn usage_text(&self) -> String {
        let mut parts = vec![format!("usage: {}", self.prog)];
        for a in self.actions.iter().filter(|a| !a.is_positional()) {
            let first = &a.option_strings[0];
            parts.push(if a.takes_value() {
                format!("[{first} {}]", a.dest.to_uppercase())
            } else {
                format!("[{first}]")
            });
        }
        for a in self.actions.iter().filter(|a| a.is_positional()) {
            parts.push(match a.kind {
                Kind::Positional(Nargs::Optional) => format!("[{}]", a.dest),
                Kind::Positional(Nargs::ZeroOrMore) => format!("[{} ...]", a.dest),
                _ => a.dest.clone(),
            });
        }
        parts.join(" ")
    }

    /// The text `parser.error(message)` prints on stderr.
    pub fn error_text(&self, message: &str) -> String {
        format!("{}\n{}: error: {message}\n", self.usage_text(), self.prog)
    }

    pub fn help_text(&self) -> String {
        let mut out = self.usage_text();
        out.push_str("\n\n");
        out.push_str(self.description);
        out.push_str("\n\n");
        for (title, positional) in [("positional arguments", true), ("options", false)] {
            let _ = writeln!(out, "{title}:");
            for a in self
                .actions
                .iter()
                .filter(|a| a.is_positional() == positional)
            {
                let _ = writeln!(out, "  {}  {}", a.display_name(), a.help);
            }
            out.push('\n');
        }
        out
    }
}

fn is_negative_number(arg: &str) -> bool {
    // `^-\d+$|^-\d*\.\d+$`
    let Some(rest) = arg.strip_prefix('-') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| decimal_digit(c).is_some());
    if digits(rest) {
        return true;
    }
    match rest.split_once('.') {
        Some((int, frac)) => (int.is_empty() || digits(int)) && digits(frac),
        None => false,
    }
}

struct ParseState<'a> {
    parser: &'a Parser,
    args: &'a [String],
    /// 'O' option, 'A' argument, '-' for the first `--`.
    pattern: Vec<u8>,
    option_indices: Vec<(usize, Vec<OptionTuple>)>,
    ns: Namespace,
    extras: Vec<String>,
    seen: Vec<bool>,
    /// Positionals not consumed yet (indices into `parser.actions`).
    positionals: Vec<usize>,
}

impl<'a> ParseState<'a> {
    fn new(parser: &'a Parser, args: &'a [String], ns: Namespace) -> Self {
        let mut pattern = Vec::with_capacity(args.len());
        let mut option_indices = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--" {
                pattern.push(b'-');
                pattern.extend(std::iter::repeat_n(b'A', args.len() - i - 1));
                break;
            }
            match parser.parse_optional(&args[i]) {
                None => pattern.push(b'A'),
                Some(tuples) => {
                    option_indices.push((i, tuples));
                    pattern.push(b'O');
                }
            }
            i += 1;
        }
        let positionals = parser
            .actions
            .iter()
            .enumerate()
            .filter(|(_, a)| a.is_positional())
            .map(|(i, _)| i)
            .collect();
        ParseState {
            parser,
            args,
            pattern,
            option_indices,
            ns,
            extras: Vec::new(),
            seen: vec![false; parser.actions.len()],
            positionals,
        }
    }

    fn option_at(&self, index: usize) -> Option<&Vec<OptionTuple>> {
        self.option_indices
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, t)| t)
    }

    fn run(&mut self) -> Result<(), ParseError> {
        let max_option = self.option_indices.last().map(|(i, _)| *i);
        let mut start = 0;
        if let Some(max_option) = max_option {
            while start <= max_option {
                let mut next_option = start;
                while next_option <= max_option && self.option_at(next_option).is_none() {
                    next_option += 1;
                }
                if start != next_option {
                    let end = self.consume_positionals(start)?;
                    if end > start {
                        start = end;
                        continue;
                    }
                    start = end;
                }
                if self.option_at(start).is_none() {
                    self.extras
                        .extend_from_slice(&self.args[start..next_option]);
                    start = next_option;
                }
                start = self.consume_optional(start)?;
            }
        }
        let stop = self.consume_positionals(start)?;
        self.extras.extend_from_slice(&self.args[stop..]);

        let missing: Vec<String> = self
            .parser
            .actions
            .iter()
            .enumerate()
            .filter(|(i, a)| !self.seen[*i] && a.required())
            .map(|(_, a)| a.display_name())
            .collect();
        if !missing.is_empty() {
            return Err(ParseError::Usage(format!(
                "the following arguments are required: {}",
                missing.join(", ")
            )));
        }
        Ok(())
    }

    fn argument_error(&self, action: Option<usize>, message: &str) -> ParseError {
        match action {
            Some(i) => ParseError::Usage(format!(
                "argument {}: {message}",
                self.parser.actions[i].display_name()
            )),
            None => ParseError::Usage(message.to_owned()),
        }
    }

    /// `_match_argument` for an option's arguments: how many of the strings that follow it.
    fn match_option_args(&self, action: usize, pattern: &[u8]) -> Result<usize, ParseError> {
        if !self.parser.actions[action].takes_value() {
            return Ok(0);
        }
        if pattern.first() == Some(&b'A') {
            Ok(1)
        } else {
            Err(self.argument_error(Some(action), "expected one argument"))
        }
    }

    fn consume_optional(&mut self, start_index: usize) -> Result<usize, ParseError> {
        let tuples = self.option_at(start_index).cloned().unwrap_or_default();
        if tuples.len() > 1 {
            let options: Vec<&str> = tuples.iter().map(|t| t.option_string.as_str()).collect();
            return Err(ParseError::Usage(format!(
                "ambiguous option: {} could match {}",
                self.args[start_index],
                options.join(", ")
            )));
        }
        let OptionTuple {
            mut action,
            mut option_string,
            mut sep,
            mut explicit_arg,
        } = tuples[0].clone();
        let mut action_tuples: Vec<(usize, Vec<String>, String)> = Vec::new();
        let stop;
        loop {
            let Some(act) = action else {
                self.extras.push(self.args[start_index].clone());
                return Ok(start_index + 1);
            };
            if let Some(explicit) = explicit_arg.clone() {
                let arg_count = self.match_option_args(act, b"A")?;
                let second_is_prefix = option_string.chars().nth(1).is_some_and(|c| c != '-');
                if arg_count == 0 && second_is_prefix && !explicit.is_empty() {
                    if sep == Some("=") || explicit.starts_with('-') {
                        return Err(self.argument_error(
                            Some(act),
                            &format!("ignored explicit argument {}", repr_str(&explicit)),
                        ));
                    }
                    action_tuples.push((act, Vec::new(), option_string.clone()));
                    let next_char = explicit.chars().next().unwrap_or('-');
                    option_string = format!("-{next_char}");
                    if let Some(next) = self.parser.find_option(&option_string) {
                        action = Some(next);
                        let rest: String = explicit.chars().skip(1).collect();
                        if rest.is_empty() {
                            sep = None;
                            explicit_arg = None;
                        } else if let Some(after) = rest.strip_prefix('=') {
                            sep = Some("=");
                            explicit_arg = Some(after.to_owned());
                        } else {
                            sep = Some("");
                            explicit_arg = Some(rest);
                        }
                    } else {
                        self.extras.push(format!("-{explicit}"));
                        stop = start_index + 1;
                        break;
                    }
                } else if arg_count == 1 {
                    stop = start_index + 1;
                    action_tuples.push((act, vec![explicit], option_string.clone()));
                    break;
                } else {
                    return Err(self.argument_error(
                        Some(act),
                        &format!("ignored explicit argument {}", repr_str(&explicit)),
                    ));
                }
            } else {
                let start = start_index + 1;
                let arg_count = self.match_option_args(act, &self.pattern[start..])?;
                stop = start + arg_count;
                action_tuples.push((act, self.args[start..stop].to_vec(), option_string.clone()));
                break;
            }
        }
        for (act, args, option_string) in action_tuples {
            self.take_action(act, &args, Some(&option_string))?;
        }
        Ok(stop)
    }

    fn consume_positionals(&mut self, start_index: usize) -> Result<usize, ParseError> {
        let kinds: Vec<Nargs> = self
            .positionals
            .iter()
            .map(|&i| match self.parser.actions[i].kind {
                Kind::Positional(n) => n,
                _ => unreachable!("only positionals are in the list"),
            })
            .collect();
        let counts = match_arguments_partial(&kinds, &self.pattern[start_index..]);
        let mut index = start_index;
        for (n, &count) in counts.iter().enumerate() {
            let action = self.positionals[n];
            let mut args = self.args[index..index + count].to_vec();
            if self.pattern[index..index + count].contains(&b'-') {
                if let Some(pos) = args.iter().position(|a| a == "--") {
                    args.remove(pos);
                }
            }
            index += count;
            self.take_action(action, &args, None)?;
        }
        self.positionals.drain(..counts.len());
        Ok(index)
    }

    fn take_action(
        &mut self,
        index: usize,
        args: &[String],
        option_string: Option<&str>,
    ) -> Result<(), ParseError> {
        self.seen[index] = true;
        let action = self.parser.actions[index].clone();
        match action.kind {
            Kind::Help => Err(ParseError::Exit(ExitRequest::Help)),
            Kind::Version => Err(ParseError::Exit(ExitRequest::Version)),
            Kind::ConfigHelp => Err(ParseError::Exit(ExitRequest::ConfigHelp)),
            Kind::StoreTrue => {
                self.ns.values.insert(action.dest, Value::Bool(true));
                Ok(())
            }
            Kind::StoreFalse => {
                self.ns.values.insert(action.dest, Value::Bool(false));
                Ok(())
            }
            Kind::Ignorable => {
                let option_string = option_string.unwrap_or_default();
                let positive = if option_string.len() == 2 {
                    option_string.chars().nth(1).is_some_and(char::is_lowercase)
                } else {
                    !option_string[2..].starts_with("ignore")
                };
                self.ns.values.insert(action.dest, Value::Bool(positive));
                Ok(())
            }
            Kind::Store => {
                let value = args.first().cloned().unwrap_or_default();
                self.check_choice(index, &action, &value)?;
                if action.dest == "log_level" {
                    self.ns.log_level = Some(value.clone());
                }
                self.ns.values.insert(action.dest, Value::Str(value));
                Ok(())
            }
            Kind::Positional(nargs) => {
                let value = match (nargs, args) {
                    (Nargs::One, [one]) | (Nargs::Optional, [one]) => Value::Str(one.clone()),
                    (Nargs::Optional, []) => action.default.clone(),
                    (Nargs::ZeroOrMore, []) => {
                        if action.default == Value::Null {
                            Value::List(Vec::new())
                        } else {
                            action.default.clone()
                        }
                    }
                    (_, many) => Value::List(many.iter().cloned().map(Value::Str).collect()),
                };
                self.ns.values.insert(action.dest, value);
                Ok(())
            }
        }
    }

    fn check_choice(&self, index: usize, action: &Action, value: &str) -> Result<(), ParseError> {
        if let Some(choices) = &action.choices {
            if !choices.contains(&value) {
                let listed: Vec<String> = choices.iter().map(|c| repr_str(c)).collect();
                return Err(self.argument_error(
                    Some(index),
                    &format!(
                        "invalid choice: {} (choose from {})",
                        repr_str(value),
                        listed.join(", ")
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// `_match_arguments_partial`: how many strings each leading positional takes, for the longest
/// prefix of `kinds` whose patterns match the start of `pattern`.
fn match_arguments_partial(kinds: &[Nargs], pattern: &[u8]) -> Vec<usize> {
    for n in (1..=kinds.len()).rev() {
        if let Some(mut result) = match_items(&kinds[..n], pattern, 0) {
            let end: usize = result.iter().sum();
            if end < pattern.len() && pattern[end] == b'O' {
                while result.last() == Some(&0) {
                    result.pop();
                }
            }
            return result;
        }
    }
    Vec::new()
}

/// Matches the regular expressions `(-*A-*)`, `(-*A?-*)` and `(-*[A-]*)` for `items`, in
/// sequence, at the start of `pattern[pos..]`, with `re.match`'s backtracking order. Returns the
/// length each item took.
fn match_items(items: &[Nargs], pattern: &[u8], pos: usize) -> Option<Vec<usize>> {
    let Some((first, rest)) = items.split_first() else {
        return Some(Vec::new());
    };
    for end in candidate_ends(*first, pattern, pos) {
        if let Some(mut tail) = match_items(rest, pattern, end) {
            tail.insert(0, end - pos);
            return Some(tail);
        }
    }
    None
}

/// The positions where an item that starts at `pos` can end, in the order a greedy regular
/// expression tries them.
fn candidate_ends(nargs: Nargs, pattern: &[u8], pos: usize) -> Vec<usize> {
    let run_of = |from: usize, accept: &dyn Fn(u8) -> bool| {
        pattern[from.min(pattern.len())..]
            .iter()
            .take_while(|c| accept(**c))
            .count()
    };
    let is_dash = |c: u8| c == b'-';
    let mut ends = Vec::new();
    match nargs {
        Nargs::ZeroOrMore => {
            let run = run_of(pos, &|c| c == b'A' || c == b'-');
            ends.extend((pos..=pos + run).rev());
        }
        Nargs::One | Nargs::Optional => {
            let lead = run_of(pos, &is_dash);
            let after_lead = pos + lead;
            let mut bodies = Vec::new();
            if pattern.get(after_lead) == Some(&b'A') {
                bodies.push(after_lead + 1);
            }
            if nargs == Nargs::Optional {
                bodies.push(after_lead);
            }
            for body_end in bodies {
                let trail = run_of(body_end, &is_dash);
                ends.extend((body_end..=body_end + trail).rev());
            }
        }
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn diff_like() -> Parser {
        let mut p = Parser::new("nbdiff", "diff");
        p.add(Action::ignorable(&["-s", "--sources"], "sources"));
        p.add(Action::ignorable(&["-o", "--outputs"], "outputs"));
        p.add(Action::flag(
            Kind::StoreFalse,
            &["--no-color"],
            "use_color",
            Value::Bool(true),
        ));
        p.add(Action::flag(
            Kind::StoreFalse,
            &["--no-git"],
            "use_git",
            Value::Bool(true),
        ));
        p.add(
            Action::store(&["--log-level"], "log_level", Value::from("INFO"))
                .choices(&["DEBUG", "INFO", "WARN", "ERROR", "CRITICAL"]),
        );
        p.add(Action::positional(
            "base",
            Nargs::Optional,
            Value::from("HEAD"),
        ));
        p.add(Action::positional("remote", Nargs::Optional, Value::Null));
        p.add(Action::positional("paths", Nargs::ZeroOrMore, Value::Null));
        p.add(Action::store(&["--out"], "out", Value::Null));
        p
    }

    fn merge_like() -> Parser {
        let mut p = Parser::new("nbmerge", "merge");
        p.add(Action::positional(
            "base",
            Nargs::Optional,
            Value::from("/dev/null"),
        ));
        p.add(Action::positional("local", Nargs::One, Value::Null));
        p.add(Action::positional("remote", Nargs::One, Value::Null));
        p.add(Action::flag(
            Kind::StoreTrue,
            &["--decisions"],
            "decisions",
            Value::Bool(false),
        ));
        p
    }

    fn parse(p: &Parser, list: &[&str]) -> Result<Namespace, ParseError> {
        p.parse_args(&args(list))
    }

    #[test]
    fn positionals_and_defaults() {
        let ns = parse(&diff_like(), &["a", "b"]).unwrap();
        assert_eq!(ns.str("base"), Some("a"));
        assert_eq!(ns.str("remote"), Some("b"));
        assert_eq!(ns.get("paths"), Some(&Value::List(vec![])));
        let ns = parse(&diff_like(), &[]).unwrap();
        assert_eq!(ns.str("base"), Some("HEAD"));
        assert_eq!(ns.get("remote"), Some(&Value::Null));
        let ns = parse(&diff_like(), &["a", "b", "c", "d"]).unwrap();
        assert_eq!(
            ns.get("paths"),
            Some(&Value::List(vec![Value::from("c"), Value::from("d")]))
        );
    }

    #[test]
    fn positionals_around_options() {
        let ns = parse(&diff_like(), &["a", "--no-color", "b"]).unwrap();
        assert_eq!(ns.str("base"), Some("a"));
        assert_eq!(ns.str("remote"), Some("b"));
        assert!(!ns.truthy("use_color"));
        let ns = parse(&diff_like(), &["--out", "x", "a"]).unwrap();
        assert_eq!(ns.str("out"), Some("x"));
        assert_eq!(ns.str("base"), Some("a"));
    }

    #[test]
    fn merge_positionals_backtrack() {
        let ns = parse(&merge_like(), &["l", "r"]).unwrap();
        assert_eq!(ns.str("base"), Some("/dev/null"));
        assert_eq!(ns.str("local"), Some("l"));
        let ns = parse(&merge_like(), &["b", "l", "r"]).unwrap();
        assert_eq!(ns.str("base"), Some("b"));
        assert_eq!(ns.str("remote"), Some("r"));
        assert!(matches!(
            parse(&merge_like(), &["only"]),
            Err(ParseError::Usage(m)) if m.contains("required")
        ));
    }

    #[test]
    fn prefixes_and_combined_short_flags() {
        let ns = parse(&diff_like(), &["--no-col", "-so", "a", "b"]).unwrap();
        assert!(!ns.truthy("use_color"));
        assert_eq!(ns.get("sources"), Some(&Value::Bool(true)));
        assert_eq!(ns.get("outputs"), Some(&Value::Bool(true)));
        let ns = parse(&diff_like(), &["-sO", "a"]).unwrap();
        assert_eq!(ns.get("sources"), Some(&Value::Bool(true)));
        assert_eq!(ns.get("outputs"), Some(&Value::Bool(false)));
        let ns = parse(&diff_like(), &["--ignore-sources", "--outputs", "a"]).unwrap();
        assert_eq!(ns.get("sources"), Some(&Value::Bool(false)));
        assert_eq!(ns.get("outputs"), Some(&Value::Bool(true)));
    }

    #[test]
    fn ambiguous_and_unknown_options() {
        assert!(matches!(
            parse(&diff_like(), &["--no", "a"]),
            Err(ParseError::Usage(m)) if m.contains("ambiguous")
        ));
        assert!(matches!(
            parse(&diff_like(), &["--bogus", "a"]),
            Err(ParseError::Usage(m)) if m.contains("unrecognized")
        ));
        assert!(matches!(
            parse(&diff_like(), &["-sx", "a"]),
            Err(ParseError::Usage(m)) if m.contains("-x")
        ));
        assert!(matches!(
            parse(&diff_like(), &["--no-color=1", "a"]),
            Err(ParseError::Usage(m)) if m.contains("ignored explicit argument")
        ));
    }

    #[test]
    fn option_values_and_choices() {
        let ns = parse(&diff_like(), &["--out=file", "--log-level", "WARN", "a"]).unwrap();
        assert_eq!(ns.str("out"), Some("file"));
        assert_eq!(ns.log_level.as_deref(), Some("WARN"));
        assert!(matches!(
            parse(&diff_like(), &["--log-level", "BOGUS"]),
            Err(ParseError::Usage(m)) if m.contains("invalid choice")
        ));
        assert!(matches!(
            parse(&diff_like(), &["--out"]),
            Err(ParseError::Usage(m)) if m.contains("expected one argument")
        ));
        assert!(matches!(
            parse(&diff_like(), &["--out", "--", "a"]),
            Err(ParseError::Usage(m)) if m.contains("expected one argument")
        ));
        let ns = parse(&diff_like(), &["--out=", "a"]).unwrap();
        assert_eq!(ns.str("out"), Some(""));
    }

    #[test]
    fn double_dash_ends_options() {
        let ns = parse(&diff_like(), &["--", "--no-color", "b"]).unwrap();
        assert_eq!(ns.str("base"), Some("--no-color"));
        assert_eq!(ns.str("remote"), Some("b"));
        assert!(ns.truthy("use_color"));
        let ns = parse(&diff_like(), &["a", "--", "b", "c"]).unwrap();
        assert_eq!(ns.str("remote"), Some("b"));
    }

    #[test]
    fn negative_numbers_are_positionals() {
        let ns = parse(&diff_like(), &["-1", "-2.5"]).unwrap();
        assert_eq!(ns.str("base"), Some("-1"));
        assert_eq!(ns.str("remote"), Some("-2.5"));
        assert!(is_negative_number("-.5") && !is_negative_number("-a") && !is_negative_number("-"));
    }

    #[test]
    fn help_requests_exit_in_order() {
        assert_eq!(
            parse(&diff_like(), &["-h"]).unwrap_err(),
            ParseError::Exit(ExitRequest::Help)
        );
        assert!(matches!(
            parse(&diff_like(), &["--log-level", "X", "-h"]),
            Err(ParseError::Usage(_))
        ));
    }

    #[test]
    fn defaults_from_config_override() {
        let mut p = diff_like();
        let mut defaults = Map::new();
        defaults.insert("sources".into(), Value::Bool(false));
        defaults.insert("unknown".into(), Value::from(1));
        p.set_defaults(defaults);
        let ns = parse(&p, &["a"]).unwrap();
        assert_eq!(ns.get("sources"), Some(&Value::Bool(false)));
        assert_eq!(ns.get("unknown"), Some(&Value::from(1)));
    }

    #[test]
    fn partial_matching_of_positional_patterns() {
        use Nargs::*;
        assert_eq!(
            match_arguments_partial(&[Optional, Optional, ZeroOrMore], b"AOA"),
            vec![1]
        );
        assert_eq!(
            match_arguments_partial(&[Optional, One, One], b"AA"),
            vec![0, 1, 1]
        );
        assert_eq!(
            match_arguments_partial(&[Optional, Optional, ZeroOrMore], b"-AA"),
            vec![2, 1, 0]
        );
        assert_eq!(match_arguments_partial(&[One], b""), Vec::<usize>::new());
    }
}
