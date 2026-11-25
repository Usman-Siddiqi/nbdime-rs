//! The backtracking matcher. It runs with an explicit stack, so the work it can undo grows the
//! heap and never the call stack; only lookarounds recurse, one level per nested lookaround in
//! the pattern.

use super::class::{fold, is_word};
use super::compile::{Inst, One, Program};
use super::parse::Assertion;

pub(super) const UNSET: usize = usize::MAX;

pub(super) enum Backtrack {
    Branch {
        pc: usize,
        pos: usize,
    },
    Restore {
        slot: usize,
        old: usize,
    },
    /// A greedy repetition gives back characters one at a time down to `min_pos`.
    Greedy {
        next: usize,
        min_pos: usize,
        pos: usize,
    },
    /// A lazy repetition takes one more character at `pos`; `count` have been taken.
    Lazy {
        pc: usize,
        pos: usize,
        count: u32,
    },
}

fn char_at(text: &str, pos: usize) -> Option<char> {
    text[pos..].chars().next()
}

fn char_before(text: &str, pos: usize) -> Option<char> {
    text[..pos].chars().next_back()
}

fn word_before(text: &str, pos: usize) -> bool {
    char_before(text, pos).is_some_and(is_word)
}

fn word_at(text: &str, pos: usize) -> bool {
    char_at(text, pos).is_some_and(is_word)
}

fn assertion_holds(assertion: Assertion, text: &str, pos: usize) -> bool {
    let bytes = text.as_bytes();
    match assertion {
        Assertion::LineStart => pos == 0 || bytes[pos - 1] == b'\n',
        Assertion::LineEnd => pos == text.len() || bytes[pos] == b'\n',
        Assertion::TextStart => pos == 0,
        Assertion::TextEnd => pos == text.len(),
        Assertion::TextEndOrFinalNewline => {
            pos == text.len() || (pos + 1 == text.len() && bytes[pos] == b'\n')
        }
        Assertion::WordBoundary => word_before(text, pos) != word_at(text, pos),
        // Python 3.13's \B requires a nonempty subject.
        Assertion::NotWordBoundary => {
            !text.is_empty() && word_before(text, pos) == word_at(text, pos)
        }
    }
}

impl Program {
    /// The position after the character `one` matches at `pos`.
    fn step(&self, one: One, text: &str, pos: usize) -> Option<usize> {
        let c = char_at(text, pos)?;
        let hit = match one {
            One::Char(expected) => c == expected,
            One::CharFolded(expected) => c == expected || fold(c) == expected,
            One::Any => true,
            One::AnyButNewline => c != '\n',
            One::Class(index) => self.classes[index].matches(c),
        };
        hit.then(|| pos + c.len_utf8())
    }

