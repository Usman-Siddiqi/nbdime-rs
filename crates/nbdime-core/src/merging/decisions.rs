//! Merge decisions: how they are built, ordered and applied to the base.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use crate::diff_format::{diff_from_value, diff_to_value, Diff, DiffEntry, Key, Op};
use crate::log::{self, Level};
use crate::patch::patch;
use crate::path::{join_path, parse_int_string, star_path};
use crate::value::{Int, Map, Value};

use super::strategies::combine_patches;
use super::{assertion, invalid, MergeError};

/// What a decision does with the diffs of the two sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Keep the base.
    Base,
    Local,
    Remote,
    /// Both sides made the same change.
    Either,
    /// Apply `custom_diff` instead of either side.
    Custom,
    LocalThenRemote,
    RemoteThenLocal,
    /// Replace the value with an empty one of the same type.
    Clear,
    /// Remove the item.
    Remove,
    /// Remove every item of the parent.
    ClearAll,
    /// Keep the largest of the base, local and remote values.
    TakeMax,
    /// An action name nothing defines; applying it is an error.
    Other(String),
}

impl Action {
    pub fn as_str(&self) -> &str {
        match self {
            Action::Base => "base",
            Action::Local => "local",
            Action::Remote => "remote",
            Action::Either => "either",
            Action::Custom => "custom",
            Action::LocalThenRemote => "local_then_remote",
            Action::RemoteThenLocal => "remote_then_local",
            Action::Clear => "clear",
            Action::Remove => "remove",
            Action::ClearAll => "clear_all",
            Action::TakeMax => "take_max",
            Action::Other(name) => name,
        }
    }

    pub fn parse(name: &str) -> Action {
        match name {
            "base" => Action::Base,
            "local" => Action::Local,
            "remote" => Action::Remote,
            "either" => Action::Either,
            "custom" => Action::Custom,
            "local_then_remote" => Action::LocalThenRemote,
            "remote_then_local" => Action::RemoteThenLocal,
            "clear" => Action::Clear,
            "remove" => Action::Remove,
            "clear_all" => Action::ClearAll,
            "take_max" => Action::TakeMax,
            other => Action::Other(other.to_owned()),
        }
    }
}

/// One decision of a merge.
///
/// `local_diff` and `remote_diff` are the changes the two sides made at `common_path`; `None`
/// is a side that did not touch it (serialized as `null`, which differs from an empty list).
/// `custom_diff` is only present on decisions that carry one.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeDecision {
    pub common_path: Vec<Key>,
    pub conflict: bool,
    pub action: Action,
    pub local_diff: Option<Diff>,
    pub remote_diff: Option<Diff>,
    pub similar_insert: Option<Diff>,
    pub custom_diff: Option<Diff>,
    /// The strategy that produced the decision; internal, removed by [`validate`].
    pub strategy: Option<String>,
    /// The path depth that bundling strips from `common_path` (`_level`).
    pub level: Option<usize>,
}

impl MergeDecision {
    /// The decision as upstream serializes it: its keys in the order upstream sets them, and
    /// only the keys that are set.
    pub fn to_value(&self) -> Value {
        let diff = |d: &Option<Diff>| d.as_deref().map_or(Value::Null, diff_to_value);
        let mut m = Map::new();
        m.insert("common_path".into(), path_to_value(&self.common_path));
        m.insert("conflict".into(), Value::Bool(self.conflict));
        m.insert("action".into(), Value::from(self.action.as_str()));
        m.insert("local_diff".into(), diff(&self.local_diff));
        m.insert("remote_diff".into(), diff(&self.remote_diff));
        if let Some(d) = &self.similar_insert {
            m.insert("similar_insert".into(), diff_to_value(d));
        }
        if let Some(d) = &self.custom_diff {
            m.insert("custom_diff".into(), diff_to_value(d));
        }
        if let Some(s) = &self.strategy {
            m.insert("strategy".into(), Value::from(s.as_str()));
        }
        if let Some(level) = self.level {
            m.insert("_level".into(), Value::from(level));
        }
        Value::Object(m)
    }

    /// Reads a decision from its JSON form. Missing diffs read as `None`.
    pub fn from_value(v: &Value) -> Result<MergeDecision, MergeError> {
        let Value::Object(m) = v else {
            return invalid("a merge decision must be an object");
        };
        let diff = |name: &str| -> Result<Option<Diff>, MergeError> {
            match m.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => Ok(Some(diff_from_value(v)?)),
            }
        };
        let Some(Value::List(path)) = m.get("common_path") else {
            return invalid("a merge decision needs a common_path list");
        };
        let common_path = path
            .iter()
            .map(|k| match k {
                Value::Str(s) => Ok(Key::Name(s.clone())),
                Value::Int(i) => i
                    .as_i64()
                    .and_then(|i| usize::try_from(i).ok())
                    .map(Key::Index)
                    .ok_or_else(|| MergeError::Invalid(format!("bad path key {i}"))),
                other => invalid(format!("bad path key {other:?}")),
            })
            .collect::<Result<_, _>>()?;
        let Some(Value::Str(action)) = m.get("action") else {
            return invalid("a merge decision needs an action");
        };
        Ok(MergeDecision {
            common_path,
            conflict: m.get("conflict").is_some_and(Value::truthy),
            action: Action::parse(action),
            local_diff: diff("local_diff")?,
            remote_diff: diff("remote_diff")?,
            similar_insert: diff("similar_insert")?,
            custom_diff: diff("custom_diff")?,
            strategy: m.get("strategy").and_then(Value::as_str).map(str::to_owned),
            level: match m.get("_level") {
                Some(Value::Int(Int::Small(l))) => usize::try_from(*l).ok(),
                _ => None,
            },
        })
    }

    /// The part of `common_path` below the bundling level.
    pub fn local_path(&self) -> &[Key] {
        let level = self.level.unwrap_or(0);
        self.common_path.get(level..).unwrap_or(&[])
    }

    fn local(&self) -> Result<&Diff, MergeError> {
        required(&self.local_diff, "local_diff")
    }

    fn remote(&self) -> Result<&Diff, MergeError> {
        required(&self.remote_diff, "remote_diff")
    }

    fn custom(&self) -> Result<&Diff, MergeError> {
        required(&self.custom_diff, "custom_diff")
    }
}

fn required<'a>(diff: &'a Option<Diff>, name: &str) -> Result<&'a Diff, MergeError> {
    match diff {
        Some(d) => Ok(d),
        None => invalid(format!("decision has no {name}")),
    }
}

fn path_to_value(path: &[Key]) -> Value {
    Value::List(
        path.iter()
            .map(|k| match k {
                Key::Index(i) => Value::from(*i),
                Key::Name(s) => Value::from(s.as_str()),
            })
            .collect(),
    )
}

/// Python `==` on two entries (dicts): payloads compare as container items.
pub fn entries_eq(a: &DiffEntry, b: &DiffEntry) -> bool {
    a.key == b.key
        && match (&a.op, &b.op) {
            (Op::Add { value: x }, Op::Add { value: y })
            | (Op::Replace { value: x }, Op::Replace { value: y }) => x.py_eq_item(y),
            (Op::Remove, Op::Remove) | (Op::ParentDeleted, Op::ParentDeleted) => true,
            (Op::AddRange { valuelist: x }, Op::AddRange { valuelist: y }) => x.py_eq_item(y),
            (Op::RemoveRange { length: x }, Op::RemoveRange { length: y }) => x == y,
            (Op::Patch { diff: x }, Op::Patch { diff: y }) => diffs_eq(x, y),
            _ => false,
        }
}

