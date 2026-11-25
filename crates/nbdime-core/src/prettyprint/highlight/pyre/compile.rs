//! Compilation of a parsed pattern into a program for the backtracking matcher.

use super::class::CharClass;
use super::parse::{Assertion, Node};
use super::PatternError;

/// An instruction that consumes exactly one character.
#[derive(Clone, Copy, Debug)]
pub(super) enum One {
    Char(char),
    /// A character compared by its lowercase form; the stored character is already lowercase.
    CharFolded(char),
    Any,
    AnyButNewline,
    Class(usize),
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Inst {
    One(One),
    /// `min` to `max` repetitions of a one-character matcher.
    Repeat {
        one: One,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
    Assert(Assertion),
    Save(usize),
    /// Continue at the first target, and at the second if that fails.
    Split(usize, usize),
    Jump(usize),
    /// Records the position in a mark slot, to tell empty loop iterations from progress.
    Mark(usize),
    /// Ends an iteration of a loop whose body may match nothing: loop again only if the
    /// iteration consumed something.
    LoopBack {
        mark: usize,
        target: usize,
    },
    /// A lookaround whose subprogram starts at the next instruction and ends at a `LookEnd`
    /// before `end`. `width` is the length in characters a lookbehind steps back.
    Look {
        ahead: bool,
        negative: bool,
        width: usize,
        end: usize,
    },
    LookEnd,
    Backref(usize),
    Match,
}

pub(super) struct Program {
    pub insts: Vec<Inst>,
    pub classes: Vec<CharClass>,
    /// Capture slots (two per group, group 0 included) followed by the mark slots.
    pub slots: usize,
}

pub(super) fn compile(node: &Node, groups: usize) -> Result<Program, PatternError> {
    let mut compiler = Compiler {
        insts: Vec::new(),
        classes: Vec::new(),
        slots: 2 * (groups + 1),
    };
    compiler.emit(node)?;
    compiler.insts.push(Inst::Match);
    Ok(Program {
        insts: compiler.insts,
        classes: compiler.classes,
        slots: compiler.slots,
    })
}

struct Compiler {
    insts: Vec<Inst>,
    classes: Vec<CharClass>,
    slots: usize,
}

/// Whether `node` can match without consuming anything.
fn nullable(node: &Node) -> bool {
    match node {
        Node::Empty | Node::Assert(_) | Node::Look { .. } | Node::Backref(_) => true,
        Node::Char { .. } | Node::Any { .. } | Node::Class(_) => false,
        Node::Group { node, .. } => nullable(node),
        Node::Concat(items) => items.iter().all(nullable),
        Node::Alt(items) => items.iter().any(nullable),
        Node::Repeat { node, min, .. } => *min == 0 || nullable(node),
    }
}

/// The number of characters `node` always matches, if that is a single number.
fn width(node: &Node) -> Option<usize> {
    match node {
        Node::Empty | Node::Assert(_) | Node::Look { .. } => Some(0),
        Node::Char { .. } | Node::Any { .. } | Node::Class(_) => Some(1),
        Node::Backref(_) => None,
        Node::Group { node, .. } => width(node),
        Node::Concat(items) => items.iter().map(width).sum(),
        Node::Alt(items) => {
            let first = width(items.first()?)?;
            items
                .iter()
                .all(|i| width(i) == Some(first))
                .then_some(first)
        }
        Node::Repeat { node, min, max, .. } => (Some(*min) == *max)
            .then(|| width(node).map(|w| w * *min as usize))
            .flatten(),
    }
}

impl Compiler {
    fn next(&self) -> usize {
        self.insts.len()
    }

    fn one(&mut self, node: &Node) -> Option<One> {
        Some(match node {
            Node::Char { c, ignore_case } if *ignore_case => One::CharFolded(*c),
            Node::Char { c, .. } => One::Char(*c),
            Node::Any { dot_all: true } => One::Any,
            Node::Any { dot_all: false } => One::AnyButNewline,
            Node::Class(class) => {
                self.classes.push(class.clone());
                One::Class(self.classes.len() - 1)
            }
            _ => return None,
        })
    }

    fn emit(&mut self, node: &Node) -> Result<(), PatternError> {
        match node {
            Node::Empty => {}
            Node::Char { .. } | Node::Any { .. } | Node::Class(_) => {
                let one = self.one(node).expect("a one-character node");
                self.insts.push(Inst::One(one));
            }
            Node::Assert(assertion) => self.insts.push(Inst::Assert(*assertion)),
            Node::Group { index, node } => match index {
                Some(i) => {
                    self.insts.push(Inst::Save(2 * i));
                    self.emit(node)?;
                    self.insts.push(Inst::Save(2 * i + 1));
                }
                None => self.emit(node)?,
            },
            Node::Concat(items) => {
                for item in items {
                    self.emit(item)?;
                }
            }
            Node::Alt(alternatives) => self.alternation(alternatives)?,
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.repeat(node, *min, *max, *greedy)?,
            Node::Look {
                ahead,
                negative,
                node,
            } => {
                let width = if *ahead {
                    0
                } else {
                    width(node).ok_or_else(|| {
                        PatternError("look-behind requires fixed-width pattern".into())
                    })?
                };
                let at = self.next();
                self.insts.push(Inst::LookEnd);
                self.emit(node)?;
                self.insts.push(Inst::LookEnd);
                self.insts[at] = Inst::Look {
                    ahead: *ahead,
                    negative: *negative,
                    width,
                    end: self.next(),
                };
            }
            Node::Backref(group) => self.insts.push(Inst::Backref(*group)),
        }
        Ok(())
    }

    fn alternation(&mut self, alternatives: &[Node]) -> Result<(), PatternError> {
        let mut jumps = Vec::new();
        for (i, alternative) in alternatives.iter().enumerate() {
            if i + 1 < alternatives.len() {
                let split = self.next();
                self.insts.push(Inst::Split(split + 1, 0));
                self.emit(alternative)?;
                jumps.push(self.next());
                self.insts.push(Inst::Jump(0));
                let after = self.next();
                self.insts[split] = Inst::Split(split + 1, after);
            } else {
                self.emit(alternative)?;
            }
        }
        let end = self.next();
        for jump in jumps {
            self.insts[jump] = Inst::Jump(end);
        }
        Ok(())
    }

    fn repeat(
        &mut self,
        node: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    ) -> Result<(), PatternError> {
        if let Some(one) = self.one(node) {
            self.insts.push(Inst::Repeat {
                one,
                min,
                max,
                greedy,
            });
            return Ok(());
        }
        for _ in 0..min {
            self.emit(node)?;
        }
        match max {
            None => self.star(node, greedy)?,
            Some(max) => {
                let mut splits = Vec::new();
                for _ in min..max {
                    splits.push(self.next());
                    self.insts.push(Inst::Split(0, 0));
                    self.emit(node)?;
                }
                let end = self.next();
                for split in splits {
                    self.insts[split] = if greedy {
                        Inst::Split(split + 1, end)
                    } else {
                        Inst::Split(end, split + 1)
                    };
                }
            }
        }
        Ok(())
    }

    fn star(&mut self, node: &Node, greedy: bool) -> Result<(), PatternError> {
        let head = self.next();
        self.insts.push(Inst::Split(0, 0));
        let guarded = nullable(node);
        let mark = self.slots;
        if guarded {
            self.slots += 1;
            self.insts.push(Inst::Mark(mark));
        }
        self.emit(node)?;
        if guarded {
            self.insts.push(Inst::LoopBack { mark, target: head });
        } else {
            self.insts.push(Inst::Jump(head));
        }
        let end = self.next();
        self.insts[head] = if greedy {
            Inst::Split(head + 1, end)
        } else {
            Inst::Split(end, head + 1)
        };
        Ok(())
    }
}