    /// Runs the program from `pc` at `pos`; returns the end of the match. `slots` hold the
    /// captures of the match on success and are unchanged on failure.
    pub(super) fn run(
        &self,
        text: &str,
        mut pc: usize,
        mut pos: usize,
        slots: &mut [usize],
        stack: &mut Vec<Backtrack>,
    ) -> Option<usize> {
        stack.clear();
        loop {
            let proceed = match self.insts[pc] {
                Inst::One(one) => match self.step(one, text, pos) {
                    Some(next) => {
                        pos = next;
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Inst::Repeat {
                    one,
                    min,
                    max,
                    greedy,
                } => {
                    let taken = self.repeat(one, min, max, greedy, text, pc, &mut pos, stack);
                    pc += taken as usize;
                    taken
                }
                Inst::Assert(assertion) => {
                    let holds = assertion_holds(assertion, text, pos);
                    pc += holds as usize;
                    holds
                }
                Inst::Save(slot) | Inst::Mark(slot) => {
                    stack.push(Backtrack::Restore {
                        slot,
                        old: slots[slot],
                    });
                    slots[slot] = pos;
                    pc += 1;
                    true
                }
                Inst::Split(first, second) => {
                    stack.push(Backtrack::Branch { pc: second, pos });
                    pc = first;
                    true
                }
                Inst::Jump(target) => {
                    pc = target;
                    true
                }
                Inst::LoopBack { mark, target } => {
                    pc = if slots[mark] == pos { pc + 1 } else { target };
                    true
                }
                Inst::Look {
                    ahead,
                    negative,
                    width,
                    end,
                } => {
                    if self.look(ahead, negative, width, text, pc, pos, slots, stack) {
                        pc = end;
                        true
                    } else {
                        false
                    }
                }
                Inst::LookEnd | Inst::Match => return Some(pos),
                Inst::Backref(group) => {
                    let (start, stop) = (slots[2 * group], slots[2 * group + 1]);
                    let captured = (start != UNSET && stop != UNSET).then(|| &text[start..stop]);
                    match captured {
                        Some(captured) if text[pos..].starts_with(captured) => {
                            pos += captured.len();
                            pc += 1;
                            true
                        }
                        _ => false,
                    }
                }
            };
            if proceed {
                continue;
            }
            loop {
                match stack.pop() {
                    None => return None,
                    Some(Backtrack::Branch { pc: next, pos: at }) => {
                        pc = next;
                        pos = at;
                        break;
                    }
                    Some(Backtrack::Restore { slot, old }) => slots[slot] = old,
                    Some(Backtrack::Greedy {
                        next,
                        min_pos,
                        pos: from,
                    }) => {
                        let mut back = from - 1;
                        while !text.is_char_boundary(back) {
                            back -= 1;
                        }
                        if back > min_pos {
                            stack.push(Backtrack::Greedy {
                                next,
                                min_pos,
                                pos: back,
                            });
                        }
                        pc = next;
                        pos = back;
                        break;
                    }
                    Some(Backtrack::Lazy {
                        pc: at,
                        pos: from,
                        count,
                    }) => {
                        let Inst::Repeat { one, max, .. } = self.insts[at] else {
                            unreachable!("a lazy entry points at its repetition");
                        };
                        if let Some(next) = self.step(one, text, from) {
                            if max.is_none_or(|max| count + 1 < max) {
                                stack.push(Backtrack::Lazy {
                                    pc: at,
                                    pos: next,
                                    count: count + 1,
                                });
                            }
                            pc = at + 1;
                            pos = next;
                            break;
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(
        &self,
        one: One,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        text: &str,
        pc: usize,
        pos: &mut usize,
        stack: &mut Vec<Backtrack>,
    ) -> bool {
        let mut at = *pos;
        let mut count = 0;
        while count < min {
            match self.step(one, text, at) {
                Some(next) => at = next,
                None => return false,
            }
            count += 1;
        }
        if greedy {
            let min_pos = at;
            while max.is_none_or(|max| count < max) {
                match self.step(one, text, at) {
                    Some(next) => at = next,
                    None => break,
                }
                count += 1;
            }
            if at > min_pos {
                stack.push(Backtrack::Greedy {
                    next: pc + 1,
                    min_pos,
                    pos: at,
                });
            }
        } else if max.is_none_or(|max| count < max) {
            stack.push(Backtrack::Lazy { pc, pos: at, count });
        }
        *pos = at;
        true
    }

    /// Evaluates the lookaround at `pc`. Captures a positive lookaround sets are kept, and
    /// restored if the match later backtracks over it.
    #[allow(clippy::too_many_arguments)]
    fn look(
        &self,
        ahead: bool,
        negative: bool,
        width: usize,
        text: &str,
        pc: usize,
        pos: usize,
        slots: &mut [usize],
        stack: &mut Vec<Backtrack>,
    ) -> bool {
        let start = if ahead {
            Some(pos)
        } else {
            text[..pos]
                .char_indices()
                .rev()
                .nth(width.wrapping_sub(1))
                .map(|(i, _)| i)
                .or((width == 0).then_some(pos))
        };
        let before = slots.to_vec();
        let matched = start.is_some_and(|start| {
            let mut inner = Vec::new();
            self.run(text, pc + 1, start, slots, &mut inner).is_some()
        });
        if matched && !negative {
            for (slot, &old) in before.iter().enumerate() {
                if slots[slot] != old {
                    stack.push(Backtrack::Restore { slot, old });
                }
            }
            true
        } else {
            slots.copy_from_slice(&before);
            negative && !matched
        }
    }
}