/// Python `==` on two diffs (lists of entries).
pub fn diffs_eq(a: &[DiffEntry], b: &[DiffEntry]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| entries_eq(x, y))
}

fn nonempty(diff: &Option<Diff>) -> bool {
    diff.as_ref().is_some_and(|d| !d.is_empty())
}

/// Collects the decisions of a merge and applies the rules for registering them.
#[derive(Clone, Debug, Default)]
pub struct MergeDecisionBuilder {
    pub decisions: Vec<MergeDecision>,
}

/// What [`MergeDecisionBuilder::add_decision`] needs besides the diffs.
#[derive(Default)]
pub struct Extras {
    pub conflict: bool,
    pub strategy: Option<String>,
    pub custom_diff: Option<Diff>,
    pub similar_insert: Option<Diff>,
}

impl MergeDecisionBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.decisions.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, MergeDecision> {
        self.decisions.iter()
    }

    pub fn extend(&mut self, other: MergeDecisionBuilder) {
        self.decisions.extend(other.decisions);
    }

    pub fn has_conflicted(&self) -> bool {
        self.decisions.iter().any(|d| d.conflict)
    }

    pub fn get_conflicted(&self) -> Vec<&MergeDecision> {
        self.decisions.iter().filter(|d| d.conflict).collect()
    }

    /// The decisions ready for application: internal `strategy` fields removed and sorted so
    /// that deeper paths and higher indices come first.
    pub fn validated(self) -> Vec<MergeDecision> {
        let mut keyed: Vec<_> = self
            .decisions
            .into_iter()
            .map(|mut d| {
                d.strategy = None;
                (sort_key(&d.common_path), d)
            })
            .collect();
        // Python's `sorted(reverse=True)` keeps equal elements in their original order, as a
        // stable sort with the comparison flipped does.
        keyed.sort_by(|a, b| b.0.cmp(&a.0));
        keyed.into_iter().map(|(_, d)| d).collect()
    }

    /// Adds a decision, moving the common path as far down the tree as the diffs allow.
    pub fn add_decision(
        &mut self,
        path: Vec<Key>,
        action: Action,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        extras: Extras,
    ) {
        let (common_path, [local_diff, remote_diff, custom_diff]) =
            ensure_common_path(path, [local_diff, remote_diff, extras.custom_diff]);
        self.decisions.push(MergeDecision {
            common_path,
            conflict: extras.conflict,
            action,
            local_diff,
            remote_diff,
            similar_insert: extras.similar_insert,
            custom_diff,
            strategy: extras.strategy,
            level: None,
        });
    }

    pub fn base(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        conflict: bool,
        strategy: Option<String>,
    ) {
        self.add_decision(
            path,
            Action::Base,
            local_diff,
            remote_diff,
            Extras {
                conflict,
                strategy,
                ..Extras::default()
            },
        );
    }

    pub fn onesided(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
    ) -> Result<(), MergeError> {
        let (l, r) = (nonempty(&local_diff), nonempty(&remote_diff));
        if !(l || r) {
            return assertion("one diff needed in onesided merge decisions");
        }
        if l && r {
            return assertion("one diff should be empty in onesided merge decisions");
        }
        let action = if l { Action::Local } else { Action::Remote };
        self.add_decision(path, action, local_diff, remote_diff, Extras::default());
        Ok(())
    }

    pub fn local_then_remote(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        conflict: bool,
        strategy: Option<String>,
    ) -> Result<(), MergeError> {
        self.sequential(
            Action::LocalThenRemote,
            path,
            local_diff,
            remote_diff,
            conflict,
            strategy,
        )
    }

    pub fn remote_then_local(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        conflict: bool,
        strategy: Option<String>,
    ) -> Result<(), MergeError> {
        self.sequential(
            Action::RemoteThenLocal,
            path,
            local_diff,
            remote_diff,
            conflict,
            strategy,
        )
    }

    fn sequential(
        &mut self,
        action: Action,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        conflict: bool,
        strategy: Option<String>,
    ) -> Result<(), MergeError> {
        if !(nonempty(&local_diff) && nonempty(&remote_diff)) {
            return assertion("should have two diffs for sequential merge decisions");
        }
        self.add_decision(
            path,
            action,
            local_diff,
            remote_diff,
            Extras {
                conflict,
                strategy,
                ..Extras::default()
            },
        );
        Ok(())
    }

    pub fn agreement(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
    ) -> Result<(), MergeError> {
        if !(nonempty(&local_diff) && nonempty(&remote_diff)) {
            return assertion("should have two diffs for agreed merge decisions");
        }
        if !diffs_eq(
            local_diff.as_deref().unwrap(),
            remote_diff.as_deref().unwrap(),
        ) {
            return assertion("should have identical diffs for agreed merged decisions");
        }
        self.add_decision(
            path,
            Action::Either,
            local_diff,
            remote_diff,
            Extras::default(),
        );
        Ok(())
    }

    /// Resolves a conflict with `strategy` if it names one of the simple strategies.
    ///
    /// Registers a decision and returns its action when it does; returns `None` and registers
    /// nothing otherwise. The strategy `fail` is an error.
    pub fn tryresolve(
        &mut self,
        path: &[Key],
        local_diff: &Option<Diff>,
        remote_diff: &Option<Diff>,
        strategy: Option<&str>,
    ) -> Result<Option<Action>, MergeError> {
        let Some(strategy) = strategy.filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        check_conflicted(local_diff, remote_diff)?;
        let action = match strategy {
            "use-local" => Action::Local,
            "use-remote" => Action::Remote,
            "use-base" => Action::Base,
            "union" => Action::LocalThenRemote,
            "clear" => Action::Clear,
            "take-max" => Action::TakeMax,
            "fail" => {
                return Err(MergeError::Runtime(format!(
                    "Unexpected conflict on {}.",
                    path_repr(path)
                )))
            }
            // A strategy that only applies at another level: not resolved here.
            other => {
                log::emit(
                    Level::Warning,
                    "decisions",
                    224,
                    format!("Unhandled conflict strategy {other} on {}", join_path(path)),
                );
                return Ok(None);
            }
        };
        self.add_decision(
            path.to_vec(),
            action.clone(),
            local_diff.clone(),
            remote_diff.clone(),
            Extras {
                strategy: Some(strategy.to_owned()),
                ..Extras::default()
            },
        );
        Ok(Some(action))
    }

    /// Registers a conflict, resolving it with `strategy` first if possible.
    pub fn conflict(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        strategy: Option<&str>,
    ) -> Result<(), MergeError> {
        check_conflicted(&local_diff, &remote_diff)?;
        if self
            .tryresolve(&path, &local_diff, &remote_diff, strategy)?
            .is_none()
        {
            // The strategy was not applied, so it is not recorded on the decision.
            self.add_decision(
                path,
                Action::Base,
                local_diff,
                remote_diff,
                Extras {
                    conflict: true,
                    ..Extras::default()
                },
            );
        }
        Ok(())
    }

    /// [`MergeDecisionBuilder::conflict`] for inserts that are similar, which keeps the diff
    /// between them.
    pub fn similar_insert(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        insert_diff: Diff,
        strategy: Option<&str>,
    ) -> Result<(), MergeError> {
        check_conflicted(&local_diff, &remote_diff)?;
        if self
            .tryresolve(&path, &local_diff, &remote_diff, strategy)?
            .is_none()
        {
            self.add_decision(
                path,
                Action::Base,
                local_diff,
                remote_diff,
                Extras {
                    conflict: true,
                    similar_insert: Some(insert_diff),
                    ..Extras::default()
                },
            );
        }
        Ok(())
    }

    pub fn local(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        conflict: bool,
        strategy: Option<String>,
    ) -> Result<(), MergeError> {
        if !nonempty(&local_diff) {
            return assertion("needs non-empty local diff");
        }
        self.add_decision(
            path,
            Action::Local,
            local_diff,
            remote_diff,
            Extras {
                conflict,
                strategy,
                ..Extras::default()
            },
        );
        Ok(())
    }

    pub fn remote(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        conflict: bool,
        strategy: Option<String>,
    ) -> Result<(), MergeError> {
        if !nonempty(&remote_diff) {
            return assertion("needs non-empty remote diff");
        }
        self.add_decision(
            path,
            Action::Remote,
            local_diff,
            remote_diff,
            Extras {
                conflict,
                strategy,
                ..Extras::default()
            },
        );
        Ok(())
    }

    pub fn custom(
        &mut self,
        path: Vec<Key>,
        local_diff: Option<Diff>,
        remote_diff: Option<Diff>,
        custom_diff: Diff,
        conflict: bool,
        strategy: Option<String>,
    ) {
        self.add_decision(
            path,
            Action::Custom,
            local_diff,
            remote_diff,
            Extras {
                conflict,
                strategy,
                custom_diff: Some(custom_diff),
                ..Extras::default()
            },
        );
    }
}

