//! `pygments.lexer.RegexLexer`: states of rules that match at the current position, with token
//! actions, `bygroups`, `using(this)`, `include`, `default`, `words`, `combined` and a stack of
//! states.

use std::collections::HashMap;
use std::sync::Arc;

use super::pyre::{Flags, Regex, Scratch};
use super::regexopt::regex_opt;
use super::token::TokenType;
use crate::prettyprint::PrettyPrintError;

pub(super) type Token<'t> = (TokenType, &'t str);

/// How deep `using` may nest (Python gives up on recursion much earlier).
const MAX_DEPTH: usize = 400;

pub(super) enum Pattern {
    Regex(String),
    /// `words(words, prefix, suffix)`.
    Words {
        words: &'static [&'static str],
        prefix: &'static str,
        suffix: &'static str,
    },
}

/// What `bygroups` does with one group.
#[derive(Clone, Copy)]
pub(super) enum Group {
    Token(TokenType),
    /// `using(this)`, starting in the state named (on top of `root`).
    UsingThis(Option<&'static str>),
}

/// The rule matched: its text, with access to the groups.
pub(super) struct Matched<'a, 't> {
    pub text: &'t str,
    regex: &'a Regex,
    scratch: &'a Scratch,
}

impl<'t> Matched<'_, 't> {
    pub fn group(&self, index: usize) -> Option<&'t str> {
        self.scratch.group(index).map(|(s, e)| &self.text[s..e])
    }

    pub fn named(&self, name: &str) -> Option<&'t str> {
        self.group(self.regex.group_index(name)?)
    }
}

pub(super) type Callback = for<'t> fn(
    &RegexLexer,
    &Matched<'_, 't>,
    usize,
    &mut Vec<Token<'t>>,
) -> Result<(), PrettyPrintError>;

#[derive(Clone)]
pub(super) enum Action {
    Token(TokenType),
    ByGroups(Vec<Option<Group>>),
    Callback(Callback),
    /// `default(...)`: matches nothing and only changes the state.
    None,
}

/// A state transition in a rule definition.
#[derive(Clone)]
pub(super) enum NewState {
    /// `'#pop'` and `'#pop:n'`.
    Pop(usize),
    /// `'#push'`.
    #[allow(dead_code)] // transitions other Pygments lexers use
    Push,
    State(&'static str),
    /// A tuple of states, pushed in turn (`'#pop'` and `'#push'` allowed).
    #[allow(dead_code)]
    Many(&'static [&'static str]),
    /// `combined(...)`: one anonymous state with the rules of all of these.
    Combined(&'static [&'static str]),
}

pub(super) enum RuleDef {
    Include(&'static str),
    Rule {
        pattern: Pattern,
        action: Action,
        new_state: Option<NewState>,
    },
}

pub(super) struct LexerDef {
    pub states: Vec<(&'static str, Vec<RuleDef>)>,
    pub flags: Flags,
}

pub(super) fn rule(pattern: &str, token: TokenType) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Regex(pattern.to_owned()),
        action: Action::Token(token),
        new_state: None,
    }
}

pub(super) fn rule_to(pattern: &str, token: TokenType, new_state: NewState) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Regex(pattern.to_owned()),
        action: Action::Token(token),
        new_state: Some(new_state),
    }
}

pub(super) fn by_groups(pattern: &str, groups: Vec<Option<Group>>) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Regex(pattern.to_owned()),
        action: Action::ByGroups(groups),
        new_state: None,
    }
}

pub(super) fn by_groups_to(
    pattern: &str,
    groups: Vec<Option<Group>>,
    new_state: NewState,
) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Regex(pattern.to_owned()),
        action: Action::ByGroups(groups),
        new_state: Some(new_state),
    }
}

pub(super) fn words_rule(
    words: &'static [&'static str],
    prefix: &'static str,
    suffix: &'static str,
    token: TokenType,
) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Words {
            words,
            prefix,
            suffix,
        },
        action: Action::Token(token),
        new_state: None,
    }
}