impl<'a> IntoIterator for &'a MergeDecisionBuilder {
    type Item = &'a MergeDecision;
    type IntoIter = std::slice::Iter<'a, MergeDecision>;

    fn into_iter(self) -> Self::IntoIter {
        self.decisions.iter()
    }
}

fn check_conflicted(
    local_diff: &Option<Diff>,
    remote_diff: &Option<Diff>,
) -> Result<(), MergeError> {
    if !(nonempty(local_diff) && nonempty(remote_diff)) {
        return assertion("onesided merges should not be conflicted");
    }
    if diffs_eq(
        local_diff.as_deref().unwrap(),
        remote_diff.as_deref().unwrap(),
    ) {
        return assertion("agreed merges should not be conflicted");
    }
    Ok(())
}

fn path_repr(path: &[Key]) -> String {
    let items: Vec<String> = path
        .iter()
        .map(|k| match k {
            Key::Index(i) => i.to_string(),
            Key::Name(s) => format!("'{s}'"),
        })
        .collect();
    match items.as_slice() {
        [one] => format!("({one},)"),
        _ => format!("({})", items.join(", ")),
    }
}

/// Moves the keys that all diffs share as their single patch from the diffs to the path.
///
/// If a local and a remote diff both patch the key `a`, the path gains `a` and the diffs become
/// the inner diffs of the patches. Works repeatedly, so a common chain of patches is resolved
/// as well. Diffs that are empty or absent count as agreeing with any key.
pub fn ensure_common_path<const N: usize>(
    mut path: Vec<Key>,
    mut diffs: [Option<Diff>; N],
) -> (Vec<Key>, [Option<Diff>; N]) {
    while let Some(key) = poppable_key(diffs.iter().map(Option::as_deref)) {
        path.push(key);
        for slot in &mut diffs {
            *slot = match slot.take() {
                Some(mut d) if !d.is_empty() => match d.pop().map(|e| e.op) {
                    Some(Op::Patch { diff }) => Some(diff),
                    _ => None,
                },
                _ => None,
            };
        }
    }
    (path, diffs)
}

/// The key shared by the single patch of every non-empty diff, if there is one.
fn poppable_key<'a>(diffs: impl Iterator<Item = Option<&'a [DiffEntry]>>) -> Option<Key> {
    let mut key: Option<&Key> = None;
    for d in diffs {
        let Some(d) = d.filter(|d| !d.is_empty()) else {
            continue;
        };
        let [DiffEntry {
            key: k,
            op: Op::Patch { .. },
        }] = d
        else {
            return None;
        };
        match key {
            None => key = Some(k),
            Some(existing) if existing != k => return None,
            Some(_) => {}
        }
    }
    key.cloned()
}

/// Wraps `diffs` in patch entries for each key of `path`, innermost key last.
pub fn push_path(path: &[Key], mut diffs: Diff) -> Diff {
    for key in path.iter().rev() {
        diffs = vec![DiffEntry::patch(key.clone(), diffs)];
    }
    diffs
}

/// A new decision one level further down, if the diffs allow it (see [`ensure_common_path`]).
pub fn pop_patch_decision(decision: &MergeDecision) -> Option<MergeDecision> {
    let mut diffs = [
        decision.local_diff.clone(),
        decision.remote_diff.clone(),
        if decision.action == Action::Custom {
            decision.custom_diff.clone()
        } else {
            None
        },
    ];
    let key = poppable_key(diffs.iter().map(Option::as_deref))?;
    for slot in &mut diffs {
        *slot = match slot.take() {
            Some(mut d) if !d.is_empty() => match d.pop().map(|e| e.op) {
                Some(Op::Patch { diff }) => Some(diff),
                _ => None,
            },
            _ => None,
        };
    }
    let [local_diff, remote_diff, custom_diff] = diffs;
    let mut common_path = decision.common_path.clone();
    common_path.push(key);
    Some(MergeDecision {
        common_path,
        conflict: decision.conflict,
        action: decision.action.clone(),
        local_diff,
        remote_diff,
        similar_insert: None,
        custom_diff,
        strategy: None,
        level: None,
    })
}

/// [`pop_patch_decision`] repeatedly, down to the deepest level.
pub fn pop_all_patch_decisions(decision: MergeDecision) -> MergeDecision {
    let mut decision = decision;
    while let Some(popped) = pop_patch_decision(&decision) {
        decision = popped;
    }
    decision
}

/// Moves the keys in `prefix` from the end of `common_path` into the diffs, as nested patches.
pub fn push_patch_decision(
    decision: &MergeDecision,
    prefix: &[Key],
) -> Result<MergeDecision, MergeError> {
    let mut dec = decision.clone();
    for key in prefix.iter().rev() {
        let Some(last) = dec.common_path.pop() else {
            return invalid(format!("Cannot remove key from empty decision path: {key}"));
        };
        if last != *key {
            return assertion(format!("Key {key} not at end of {:?}", dec.common_path));
        }
        let wrap = |d: &Option<Diff>| match d {
            Some(d) if !d.is_empty() => vec![DiffEntry::patch(key.clone(), d.clone())],
            _ => Vec::new(),
        };
        dec.local_diff = Some(wrap(&dec.local_diff));
        dec.remote_diff = Some(wrap(&dec.remote_diff));
        if dec.action == Action::Custom {
            dec.custom_diff = Some(wrap(&Some(dec.custom()?.clone())));
        }
    }
    Ok(dec)
}

/// An integer as Python orders it, held as sign and digits so that any size sorts correctly.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SortInt {
    negative: bool,
    /// Decimal digits without leading zeros.
    magnitude: String,
}

impl SortInt {
    fn new(negative: bool, digits: &str) -> SortInt {
        let magnitude = digits.trim_start_matches('0');
        if magnitude.is_empty() {
            return SortInt {
                negative: false,
                magnitude: "0".into(),
            };
        }
        SortInt {
            negative,
            magnitude: magnitude.into(),
        }
    }
}

impl Ord for SortInt {
    fn cmp(&self, other: &Self) -> Ordering {
        let magnitude = self
            .magnitude
            .len()
            .cmp(&other.magnitude.len())
            .then_with(|| self.magnitude.cmp(&other.magnitude));
        match (self.negative, other.negative) {
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

impl PartialOrd for SortInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One path element as the tuple Python sorts by: `('', -n)` for an integer and `(s,)` for any
/// other string, so that every integer sorts before every non-empty string and a longer tuple
/// sorts after its prefix.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SortElement {
    head: String,
    tail: Option<SortInt>,
}

/// Sort key for a common path, so that sorting descending processes decisions in an order
/// that needs no index adjustment: higher indices first, deeper paths before their parents.
fn sort_key(path: &[Key]) -> Vec<SortElement> {
    path.iter()
        .map(|key| {
            let negated = match key {
                Key::Index(i) => Some(SortInt::new(*i != 0, &i.to_string())),
                Key::Name(s) => {
                    parse_int_string(s).map(|(neg, digits)| SortInt::new(!neg, &digits))
                }
            };
            match (negated, key) {
                (Some(n), _) => SortElement {
                    head: String::new(),
                    tail: Some(n),
                },
                (None, Key::Name(s)) => SortElement {
                    head: s.clone(),
                    tail: None,
                },
                (None, Key::Index(_)) => unreachable!("an index is an integer"),
            }
        })
        .collect()
}

/// Splits `path` where it enters a string: decisions on a line of a string cannot resolve in
/// the base, since strings are diffed by line without being split.
pub fn split_string_path<'p>(
    base: &Value,
    path: &'p [Key],
) -> Result<(&'p [Key], &'p [Key]), MergeError> {
    let mut current = base;
    for (i, key) in path.iter().enumerate() {
        if current.is_str() {
            return Ok((&path[..i], &path[i..]));
        }
        current = get_item(current, key)?;
    }
    Ok((path, &[]))
}

/// `base[key]` for a list or dict.
pub fn get_item<'a>(base: &'a Value, key: &Key) -> Result<&'a Value, MergeError> {
    match (base, key) {
        (Value::List(items), Key::Index(i)) => items
            .get(*i)
            .map_or_else(|| invalid("list index out of range"), Ok),
        (Value::Object(map), Key::Name(name)) => map
            .get(name)
            .map_or_else(|| invalid(format!("KeyError: {name:?}")), Ok),
        _ => invalid(format!("cannot index this value with {key}")),
    }
}

fn get_item_mut<'a>(base: &'a mut Value, key: &Key) -> Result<&'a mut Value, MergeError> {
    match (base, key) {
        (Value::List(items), Key::Index(i)) => items
            .get_mut(*i)
            .map_or_else(|| invalid("list index out of range"), Ok),
        (Value::Object(map), Key::Name(name)) => map
            .get_mut(name)
            .map_or_else(|| invalid(format!("KeyError: {name:?}")), Ok),
        _ => invalid(format!("cannot index this value with {key}")),
    }
}

/// `base[p0][p1]...`
pub fn resolve_path<'a>(base: &'a Value, path: &[Key]) -> Result<&'a Value, MergeError> {
    path.iter()
        .try_fold(base, |value, key| get_item(value, key))
}

/// `resolve_path` that also indexes into strings, as Python does.
pub fn resolve_path_chars<'a>(base: &'a Value, path: &[Key]) -> Result<Cow<'a, Value>, MergeError> {
    path.iter()
        .try_fold(Cow::Borrowed(base), |value, key| match value {
            Cow::Borrowed(v) if !matches!(v, Value::Str(_)) => get_item(v, key).map(Cow::Borrowed),
            v => item_value(&v, key).map(Cow::Owned),
        })
}

/// A cleared value of the same type: an empty list, dict or string, otherwise null.
pub fn make_cleared_value(value: &Value) -> Value {
    match value {
        Value::List(_) => Value::List(Vec::new()),
        Value::Object(_) => Value::Object(Map::new()),
        Value::Str(_) => Value::Str(String::new()),
        _ => Value::Null,
    }
}

/// `base[key]` where `base` may also be a string, whose items are its characters.
fn item_value(base: &Value, key: &Key) -> Result<Value, MergeError> {
    match (base, key) {
        (Value::Str(s), Key::Index(i)) => s
            .chars()
            .nth(*i)
            .map(|c| Value::Str(c.to_string()))
            .map_or_else(|| invalid("string index out of range"), Ok),
        _ => get_item(base, key).cloned(),
    }
}

fn the_only_key(local: &Diff, remote: &Diff) -> Result<Key, MergeError> {
    let mut keys: Vec<&Key> = Vec::new();
    for e in local.iter().chain(remote) {
        if !keys.contains(&&e.key) {
            keys.push(&e.key);
        }
    }
    match keys.as_slice() {
        [key] => Ok((*key).clone()),
        [] => invalid("not enough values to unpack (expected 1, got 0)"),
        _ => invalid("too many values to unpack (expected 1)"),
    }
}

/// The diff that a decision stands for, to be applied to `base`. A side the decision does not
/// have is passed on as `None` for the actions that only copy it (Python's `copy.copy(None)`);
/// whoever uses the result decides what that means.
pub fn resolve_action(base: &Value, decision: &MergeDecision) -> Result<Option<Diff>, MergeError> {
    match &decision.action {
        Action::Base => Ok(Some(Vec::new())),
        Action::Local | Action::Either => Ok(decision.local_diff.clone()),
        Action::Remote => Ok(decision.remote_diff.clone()),
        Action::Custom => Ok(decision.custom_diff.clone()),
        Action::LocalThenRemote => {
            let mut diff = decision.local()?.clone();
            diff.extend(decision.remote()?.iter().cloned());
            Ok(Some(diff))
        }
        Action::RemoteThenLocal => {
            let mut diff = decision.remote()?.clone();
            diff.extend(decision.local()?.iter().cloned());
            Ok(Some(diff))
        }
        action @ (Action::Clear | Action::Remove) => {
            let key = the_only_key(decision.local()?, decision.remote()?)?;
            if *action == Action::Clear {
                let cleared = match (base, &key) {
                    (Value::Str(_), _) => make_cleared_value(&item_value(base, &key)?),
                    _ => make_cleared_value(get_item(base, &key)?),
                };
                Ok(Some(vec![DiffEntry::replace(key, cleared)]))
            } else if matches!(base, Value::List(_) | Value::Str(_)) {
                let Key::Index(i) = key else {
                    return invalid("list key must be integer");
                };
                Ok(Some(vec![DiffEntry::removerange(i, 1)]))
            } else {
                Ok(Some(vec![DiffEntry::remove(key)]))
            }
        }
        Action::ClearAll => match base {
            Value::Object(map) => Ok(Some(
                map.keys().map(|k| DiffEntry::remove(k.as_str())).collect(),
            )),
            Value::List(items) => Ok(Some(vec![DiffEntry::removerange(0, items.len())])),
            Value::Str(s) => Ok(Some(vec![DiffEntry::removerange(0, s.chars().count())])),
            // Upstream falls off the end of the function.
            _ => Ok(None),
        },
        Action::TakeMax => {
            let (local, remote) = (decision.local()?, decision.remote()?);
            let key = the_only_key(local, remote)?;
            let bval = item_value(base, &key)?;
            let side_value = |diff: &Diff| -> Result<Value, MergeError> {
                match diff.first() {
                    None => Ok(bval.clone()),
                    Some(DiffEntry {
                        op: Op::Add { value } | Op::Replace { value },
                        ..
                    }) => Ok(value.clone()),
                    Some(e) => {
                        invalid(format!("'value' is missing from the {} entry", e.op.name()))
                    }
                }
            };
            let (lval, rval) = (side_value(local)?, side_value(remote)?);
            let mut max = &bval;
            for candidate in [&lval, &rval] {
                if py_greater(candidate, max)? {
                    max = candidate;
                }
            }
            if bval.py_eq(max) {
                Ok(Some(Vec::new()))
            } else {
                Ok(Some(vec![DiffEntry::replace(key, max.clone())]))
            }
        }
        Action::Other(name) => Err(MergeError::NotImplemented(name.clone())),
    }
}