pub(super) fn callback(pattern: &str, callback: Callback) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Regex(pattern.to_owned()),
        action: Action::Callback(callback),
        new_state: None,
    }
}

pub(super) fn default(new_state: NewState) -> RuleDef {
    RuleDef::Rule {
        pattern: Pattern::Regex(String::new()),
        action: Action::None,
        new_state: Some(new_state),
    }
}

pub(super) fn token(t: TokenType) -> Option<Group> {
    Some(Group::Token(t))
}

pub(super) fn using_this() -> Option<Group> {
    Some(Group::UsingThis(None))
}

pub(super) fn using_this_in(state: &'static str) -> Option<Group> {
    Some(Group::UsingThis(Some(state)))
}

enum Step {
    Pop,
    Push,
    State(usize),
}

enum Transition {
    /// Pop this many states, keeping at least the first.
    Pop(usize),
    Push,
    Steps(Vec<Step>),
}

struct CompiledRule {
    regex: Regex,
    action: Action,
    transition: Option<Transition>,
}

pub(crate) struct RegexLexer {
    states: Vec<Vec<Arc<CompiledRule>>>,
    ids: HashMap<&'static str, usize>,
}

/// `RegexLexerMeta.process_tokendef`. Declared states keep their position in the definition as
/// their id; the anonymous states of `combined` follow them.
struct Processor<'d> {
    def: &'d LexerDef,
    processed: Vec<Option<Vec<Arc<CompiledRule>>>>,
    anonymous: Vec<Vec<Arc<CompiledRule>>>,
}

impl Processor<'_> {
    fn id(&self, name: &str) -> usize {
        self.def
            .states
            .iter()
            .position(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("unknown state {name:?}"))
    }

    /// `_process_state`: the rules of a state with its `include`s expanded in place.
    fn state(&mut self, name: &str) -> Vec<Arc<CompiledRule>> {
        let at = self.id(name);
        if let Some(done) = &self.processed[at] {
            return done.clone();
        }
        self.processed[at] = Some(Vec::new());
        let mut out = Vec::new();
        for definition in &self.def.states[at].1 {
            match definition {
                RuleDef::Include(other) => {
                    assert!(*other != name, "circular state reference {name:?}");
                    out.extend(self.state(other));
                }
                RuleDef::Rule {
                    pattern,
                    action,
                    new_state,
                } => {
                    let transition = new_state.as_ref().map(|n| self.transition(n));
                    let text = match pattern {
                        Pattern::Regex(text) => text.clone(),
                        Pattern::Words {
                            words,
                            prefix,
                            suffix,
                        } => regex_opt(words, prefix, suffix),
                    };
                    let regex = Regex::new(&text, self.def.flags)
                        .unwrap_or_else(|e| panic!("uncompilable regex {text:?} in {name:?}: {e}"));
                    out.push(Arc::new(CompiledRule {
                        regex,
                        action: action.clone(),
                        transition,
                    }));
                }
            }
        }
        self.processed[at] = Some(out.clone());
        out
    }

    /// `_process_new_state`.
    fn transition(&mut self, new_state: &NewState) -> Transition {
        match new_state {
            NewState::Pop(n) => Transition::Pop(*n),
            NewState::Push => Transition::Push,
            NewState::State(name) => Transition::Steps(vec![Step::State(self.id(name))]),
            NewState::Many(names) => Transition::Steps(
                names
                    .iter()
                    .map(|name| match *name {
                        "#pop" => Step::Pop,
                        "#push" => Step::Push,
                        name => Step::State(self.id(name)),
                    })
                    .collect(),
            ),
            NewState::Combined(names) => {
                let mut rules = Vec::new();
                for name in *names {
                    rules.extend(self.state(name));
                }
                self.anonymous.push(rules);
                Transition::Steps(vec![Step::State(
                    self.def.states.len() + self.anonymous.len() - 1,
                )])
            }
        }
    }
}