/// Python `a > b` for the values JSON has: numbers (with bool), strings and lists of those;
/// every other pair is a `TypeError`.
fn py_greater(a: &Value, b: &Value) -> Result<bool, MergeError> {
    Ok(py_compare(a, b)? == Some(Ordering::Greater))
}

/// Python's ordering of two values, `None` where a comparison is false both ways (NaN).
fn py_compare(a: &Value, b: &Value) -> Result<Option<Ordering>, MergeError> {
    use Value::*;
    let number = |v: &Value| match v {
        Bool(b) => Some(crate::value::Int::Small(i64::from(*b))),
        Int(i) => Some(i.clone()),
        _ => None,
    };
    match (a, b) {
        (Str(x), Str(y)) => Ok(Some(x.cmp(y))),
        (List(x), List(y)) => {
            for (p, q) in x.iter().zip(y) {
                if !p.py_eq_item(q) {
                    return py_compare(p, q);
                }
            }
            Ok(Some(x.len().cmp(&y.len())))
        }
        (Float(x), Float(y)) => Ok(x.partial_cmp(y)),
        (Float(x), other) | (other, Float(x)) if number(other).is_some() => {
            let i = number(other).expect("checked above");
            let ordering = cmp_int_float(&i, *x);
            Ok(if matches!(a, Float(_)) {
                ordering.map(Ordering::reverse)
            } else {
                ordering
            })
        }
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => Ok(Some(crate::value::cmp_int(&x, &y))),
            _ => invalid(format!(
                "'>' not supported between instances of '{}' and '{}'",
                crate::diff_utils::type_name(a),
                crate::diff_utils::type_name(b)
            )),
        },
    }
}

/// Exact comparison of an integer with a float; `None` for NaN.
fn cmp_int_float(i: &crate::value::Int, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let floor = f.floor();
    let floor_int = crate::value::Int::from_literal(&format!("{floor:.0}"));
    Some(match crate::value::cmp_int(i, &floor_int) {
        Ordering::Equal if f.fract() != 0.0 => Ordering::Less,
        other => other,
    })
}

/// Applies a list of decisions to `base`.
pub fn apply_decisions(base: &Value, decisions: &[MergeDecision]) -> Result<Value, MergeError> {
    /// The diffs collected for one path, applied together.
    struct Group {
        path: Vec<Key>,
        /// `None` is Python's `None` (a decision that has no such diff), which fails wherever it
        /// is used, unless a `clear_all` replaces the collected diffs first.
        diffs: Option<Diff>,
        clear_all: bool,
        /// Whether a second decision joined the group, which upstream combines the patches
        /// for (a group of one decision is patched as it is). Combining is deferred to the
        /// flush: it is idempotent and stable, so one pass gives what a pass per decision does.
        combine: bool,
    }

    fn flush(merged: &mut Value, group: Group) -> Result<(), MergeError> {
        let Some(mut diffs) = group.diffs else {
            return invalid("'NoneType' object is not iterable");
        };
        if group.combine {
            diffs = combine_patches(diffs)?;
        }
        let resolved = resolve_path(merged, &group.path)?;
        let patched = patch(resolved, &diffs)?;
        let mut target = merged;
        for key in &group.path {
            target = get_item_mut(target, key)?;
        }
        *target = patched;
        Ok(())
    }

    /// `resolve_action` followed by wrapping the result in the patches of `line`.
    fn resolve_in(
        resolved: &Value,
        md: &MergeDecision,
        line: &[Key],
    ) -> Result<Option<Diff>, MergeError> {
        let diffs = resolve_action(resolved, md)?;
        if line.is_empty() {
            return Ok(diffs);
        }
        match diffs {
            Some(diffs) => Ok(Some(push_path(line, diffs))),
            None => assertion("Patch op needs a diff sequence"),
        }
    }

    let mut merged = base.clone();
    let mut group: Option<Group> = None;
    for md in decisions {
        let (path, line) = split_string_path(&merged, &md.common_path)?;
        match &mut group {
            Some(g) if g.path == path => {
                // A clear_all on the parent makes every other decision on it moot.
                if !g.clear_all {
                    if md.action == Action::ClearAll {
                        if g.combine {
                            // The combine that would have failed upstream still fails.
                            if let Some(diffs) = g.diffs.take() {
                                combine_patches(diffs)?;
                            }
                        }
                        g.clear_all = true;
                        g.diffs = Some(Vec::new());
                    }
                    let ad = resolve_in(resolve_path(&merged, path)?, md, line)?;
                    let (Some(mut diffs), Some(ad)) = (g.diffs.take(), ad) else {
                        return invalid("can only concatenate list (not \"NoneType\") to list");
                    };
                    diffs.extend(ad);
                    g.diffs = Some(diffs);
                    g.combine = true;
                }
            }
            _ => {
                if let Some(previous) = group.take() {
                    flush(&mut merged, previous)?;
                }
                let diffs = resolve_in(resolve_path(&merged, path)?, md, line)?;
                group = Some(Group {
                    path: path.to_vec(),
                    diffs,
                    clear_all: md.action == Action::ClearAll,
                    combine: false,
                });
            }
        }
    }
    if let Some(last) = group {
        flush(&mut merged, last)?;
    }
    Ok(merged)
}

/// Indices of the decisions whose path, extended by the key their diffs share, starts with
/// `pattern` (a starred path as a string). With `exact`, the path may equal it instead.
pub fn filter_decisions(pattern: &str, decisions: &[MergeDecision], exact: bool) -> Vec<usize> {
    let mut indices = Vec::new();
    for (i, md) in decisions.iter().enumerate() {
        let mut path = md.common_path.clone();
        let diffs = [
            md.local_diff.as_deref(),
            md.remote_diff.as_deref(),
            md.custom_diff.as_deref(),
        ];
        if let Some(key) = poppable_key(diffs.into_iter()) {
            path.push(key);
        }
        let starred = star_path(&path);
        let prefix = starred
            .chars()
            .take(pattern.chars().count())
            .collect::<String>();
        if (exact && starred == pattern) || prefix == pattern {
            indices.push(i);
        }
    }
    indices
}

/// The entries of one path in a tree of diffs built by [`build_diffs`].
struct TreeNode {
    diff: Diff,
    path: Vec<Key>,
}

/// Builds one diff for the base out of all decisions, nested at the root: the `local` or the
/// `remote` diffs of the decisions. `None` when there are no diffs to apply.
///
/// The diffs are copied; upstream shares them with the decisions and extends them in place.
/// The `merged` variant upstream offers fails on its first decision (it indexes the base with
/// a path tuple), and so does this.
pub fn build_diffs(
    base: &Value,
    decisions: &[MergeDecision],
    which: Which,
) -> Result<Option<Diff>, MergeError> {
    let mut tree: HashMap<String, TreeNode> = HashMap::new();
    let mut sorted_paths: Vec<String> = Vec::new();

    for md in decisions {
        let (path, line) = split_string_path(base, md.local_path())?;
        let subdiffs = match which {
            Which::Merged => return invalid("unhashable type: 'tuple'"),
            Which::Local => md.local_diff.clone(),
            Which::Remote => md.remote_diff.clone(),
        };
        let Some(subdiffs) = subdiffs else { continue };

        let str_path = join_path(path);
        if let Some(node) = tree.get_mut(&str_path) {
            if let Some(first) = line.first() {
                let matching: Vec<usize> = node
                    .diff
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| d.key == *first)
                    .map(|(i, _)| i)
                    .collect();
                match matching.as_slice() {
                    [] => {
                        let pushed = push_path(line, subdiffs);
                        if pushed.len() != 1 {
                            return assertion("expected one pushed diff entry");
                        }
                        node.diff.extend(pushed);
                    }
                    [i] => match &mut node.diff[*i].op {
                        Op::Patch { diff } if !diff.is_empty() => diff.extend(subdiffs),
                        _ => return assertion("empty diff found in tree"),
                    },
                    _ => return assertion("multiple mathing diffs found in tree"),
                }
            } else {
                node.diff.extend(subdiffs);
            }
        } else {
            let subdiffs = if line.is_empty() {
                subdiffs
            } else {
                push_path(line, subdiffs)
            };
            tree.insert(
                str_path.clone(),
                TreeNode {
                    diff: subdiffs,
                    path: path.to_vec(),
                },
            );
            sorted_paths.push(str_path);
        }
    }

    if tree.is_empty() {
        return Ok(None);
    }
    if !tree.contains_key("/") {
        tree.insert(
            "/".into(),
            TreeNode {
                diff: Vec::new(),
                path: Vec::new(),
            },
        );
        sorted_paths.push("/".into());
    }
    Ok(Some(merge_tree(&tree, &sorted_paths)))
}

/// Which diffs [`build_diffs`] collects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Local,
    Remote,
    Merged,
}

fn is_prefix_array(parent: Option<&[Key]>, child: &[Key]) -> bool {
    match parent {
        None => true,
        Some(parent) => parent.is_empty() || child.starts_with(parent),
    }
}