impl RegexLexer {
    pub(super) fn new(def: &LexerDef) -> RegexLexer {
        let mut processor = Processor {
            def,
            processed: vec![None; def.states.len()],
            anonymous: Vec::new(),
        };
        for (name, _) in &def.states {
            processor.state(name);
        }
        let mut states: Vec<Vec<Arc<CompiledRule>>> = processor
            .processed
            .into_iter()
            .map(|rules| rules.unwrap_or_default())
            .collect();
        states.extend(processor.anonymous);
        let ids = def
            .states
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (*name, i))
            .collect();
        RegexLexer { states, ids }
    }

    /// `get_tokens_unprocessed(text, stack)`: the tokens of `text`, which is lexed as a whole
    /// from the states in `stack`.
    pub(super) fn tokens<'t>(
        &self,
        text: &'t str,
        stack: &[&str],
        depth: usize,
        out: &mut Vec<Token<'t>>,
    ) -> Result<(), PrettyPrintError> {
        if depth > MAX_DEPTH {
            return Err(PrettyPrintError::Invalid(
                "source is nested too deeply to highlight".into(),
            ));
        }
        let root = self.ids["root"];
        let mut stack: Vec<usize> = stack.iter().map(|name| self.ids[name]).collect();
        let mut scratch = Scratch::default();
        let mut pos = 0;
        while pos < text.len() {
            let rules = &self.states[*stack.last().expect("the stack is never empty")];
            let mut matched = false;
            for rule in rules {
                let Some(end) = rule.regex.match_at(text, pos, &mut scratch) else {
                    continue;
                };
                let view = Matched {
                    text,
                    regex: &rule.regex,
                    scratch: &scratch,
                };
                match &rule.action {
                    Action::None => {}
                    Action::Token(t) => out.push((*t, &text[pos..end])),
                    Action::ByGroups(groups) => {
                        for (i, group) in groups.iter().enumerate() {
                            let Some(group) = group else { continue };
                            let data = view.group(i + 1);
                            match group {
                                Group::Token(t) => {
                                    if let Some(data) = data.filter(|d| !d.is_empty()) {
                                        out.push((*t, data));
                                    }
                                }
                                Group::UsingThis(state) => {
                                    if let Some(data) = data {
                                        let mut stack = vec!["root"];
                                        stack.extend(state);
                                        self.tokens(data, &stack, depth + 1, out)?;
                                    }
                                }
                            }
                        }
                    }
                    Action::Callback(callback) => callback(self, &view, depth, out)?,
                }
                pos = end;
                if let Some(transition) = &rule.transition {
                    apply(transition, &mut stack);
                }
                matched = true;
                break;
            }
            if matched {
                continue;
            }
            // No rule matches: a newline resets the state, anything else is an error token.
            let c = text[pos..].chars().next().expect("pos is inside the text");
            if c == '\n' {
                stack.clear();
                stack.push(root);
                out.push((TokenType::Whitespace, &text[pos..pos + 1]));
            } else {
                out.push((TokenType::Error, &text[pos..pos + c.len_utf8()]));
            }
            pos += c.len_utf8();
        }
        Ok(())
    }
}

fn apply(transition: &Transition, stack: &mut Vec<usize>) {
    match transition {
        Transition::Pop(n) => {
            // Pops, but keeps at least the first state.
            if *n >= stack.len() {
                stack.truncate(1);
            } else {
                stack.truncate(stack.len() - n);
            }
        }
        Transition::Push => stack.push(*stack.last().expect("the stack is never empty")),
        Transition::Steps(steps) => {
            for step in steps {
                match step {
                    Step::Pop => {
                        if stack.len() > 1 {
                            stack.pop();
                        }
                    }
                    Step::Push => stack.push(*stack.last().expect("the stack is never empty")),
                    Step::State(id) => stack.push(*id),
                }
            }
        }
    }
}