/// Merges a tree of diffs at varying path levels into one diff at their shared root. Relies on
/// the ordering of decisions: deeper paths come before their parents.
fn merge_tree(tree: &HashMap<String, TreeNode>, sorted_paths: &[String]) -> Diff {
    /// A trunk set aside while the paths after it are merged into a new one.
    struct Pending {
        trunk: Diff,
        path: Vec<Key>,
    }

    let last_path = &tree[&sorted_paths[sorted_paths.len() - 1]].path;
    let mut pending: Vec<Pending> = Vec::new();
    let mut start = 0;
    let mut result = loop {
        let segment = &sorted_paths[start..];
        let mut trunk: Diff = Vec::new();
        let mut root: Option<Vec<Key>> = None;
        let mut next_start = None;
        for (i, name) in segment.iter().enumerate() {
            let node = &tree[name];
            trunk.extend(node.diff.iter().cloned());
            let next_path: Option<&[Key]> = if i == segment.len() - 1 {
                root.as_deref()
            } else {
                Some(&tree[&segment[i + 1]].path)
            };
            if is_prefix_array(next_path, &node.path) {
                if let Some(next_path) = next_path {
                    trunk = push_path(&node.path[next_path.len()..], trunk);
                    root = Some(next_path.to_vec());
                }
            } else {
                pending.push(Pending {
                    trunk: std::mem::take(&mut trunk),
                    path: node.path.clone(),
                });
                next_start = Some(start + i + 1);
                break;
            }
        }
        match next_start {
            Some(next) => start = next,
            None => break trunk,
        }
    };
    while let Some(Pending { trunk, path }) = pending.pop() {
        let shared = path
            .iter()
            .zip(last_path)
            .take_while(|(a, b)| a == b)
            .count();
        let mut joined = push_path(&path[shared..], trunk);
        joined.extend(push_path(&last_path[shared..], result));
        result = joined;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    #[test]
    fn resolve_path_chars_indexes_into_strings() {
        let base = v(r#"{"a":["xy",{"b":1}]}"#);
        let path = [key("a"), Key::Index(0), Key::Index(1)];
        assert_eq!(*resolve_path_chars(&base, &path).unwrap(), Value::from("y"));
        assert!(resolve_path(&base, &path).is_err());
        let path = [key("a"), Key::Index(1)];
        assert!(resolve_path_chars(&base, &path).unwrap().is_object());
        let path = [key("a"), Key::Index(0), Key::Index(2)];
        assert!(resolve_path_chars(&base, &path).is_err());
    }

    fn diff(text: &str) -> Diff {
        diff_from_value(&v(text)).unwrap()
    }

    fn key(s: &str) -> Key {
        Key::from(s)
    }

    #[test]
    fn common_path_is_pushed_out_through_shared_patches() {
        let l = diff(
            r#"[{"op":"patch","key":"a","diff":[{"op":"patch","key":"b","diff":[{"op":"remove","key":"c"}]}]}]"#,
        );
        let r = diff(
            r#"[{"op":"patch","key":"a","diff":[{"op":"patch","key":"b","diff":[{"op":"add","key":"d","value":1}]}]}]"#,
        );
        let mut b = MergeDecisionBuilder::new();
        b.add_decision(
            vec![],
            Action::Base,
            Some(l.clone()),
            Some(r),
            Extras::default(),
        );
        b.onesided(vec![], Some(l), None).unwrap();
        for d in &b.decisions {
            assert_eq!(d.common_path, [key("a"), key("b")]);
        }
        assert_eq!(
            b.decisions[0].local_diff,
            Some(diff(r#"[{"op":"remove","key":"c"}]"#))
        );
        assert_eq!(
            b.decisions[0].remote_diff,
            Some(diff(r#"[{"op":"add","key":"d","value":1}]"#))
        );
        assert_eq!(b.decisions[1].remote_diff, None);
    }

    #[test]
    fn empty_diffs_become_null_only_when_a_patch_is_popped() {
        let p = diff(r#"[{"op":"patch","key":"a","diff":[{"op":"remove","key":"c"}]}]"#);
        let mut b = MergeDecisionBuilder::new();
        b.onesided(vec![], Some(p.clone()), Some(vec![])).unwrap();
        let removal = diff(r#"[{"op":"removerange","key":0,"length":1}]"#);
        b.onesided(vec![], Some(removal), Some(vec![])).unwrap();
        let text = |i: usize| {
            crate::json::dumps(
                &b.decisions[i].to_value(),
                &crate::json::DumpOptions::compact(),
            )
        };
        assert!(text(0).contains(r#""remote_diff":null"#));
        assert!(text(1).contains(r#""remote_diff":[]"#));
    }

    #[test]
    fn decision_keys_keep_upstream_order() {
        let mut b = MergeDecisionBuilder::new();
        let l = diff(r#"[{"op":"addrange","key":1,"valuelist":["x"]}]"#);
        let r = diff(r#"[{"op":"addrange","key":1,"valuelist":["y"]}]"#);
        b.similar_insert(
            vec![key("cells")],
            Some(l.clone()),
            Some(r.clone()),
            l.clone(),
            None,
        )
        .unwrap();
        b.custom(vec![], Some(l), Some(r), vec![], true, Some("s".into()));
        let keys = |i: usize| -> Vec<String> {
            b.decisions[i]
                .to_value()
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        assert_eq!(
            keys(0),
            [
                "common_path",
                "conflict",
                "action",
                "local_diff",
                "remote_diff",
                "similar_insert"
            ]
        );
        assert_eq!(
            keys(1),
            [
                "common_path",
                "conflict",
                "action",
                "local_diff",
                "remote_diff",
                "custom_diff",
                "strategy"
            ]
        );
        let sorted = b.validated();
        assert!(sorted.iter().all(|d| d.strategy.is_none()));
    }

    #[test]
    fn sort_key_orders_descending_with_deeper_paths_first() {
        let decisions = [
            vec![key("cells")],
            vec![key("cells"), Key::Index(2)],
            vec![key("cells"), Key::Index(10)],
            vec![key("cells"), Key::Index(2), key("source")],
            vec![key("metadata")],
            vec![],
            vec![key("cells"), key("3")],
        ];
        let mut b = MergeDecisionBuilder::new();
        for p in &decisions {
            b.add_decision(p.clone(), Action::Base, None, None, Extras::default());
        }
        let order: Vec<_> = b.validated().into_iter().map(|d| d.common_path).collect();
        assert_eq!(
            order,
            [
                vec![key("metadata")],
                vec![key("cells"), Key::Index(2), key("source")],
                vec![key("cells"), Key::Index(2)],
                vec![key("cells"), key("3")],
                vec![key("cells"), Key::Index(10)],
                vec![key("cells")],
                vec![],
            ]
        );
    }

    #[test]
    fn tryresolve_applies_simple_strategies_and_rejects_fail() {
        let l = diff(r#"[{"op":"replace","key":"a","value":1}]"#);
        let r = diff(r#"[{"op":"replace","key":"a","value":2}]"#);
        let mut b = MergeDecisionBuilder::new();
        b.conflict(vec![], Some(l.clone()), Some(r.clone()), Some("use-local"))
            .unwrap();
        assert_eq!(b.decisions[0].action, Action::Local);
        assert!(!b.decisions[0].conflict);
        b.conflict(
            vec![],
            Some(l.clone()),
            Some(r.clone()),
            Some("inline-source"),
        )
        .unwrap();
        assert!(b.decisions[1].conflict);
        assert_eq!(b.decisions[1].action, Action::Base);
        assert!(b
            .conflict(vec![], Some(l.clone()), Some(r), Some("fail"))
            .is_err());
        assert!(b.conflict(vec![], Some(l.clone()), Some(l), None).is_err());
    }

    fn apply(base: &str, decisions: &[MergeDecision]) -> Result<Value, MergeError> {
        apply_decisions(&v(base), decisions)
    }

    fn decision(path: &[Key], action: Action, local: &str, remote: &str) -> MergeDecision {
        let parse_side = |s: &str| (!s.is_empty()).then(|| diff(s));
        MergeDecision {
            common_path: path.to_vec(),
            conflict: false,
            action,
            local_diff: parse_side(local),
            remote_diff: parse_side(remote),
            similar_insert: None,
            custom_diff: None,
            strategy: None,
            level: None,
        }
    }

    #[test]
    fn applies_decisions_grouped_by_path() {
        let l = r#"[{"op":"addrange","key":1,"valuelist":["x"]}]"#;
        let r = r#"[{"op":"removerange","key":0,"length":1}]"#;
        let d1 = decision(&[key("a")], Action::Local, l, "");
        let d2 = decision(&[key("a")], Action::Remote, "", r);
        let d3 = decision(
            &[key("b")],
            Action::Local,
            r#"[{"op":"replace","key":"k","value":7}]"#,
            "",
        );
        let out = apply(r#"{"a":[1,2],"b":{"k":1}}"#, &[d1, d2, d3]).unwrap();
        assert_eq!(out, v(r#"{"a":["x",2],"b":{"k":7}}"#));
    }

    #[test]
    fn clear_all_overrides_other_decisions_on_the_same_path() {
        let ins = r#"[{"op":"addrange","key":0,"valuelist":["x"]}]"#;
        let d1 = decision(&[key("a")], Action::Local, ins, "");
        let d2 = decision(&[key("a")], Action::ClearAll, "", "");
        let d3 = decision(&[key("a")], Action::Local, ins, "");
        let out = apply(r#"{"a":[1,2]}"#, &[d1, d2, d3]).unwrap();
        assert_eq!(out, v(r#"{"a":[]}"#));
    }

    #[test]
    fn combining_once_matches_combining_after_every_decision() {
        let base = v(r#"{"a":[0,1,2,3,4,5],"b":{"k":1,"m":2,"n":3}}"#);
        let sides = [
            r#"[{"op":"patch","key":"a","diff":[{"op":"addrange","key":4,"valuelist":["p"]}]}]"#,
            r#"[{"op":"patch","key":"b","diff":[{"op":"replace","key":"n","value":9}]}]"#,
            r#"[{"op":"patch","key":"a","diff":[{"op":"addrange","key":1,"valuelist":["q"]},{"op":"removerange","key":2,"length":1}]}]"#,
            r#"[{"op":"patch","key":"b","diff":[{"op":"remove","key":"k"}]},{"op":"patch","key":"a","diff":[{"op":"addrange","key":0,"valuelist":["r"]}]}]"#,
        ];
        let decisions: Vec<_> = sides
            .iter()
            .map(|d| decision(&[], Action::Local, d, ""))
            .collect();
        let mut reference: Option<Diff> = None;
        for md in &decisions {
            let ad = resolve_action(&base, md).unwrap().unwrap();
            reference = Some(match reference {
                None => ad,
                Some(mut acc) => {
                    acc.extend(ad);
                    combine_patches(acc).unwrap()
                }
            });
        }
        let expected = patch(&base, &reference.unwrap()).unwrap();
        assert_eq!(apply_decisions(&base, &decisions).unwrap(), expected);
    }

    #[test]
    fn clear_all_still_fails_on_an_earlier_uncombinable_group() {
        let by_index = r#"[{"op":"addrange","key":0,"valuelist":["x"]}]"#;
        let by_name = r#"[{"op":"remove","key":"k"}]"#;
        let d1 = decision(&[key("a")], Action::Local, by_index, "");
        let d2 = decision(&[key("a")], Action::Local, by_name, "");
        let d3 = decision(&[key("a")], Action::ClearAll, "", "");
        assert!(apply(r#"{"a":[1,2]}"#, &[d1.clone(), d2.clone()]).is_err());
        // Upstream combines after the second decision, before the clear_all drops it.
        assert!(apply(r#"{"a":[1,2]}"#, &[d1, d2, d3]).is_err());
    }

    #[test]
    fn many_one_sided_inserts_on_one_path_apply() {
        let n = 20_000usize;
        let base = Value::Object(
            [("a".to_owned(), Value::List(vec![Value::from(0); n]))]
                .into_iter()
                .collect(),
        );
        let decisions: Vec<_> = (0..n)
            .map(|i| {
                let ins = format!(r#"[{{"op":"addrange","key":{i},"valuelist":[1]}}]"#);
                decision(&[key("a")], Action::Local, &ins, "")
            })
            .collect();
        let merged = apply_decisions(&base, &decisions).unwrap();
        assert_eq!(merged.get("a").unwrap().as_list().unwrap().len(), 2 * n);
    }

    #[test]
    fn a_null_diff_only_fails_where_it_is_used() {
        let remove_c = r#"[{"op":"remove","key":"c"}]"#;
        let add_n = r#"[{"op":"add","key":"n","value":""}]"#;
        let null_local = decision(&[], Action::Local, "", remove_c);
        let clear_all = decision(&[], Action::ClearAll, add_n, "");
        // The clear_all replaces the collected diffs, `None` included.
        assert_eq!(
            apply(r#"{"c":1}"#, &[null_local.clone(), clear_all]).unwrap(),
            v("{}")
        );
        // Used as it is, or joined by a decision that is no clear_all, it fails.
        assert!(apply(r#"{"c":1}"#, std::slice::from_ref(&null_local)).is_err());
        let more = decision(&[], Action::Remote, "", remove_c);
        assert!(apply(r#"{"c":1}"#, &[null_local.clone(), more.clone()]).is_err());
        assert!(apply(r#"{"c":1}"#, &[more, null_local]).is_err());
        // `local_then_remote` adds the diffs, which fails for `None`.
        let sum = decision(&[], Action::LocalThenRemote, "", remove_c);
        assert!(apply(r#"{"c":1}"#, &[sum]).is_err());
        // So does wrapping it in the patches of a string path.
        let path = [key("a"), Key::Index(0), Key::Index(0)];
        let line = decision(&path, Action::Local, "", remove_c);
        assert!(apply(r#"{"a":["xy"]}"#, &[line]).is_err());
        assert_eq!(
            split_string_path(&v(r#"{"a":["xy"]}"#), &path)
                .unwrap()
                .1
                .len(),
            1
        );
    }

    #[test]
    fn decisions_from_decide_merge_apply_again() {
        use crate::merging::{decide_merge, Strategies};
        let base = Value::from("bbabbbbbbbbbaaabc\u{b}");
        let local = Value::from("baaabbabbbabbbaaabbbbbaaabababc\u{b}");
        let remote = Value::from("for i in range(10):\n");
        let mut strategies = Strategies::new();
        strategies.insert("/*/*", Some("inline-source"));
        let decisions = decide_merge(&base, &local, &remote, &strategies).unwrap();
        assert!(decisions.iter().any(|d| d
            .remote_diff
            .as_ref()
            .is_some_and(|diff| diff.iter().any(|e| matches!(e.op, Op::ParentDeleted)))));
        let reread: Vec<_> = decisions
            .iter()
            .map(|d| MergeDecision::from_value(&d.to_value()).unwrap())
            .collect();
        assert_eq!(reread, decisions);
        assert_eq!(
            apply_decisions(&base, &reread).unwrap(),
            Value::from("for i in range(10):\nbbabbbbbbbbbaaabc\u{b}")
        );
    }

    #[test]
    fn clear_remove_and_take_max() {
        let rep = |n: i64| format!(r#"[{{"op":"replace","key":"k","value":{n}}}]"#);
        let clear = decision(&[key("d")], Action::Clear, &rep(1), &rep(2));
        let remove = decision(&[key("e")], Action::Remove, &rep(1), &rep(2));
        let max = decision(&[key("f")], Action::TakeMax, &rep(5), &rep(3));
        let out = apply(
            r#"{"d":{"k":[1]},"e":{"k":1,"z":2},"f":{"k":4}}"#,
            &[clear, remove, max],
        )
        .unwrap();
        assert_eq!(out, v(r#"{"d":{"k":[]},"e":{"z":2},"f":{"k":5}}"#));
        let bad = decision(
            &[key("f")],
            Action::TakeMax,
            &rep(5),
            r#"[{"op":"remove","key":"k"}]"#,
        );
        assert!(apply(r#"{"f":{"k":4}}"#, &[bad]).is_err());
        let text = decision(
            &[],
            Action::TakeMax,
            r#"[{"op":"replace","key":"k","value":"b"}]"#,
            r#"[{"op":"replace","key":"k","value":2}]"#,
        );
        assert!(apply(r#"{"k":1}"#, &[text]).is_err());
        assert!(apply(
            r#"{"k":1}"#,
            &[decision(&[], Action::Other("nope".into()), "", "")]
        )
        .is_err());
    }

    #[test]
    fn take_max_compares_like_python() {
        assert_eq!(
            py_compare(&v("[1,2]"), &v("[1,3]")).unwrap(),
            Some(Ordering::Less)
        );
        assert_eq!(
            py_compare(&v("2"), &v("1.5")).unwrap(),
            Some(Ordering::Greater)
        );
        assert_eq!(
            py_compare(&v("1.5"), &v("2")).unwrap(),
            Some(Ordering::Less)
        );
        assert_eq!(
            py_compare(&v("true"), &v("1")).unwrap(),
            Some(Ordering::Equal)
        );
        assert_eq!(py_compare(&v("NaN"), &v("1")).unwrap(), None);
        assert_eq!(
            py_compare(&v("100000000000000000000"), &v("1e20")).unwrap(),
            Some(Ordering::Equal)
        );
        assert_eq!(
            py_compare(&v("100000000000000000001"), &v("1e20")).unwrap(),
            Some(Ordering::Greater)
        );
        assert!(py_compare(&v("null"), &v("1")).is_err());
        assert!(py_compare(&v("\"a\""), &v("1")).is_err());
    }

    #[test]
    fn line_decisions_split_off_the_string_path() {
        let patch_line = r#"[{"op":"addrange","key":1,"valuelist":["new\n"]}]"#;
        let d = decision(&[key("s"), Key::Index(0)], Action::Local, "", "");
        let mut d = d;
        d.local_diff = Some(diff(patch_line));
        // The line diff `addrange 1` is wrapped under line 0, i.e. patch(0, ...).
        let out = apply(r#"{"s":"a\nb\n"}"#, &[d]).unwrap();
        assert_eq!(out, v(r#"{"s":"anew\n\nb\n"}"#));
    }

    #[test]
    fn push_and_pop_decisions_round_trip() {
        let l = r#"[{"op":"replace","key":"x","value":1}]"#;
        let d = decision(&[key("a"), key("b")], Action::Local, l, "");
        let pushed = push_patch_decision(&d, &[key("a"), key("b")]).unwrap();
        assert!(pushed.common_path.is_empty());
        let popped = pop_all_patch_decisions(pushed);
        assert_eq!(popped.common_path, [key("a"), key("b")]);
        assert_eq!(popped.local_diff, d.local_diff);
        assert!(push_patch_decision(&d, &[key("zz")]).is_err());
    }

    #[test]
    fn filter_decisions_matches_starred_prefixes() {
        let l = r#"[{"op":"patch","key":"source","diff":[]}]"#;
        let decisions = [
            decision(&[key("cells"), Key::Index(1)], Action::Local, l, ""),
            decision(&[key("metadata")], Action::Local, l, ""),
        ];
        assert_eq!(filter_decisions("/cells/*/source", &decisions, false), [0]);
        assert_eq!(filter_decisions("/cells", &decisions, true), [0]);
    }

    #[test]
    fn build_diffs_nests_branches_at_the_root() {
        let base = v(r#"{"a":{"x":1},"b":[1,2]}"#);
        let da = decision(
            &[key("a")],
            Action::Local,
            r#"[{"op":"remove","key":"x"}]"#,
            "",
        );
        let db = decision(
            &[key("b")],
            Action::Local,
            r#"[{"op":"removerange","key":0,"length":1}]"#,
            "",
        );
        let built = build_diffs(&base, &[da, db], Which::Local)
            .unwrap()
            .unwrap();
        let patched = patch(&base, &built).unwrap();
        assert_eq!(patched, v(r#"{"a":{},"b":[2]}"#));
        assert_eq!(build_diffs(&base, &[], Which::Local).unwrap(), None);
    }
}
