//! The three-way merge of lists, dicts and strings from the diffs of both sides.

use std::collections::BTreeMap;

use crate::diff_format::{Diff, DiffEntry, Key, Op};
use crate::diffing::{self, diff_with_config, DiffConfig, NotebookDiffConfig};
use crate::path::star_path;
use crate::py::splitlines_keepends;
use crate::value::Value;

use super::chunks::{chunk_typename, make_merge_chunks, Chunk};
use super::decisions::{diffs_eq, MergeDecision, MergeDecisionBuilder};
use super::strategies::{
    resolve_conflicted_decisions_dict, resolve_conflicted_decisions_list,
    resolve_conflicted_decisions_strings, resolve_strategy_generic, resolve_strategy_inline_source,
    MarkerIds, Strategies,
};
use super::{assertion, invalid, MergeError};

/// Strategies that can keep a deleted parent alive by inserting a marker into it.
const COUNTERING_STRATEGIES: [&str; 1] = ["inline-source"];

/// Whether every change in `diff` is to a transient path.
fn is_diff_all_transients(diff: &[DiffEntry], path: &[Key], strategies: &Strategies) -> bool {
    diff.iter().all(|d| {
        let subpath = child(path, &d.key);
        let transient = strategies.is_transient(&star_path(&subpath));
        match &d.op {
            // Below a transient path nothing else needs to be marked transient.
            Op::Patch { diff } => transient || is_diff_all_transients(diff, &subpath, strategies),
            _ => transient,
        }
    })
}

/// Whether any change in `diff` is to a value whose strategy can counter the deletion of its
/// parent.
fn will_diff_counter_parent_deletion(
    diff: &[DiffEntry],
    path: &[Key],
    strategies: &Strategies,
) -> bool {
    diff.iter().any(|d| {
        let subpath = child(path, &d.key);
        if strategies
            .get(&star_path(&subpath))
            .is_some_and(|s| COUNTERING_STRATEGIES.contains(&s))
        {
            return true;
        }
        match &d.op {
            Op::Patch { diff } => will_diff_counter_parent_deletion(diff, &subpath, strategies),
            _ => false,
        }
    })
}

/// The diff that stands in for a deleted parent: `parent_deleted` entries where a strategy
/// can counter the deletion, and patches that lead to them.
fn create_parent_deletion_counter_diff(
    diff: &[DiffEntry],
    path: &[Key],
    strategies: &Strategies,
) -> Diff {
    let mut newdiff = Vec::new();
    for d in diff {
        let subpath = child(path, &d.key);
        if strategies
            .get(&star_path(&subpath))
            .is_some_and(|s| COUNTERING_STRATEGIES.contains(&s))
        {
            newdiff.push(DiffEntry::parent_deleted(d.key.clone()));
        } else if let Op::Patch { diff } = &d.op {
            // Only recurse if no strategy matched here.
            let subdiff = create_parent_deletion_counter_diff(diff, &subpath, strategies);
            newdiff.push(DiffEntry::patch(d.key.clone(), subdiff));
        }
    }
    newdiff
}

fn child(path: &[Key], key: &Key) -> Vec<Key> {
    let mut p = Vec::with_capacity(path.len() + 1);
    p.extend_from_slice(path);
    p.push(key.clone());
    p
}

fn one(e: &DiffEntry) -> Option<Diff> {
    Some(vec![e.clone()])
}

fn owned(entries: &[&DiffEntry]) -> Option<Diff> {
    Some(entries.iter().map(|e| (*e).clone()).collect())
}

fn patch_diff(e: &DiffEntry) -> Result<&[DiffEntry], MergeError> {
    match &e.op {
        Op::Patch { diff } => Ok(diff),
        _ => assertion("expected a patch entry"),
    }
}

fn valuelist(e: &DiffEntry) -> Result<&Value, MergeError> {
    match &e.op {
        Op::AddRange {
            valuelist: list @ Value::List(_),
        } => Ok(list),
        _ => invalid("an addrange of a list is expected"),
    }
}

fn index_key(e: &DiffEntry) -> Result<usize, MergeError> {
    e.key
        .as_index()
        .map_or_else(|| invalid("diff key must be an integer"), Ok)
}

/// `items[start:end]` with Python's clamping.
fn slice(items: &[Value], start: usize, end: usize) -> &[Value] {
    let end = end.min(items.len());
    if start >= end {
        &[]
    } else {
        &items[start..end]
    }
}

fn addrange(key: usize, items: &[Value]) -> Diff {
    vec![DiffEntry::addrange(key, Value::List(items.to_vec()))]
}

/// One side of a merge: a diff, or the marker that the parent of the value was deleted.
#[derive(Clone, Copy)]
enum Side<'a> {
    Diff(&'a [DiffEntry]),
    ParentDeleted,
}

impl<'a> Side<'a> {
    fn diff(self) -> Result<&'a [DiffEntry], MergeError> {
        match self {
            Side::Diff(d) => Ok(d),
            Side::ParentDeleted => invalid("a parent-deleted marker is not a diff"),
        }
    }

    fn as_option(self) -> Option<&'a [DiffEntry]> {
        match self {
            Side::Diff(d) => Some(d),
            Side::ParentDeleted => None,
        }
    }
}

struct Merger<'a> {
    strategies: &'a Strategies,
    config: &'a DiffConfig,
    /// Set while the lines of a string are merged as a list: a line is never merged further.
    merging_lines: bool,
    ids: MarkerIds,
}

impl Merger<'_> {
    fn merge(
        &mut self,
        base: Option<&Value>,
        local: Side<'_>,
        remote: Side<'_>,
        path: &[Key],
    ) -> Result<MergeDecisionBuilder, MergeError> {
        match base {
            Some(Value::Object(map)) => self.merge_dicts(map, local.diff()?, remote.diff()?, path),
            Some(Value::List(items)) => {
                self.merge_lists(items, local.diff()?, remote.diff()?, path)
            }
            Some(Value::Str(s)) => self.merge_strings(s, local, remote, path),
            _ => invalid("Cannot handle merge of this type."),
        }
    }

    fn merge_dicts(
        &mut self,
        base: &crate::value::Map,
        local_diff: &[DiffEntry],
        remote_diff: &[DiffEntry],
        path: &[Key],
    ) -> Result<MergeDecisionBuilder, MergeError> {
        let spath = star_path(path);
        let strategies = self.strategies;
        let dict_strategy = strategies.get(&spath);
        let mut decisions = MergeDecisionBuilder::new();

        // The last entry of a key wins, as when upstream builds a dict from the diff.
        let by_key = |diff: &'_ [DiffEntry]| -> Result<BTreeMap<String, DiffEntry>, MergeError> {
            diff.iter()
                .map(|e| match &e.key {
                    Key::Name(name) => Ok((name.clone(), e.clone())),
                    Key::Index(_) => invalid("dict diff keys must be strings"),
                })
                .collect()
        };
        let (local, remote) = (by_key(local_diff)?, by_key(remote_diff)?);

        // Keys with no change keep their base values and need no decision. One-sided changes:
        for key in local
            .keys()
            .chain(remote.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let (ld, rd) = (local.get(key), remote.get(key));
            if ld.is_some() != rd.is_some() {
                decisions.onesided(path.to_vec(), ld.and_then(one), rd.and_then(one))?;
            }
        }

        // Changes on both sides.
        for (key, ld) in &local {
            let Some(rd) = remote.get(key) else { continue };
            let bv = base.get(key);
            let item_path = child(path, &Key::Name(key.clone()));
            let item_strategy = strategies.get(&format!("{spath}/{key}"));

            if matches!(ld.op, Op::ParentDeleted) {
                // Recurse to apply the strategy that counters the parent deletion.
                let Op::Patch { diff } = &rd.op else {
                    return assertion("assert rd.op == DiffOp.PATCH");
                };
                let sub = self.merge(bv, Side::ParentDeleted, Side::Diff(diff), &item_path)?;
                decisions.extend(sub);
            } else if matches!(rd.op, Op::ParentDeleted) {
                let Op::Patch { diff } = &ld.op else {
                    return assertion("assert ld.op == DiffOp.PATCH");
                };
                let sub = self.merge(bv, Side::Diff(diff), Side::ParentDeleted, &item_path)?;
                decisions.extend(sub);
            } else if matches!(ld.op, Op::Remove) || matches!(rd.op, Op::Remove) {
                if matches!(ld.op, Op::Remove) && matches!(rd.op, Op::Remove) {
                    // Removed on both sides.
                    decisions.agreement(path.to_vec(), one(ld), one(rd))?;
                } else if matches!(ld.op, Op::Remove)
                    && is_diff_all_transients(std::slice::from_ref(rd), path, strategies)
                {
                    // One side deleted and the other changed transients only: take the deletion.
                    decisions.local(path.to_vec(), one(ld), one(rd), false, None)?;
                } else if matches!(rd.op, Op::Remove)
                    && is_diff_all_transients(std::slice::from_ref(ld), path, strategies)
                {
                    decisions.remote(path.to_vec(), one(ld), one(rd), false, None)?;
                } else {
                    // Removed on one side and edited on the other.
                    decisions.conflict(path.to_vec(), one(ld), one(rd), item_strategy)?;
                }
            } else if std::mem::discriminant(&ld.op) != std::mem::discriminant(&rd.op) {
                // Edited in different ways.
                decisions.conflict(path.to_vec(), one(ld), one(rd), item_strategy)?;
            } else if super::decisions::entries_eq(ld, rd) {
                decisions.agreement(path.to_vec(), one(ld), one(rd))?;
            } else {
                match (&ld.op, &rd.op) {
                    // Different inserts or replacements.
                    (Op::Add { .. }, _) | (Op::Replace { .. }, _) => {
                        decisions.conflict(path.to_vec(), one(ld), one(rd), item_strategy)?;
                    }
                    // Both patched: merge the substructures.
                    (Op::Patch { diff: l }, Op::Patch { diff: r }) => {
                        let sub = self.merge(bv, Side::Diff(l), Side::Diff(r), &item_path)?;
                        decisions.extend(sub);
                    }
                    _ => {
                        return invalid(format!(
                            "Invalid diff ops {} and {}.",
                            ld.op.name(),
                            rd.op.name()
                        ))
                    }
                }
            }
        }

        resolve_conflicted_decisions_dict(path, base, &mut decisions, dict_strategy)?;
        Ok(decisions)
    }

    /// Compares two inserted value lists, and splits the decisions on similarity.
    ///
    /// The diff of the two lists aligns what they have in common by the notebook predicates:
    /// identical aligned inserts agree, similar ones are conflicts that autoresolve may merge,
    /// and everything else is a conflict-free insert by one side.
    fn split_addrange(
        &self,
        key: usize,
        local: &Value,
        remote: &Value,
        path: &[Key],
        item_strategy: Option<&str>,
    ) -> Result<MergeDecisionBuilder, MergeError> {
        let (Value::List(l), Value::List(r)) = (local, remote) else {
            return invalid("inserted values must be lists");
        };
        let intermediate = diff_with_config(local, remote, &star_path(path), self.config)?;

        let mut decisions = MergeDecisionBuilder::new();
        let mut taken = 0usize;
        // Offset between diff keys (referring to local) and remote.
        let mut offset = 0isize;
        let mut i = 0;
        while i < intermediate.len() {
            let d = &intermediate[i];
            let dkey = index_key(d)?;
            if dkey < taken {
                return assertion("assert d.key >= taken");
            }

            // No diff in [taken, d.key): the elements are inserted on both sides.
            if taken < dkey {
                let overlap = addrange(key, &l[taken..dkey]);
                decisions.agreement(path.to_vec(), Some(overlap.clone()), Some(overlap))?;
                taken = dkey;
            }

            // Either (1) conflicted, (2) local one-sided, or (3) remote one-sided. A removal
            // at the same key as the current entry makes a range substitution.
            let next_removal = intermediate.get(i + 1).and_then(|n| match n.op {
                Op::RemoveRange { length } if n.key == d.key => Some(length),
                _ => None,
            });
            if let Some(local_len) = next_removal {
                // A non-similar subsequence according to the predicates: a conflicted insert.
                let Op::AddRange {
                    valuelist: Value::List(inserted),
                } = &d.op
                else {
                    return invalid("an addrange of a list is expected");
                };
                let ld = addrange(key, slice(l, dkey, dkey + local_len));
                let rd = vec![DiffEntry::addrange(key, Value::List(inserted.clone()))];
                decisions.conflict(path.to_vec(), Some(ld), Some(rd), item_strategy)?;
                offset += inserted.len() as isize - local_len as isize;
                // This treats two entries in one go, so `taken` passes the key of the next.
                taken += local_len;
                i += 1;
            } else {
                match &d.op {
                    Op::Replace { value } => {
                        let Some(item) = l.get(dkey) else {
                            return invalid("list index out of range");
                        };
                        let ld = addrange(key, std::slice::from_ref(item));
                        let rd = addrange(key, std::slice::from_ref(value));
                        decisions.conflict(path.to_vec(), Some(ld), Some(rd), item_strategy)?;
                        taken += 1;
                    }
                    Op::Remove | Op::RemoveRange { .. } => {
                        // Local one-sided.
                        let vl: &[Value] = match &d.op {
                            Op::RemoveRange { length } => slice(l, dkey, dkey + length),
                            _ => match l.get(dkey) {
                                Some(item) => std::slice::from_ref(item),
                                None => return invalid("list index out of range"),
                            },
                        };
                        decisions.onesided(
                            path.to_vec(),
                            Some(addrange(key, vl)),
                            Some(Vec::new()),
                        )?;
                        offset -= vl.len() as isize;
                        taken += vl.len();
                    }
                    Op::Add { .. } | Op::AddRange { .. } => {
                        // Remote one-sided.
                        let vl: &[Value] = match &d.op {
                            Op::AddRange {
                                valuelist: Value::List(items),
                            } => items,
                            Op::Add { value } => std::slice::from_ref(value),
                            _ => return invalid("an addrange of a list is expected"),
                        };
                        decisions.onesided(path.to_vec(), None, Some(addrange(key, vl)))?;
                        offset += vl.len() as isize;
                    }
                    Op::Patch { .. } => {
                        // The predicates say that the local and remote items are similar.
                        let remote_index = dkey as isize + offset;
                        let remote_item = usize::try_from(remote_index).ok().and_then(|j| r.get(j));
                        let (Some(litem), Some(ritem)) = (l.get(dkey), remote_item) else {
                            return invalid("list index out of range");
                        };
                        decisions.similar_insert(
                            path.to_vec(),
                            Some(addrange(key, std::slice::from_ref(litem))),
                            Some(addrange(key, std::slice::from_ref(ritem))),
                            vec![d.clone()],
                            item_strategy,
                        )?;
                        taken += 1;
                    }
                    Op::ParentDeleted => return invalid("Invalid diff op: parent_deleted"),
                }
            }
            i += 1;
        }

        // Equal items at the end are not part of the diff.
        if taken < l.len() {
            let local_items = &l[taken..];
            let start = taken as isize - l.len() as isize + r.len() as isize;
            let start = if start < 0 {
                (start + r.len() as isize).max(0)
            } else {
                start
            } as usize;
            let remote_items = r.get(start..).unwrap_or(&[]);
            let equal = local_items.len() == remote_items.len()
                && local_items
                    .iter()
                    .zip(remote_items)
                    .all(|(a, b)| a.py_eq_item(b));
            if !equal {
                return assertion("assert local_items == remote_items");
            }
            let overlap = addrange(key, local_items);
            decisions.agreement(path.to_vec(), Some(overlap.clone()), Some(overlap))?;
        }
        Ok(decisions)
    }

    /// Merges concurrent inserts, with an optional `removerange` after each: splits the
    /// insertions into agreements, one-sided inserts and conflicts.
    fn merge_concurrent_inserts(
        &self,
        ldiff: &[DiffEntry],
        rdiff: &[DiffEntry],
        path: &[Key],
        item_strategy: Option<&str>,
    ) -> Result<MergeDecisionBuilder, MergeError> {
        let shaped = |d: &[DiffEntry]| {
            (1..=2).contains(&d.len())
                && matches!(d[0].op, Op::AddRange { .. })
                && (d.len() == 1 || matches!(d[1].op, Op::RemoveRange { .. }))
        };
        if !(shaped(ldiff) && shaped(rdiff)) {
            return assertion("unexpected shape of concurrent inserts");
        }
        let mut subdecisions = self.split_addrange(
            index_key(&ldiff[0])?,
            valuelist(&ldiff[0])?,
            valuelist(&rdiff[0])?,
            path,
            item_strategy,
        )?;

        // Conflicts in the inserts and removals after them: the whole range is conflicted.
        if subdecisions.has_conflicted() && (ldiff.len() == 2 || rdiff.len() == 2) {
            let mut decisions = MergeDecisionBuilder::new();
            decisions.conflict(
                path.to_vec(),
                Some(ldiff.to_vec()),
                Some(rdiff.to_vec()),
                item_strategy,
            )?;
            return Ok(decisions);
        }
        // Otherwise the merged inserts stand, and removals after them are conflict free.
        if ldiff.len() == 2 && rdiff.len() == 2 {
            let (Op::RemoveRange { length: l }, Op::RemoveRange { length: r }) =
                (&ldiff[1].op, &rdiff[1].op)
            else {
                unreachable!("shape checked above");
            };
            if l != r {
                return assertion("assert ldiff[1].length == rdiff[1].length");
            }
            subdecisions.agreement(
                path.to_vec(),
                Some(ldiff[1..].to_vec()),
                Some(rdiff[1..].to_vec()),
            )?;
        } else if ldiff.len() == 2 || rdiff.len() == 2 {
            subdecisions.onesided(
                path.to_vec(),
                Some(ldiff[1..].to_vec()),
                Some(rdiff[1..].to_vec()),
            )?;
        }
        Ok(subdecisions)
    }

    fn merge_lists(
        &mut self,
        base: &[Value],
        local_diff: &[DiffEntry],
        remote_diff: &[DiffEntry],
        path: &[Key],
    ) -> Result<MergeDecisionBuilder, MergeError> {
        let strategies = self.strategies;
        let spath = star_path(path);
        let list_strategy = strategies.get(&spath);
        let item_strategy = strategies.get(&format!("{spath}/*"));

        let mut decisions = MergeDecisionBuilder::new();
        let chunks = make_merge_chunks(base.len(), local_diff, remote_diff, false)?;

        // Loop over chunks of base[key:chunk_end], with the insertion at key part of the chunk.
        for Chunk {
            start: key,
            local: d0,
            remote: d1,
            ..
        } in chunks
        {
            let item_path = child(path, &Key::Index(key));

            // Each side's addrange, and its patch or removerange: 0 or 1 entries of each.
            let (a0, p0): (Vec<&DiffEntry>, Vec<&DiffEntry>) =
                d0.iter().partition(|e| e.is_addrange());
            let (a1, p1): (Vec<&DiffEntry>, Vec<&DiffEntry>) =
                d1.iter().partition(|e| e.is_addrange());

            let (laname, lpname) = chunk_typename(&d0);
            let (raname, rpname) = chunk_typename(&d1);
            // `/`, `A/`, `/A`, `A/A` by the addranges, `P/R` by the patches and removals, and
            // both together in `chunktype`.
            let achunktype = format!("{laname}/{raname}");
            let pchunktype = format!("{lpname}/{rpname}");
            let chunktype = format!("{laname}{lpname}/{raname}{rpname}");

            if chunktype == "/" {
                continue;
            }
            // One-sided modification of the chunk.
            if d0.is_empty() || d1.is_empty() {
                decisions.onesided(path.to_vec(), Some(d0), Some(d1))?;
                continue;
            }
            // Exactly the same modifications.
            if diffs_eq(&d0, &d1) {
                decisions.agreement(path.to_vec(), Some(d0), Some(d1))?;
                continue;
            }
            // Chunking makes two-sided removals agree above.
            if chunktype == "R/R" {
                continue;
            }

            if matches!(pchunktype.as_str(), "P/P" | "P/R" | "R/P") {
                // Insertions come first.
                if achunktype == "A/A" {
                    let sub = self.merge_concurrent_inserts(
                        &d0[..a0.len()],
                        &d1[..a1.len()],
                        path,
                        item_strategy,
                    )?;
                    decisions.extend(sub);
                } else if matches!(achunktype.as_str(), "A/" | "/A") {
                    decisions.onesided(path.to_vec(), owned(&a0), owned(&a1))?;
                }

                // Then the patches and removals.
                if p0.len() == p1.len()
                    && p0
                        .iter()
                        .zip(&p1)
                        .all(|(x, y)| super::decisions::entries_eq(x, y))
                {
                    decisions.agreement(path.to_vec(), owned(&p0), owned(&p1))?;
                } else if pchunktype == "P/P" {
                    let sub = self.merge(
                        base.get(key),
                        Side::Diff(patch_diff(p0[0])?),
                        Side::Diff(patch_diff(p1[0])?),
                        &item_path,
                    )?;
                    decisions.extend(sub);
                } else {
                    // A patch against a removal.
                    let thediff = match (&p0[0].op, &p1[0].op) {
                        (Op::Patch { diff }, _) | (_, Op::Patch { diff }) => diff.as_slice(),
                        _ => return invalid("Unexpected op combination at this point."),
                    };
                    for p in [p0[0], p1[0]] {
                        // Otherwise a decision to remove the rest of the items is missing.
                        if matches!(p.op, Op::RemoveRange { length } if length != 1) {
                            return assertion("assert p.length == 1");
                        }
                    }
                    let local_removes = matches!(p0[0].op, Op::RemoveRange { .. });
                    let remote_removes = matches!(p1[0].op, Op::RemoveRange { .. });

                    // What a patch of a deleted item comes to:
                    //  - only transient changes: discard them (generic behavior)
                    //  - a strategy that counters the deletion: recurse and take the deleted
                    //    parent into account (inline behavior)
                    //  - otherwise a conflict on the parent (mergetool behavior)
                    let is_transient = is_diff_all_transients(thediff, &item_path, strategies);
                    if local_removes && is_transient {
                        decisions.local(path.to_vec(), owned(&p0), owned(&p1), false, None)?;
                    } else if remote_removes && is_transient {
                        decisions.remote(path.to_vec(), owned(&p0), owned(&p1), false, None)?;
                    } else if list_strategy == Some("use-base") {
                        decisions.base(path.to_vec(), owned(&p0), owned(&p1), false, None);
                    } else if list_strategy == Some("use-local") {
                        decisions.local(path.to_vec(), owned(&p0), owned(&p1), false, None)?;
                    } else if list_strategy == Some("use-remote") {
                        decisions.remote(path.to_vec(), owned(&p0), owned(&p1), false, None)?;
                    } else if will_diff_counter_parent_deletion(thediff, &item_path, strategies) {
                        // Keep the deleted item and let strategies for its parts record the
                        // conflict.
                        let counter =
                            create_parent_deletion_counter_diff(thediff, &item_path, strategies);
                        let (ld, rd) = if local_removes {
                            (counter.as_slice(), thediff)
                        } else {
                            (thediff, counter.as_slice())
                        };
                        let sub =
                            self.merge(base.get(key), Side::Diff(ld), Side::Diff(rd), &item_path)?;
                        decisions.extend(sub);
                    } else {
                        // A conflict on the parent, and leave it at that.
                        decisions.conflict(path.to_vec(), owned(&p0), owned(&p1), item_strategy)?;
                    }
                }
            } else if matches!(chunktype.as_str(), "A/P" | "A/R") {
                // An insert before a patch or removal: apply both, marked as conflicted.
                let (l, r) = (Some(d0), Some(d1));
                if decisions.tryresolve(path, &l, &r, item_strategy)?.is_none() {
                    decisions.local_then_remote(path.to_vec(), l, r, true, None)?;
                }
            } else if matches!(chunktype.as_str(), "P/A" | "R/A") {
                let (l, r) = (Some(d0), Some(d1));
                if decisions.tryresolve(path, &l, &r, item_strategy)?.is_none() {
                    decisions.remote_then_local(path.to_vec(), l, r, true, None)?;
                }
            } else if matches!(chunktype.as_str(), "A/AP" | "AP/A") {
                // Merge the inserts, and add the one-sided patch after them.
                let sub = self.merge_concurrent_inserts(
                    &d0[..a0.len()],
                    &d1[..a1.len()],
                    path,
                    item_strategy,
                )?;
                decisions.extend(sub);
                decisions.onesided(path.to_vec(), owned(&p0), owned(&p1))?;
            } else if matches!(chunktype.as_str(), "AR/R" | "R/AR") {
                // The same removal on both sides, with an insert just before it on one.
                decisions.onesided(path.to_vec(), owned(&a0), owned(&a1))?;
                decisions.agreement(path.to_vec(), owned(&p0), owned(&p1))?;
            } else if matches!(chunktype.as_str(), "AR/A" | "A/AR" | "A/A" | "AR/AR") {
                // Removals are merged together with the inserts.
                let sub = self.merge_concurrent_inserts(&d0, &d1, path, item_strategy)?;
                decisions.extend(sub);
            } else {
                return assertion(format!("Unhandled chunk conflict type {chunktype}"));
            }
        }

        resolve_conflicted_decisions_list(
            path,
            base,
            &mut decisions,
            list_strategy,
            &mut self.ids,
        )?;
        Ok(decisions)
    }

    fn merge_strings(
        &mut self,
        base: &str,
        local: Side<'_>,
        remote: Side<'_>,
        path: &[Key],
    ) -> Result<MergeDecisionBuilder, MergeError> {
        let mut decisions = MergeDecisionBuilder::new();

        if self.merging_lines {
            // `base` is a line with edits on both sides. Merging by character would be
            // unreliable and conflict with the line chunking, so it is a conflict on the line.
            let (Side::Diff(l), Side::Diff(r)) = (local, remote) else {
                return invalid("a parent-deleted marker is not a diff");
            };
            let Some((line, parent)) = path.split_last() else {
                return invalid("string index out of range");
            };
            let strategy = self.strategies.get(&star_path(parent));
            decisions.conflict(
                parent.to_vec(),
                Some(vec![DiffEntry::patch(line.clone(), l.to_vec())]),
                Some(vec![DiffEntry::patch(line.clone(), r.to_vec())]),
                strategy,
            )?;
            return Ok(decisions);
        }

        let strategy = self.strategies.get(&star_path(path));
        match strategy {
            // Avoid resolving the diffs ourselves and leave the work to the merge renderer.
            Some("inline-source") => {
                decisions = resolve_strategy_inline_source(
                    path,
                    base,
                    local.as_option(),
                    remote.as_option(),
                )?;
            }
            Some("union") => {
                let (Side::Diff(l), Side::Diff(r)) = (local, remote) else {
                    return invalid("a parent-deleted marker is not a diff");
                };
                decisions.local_then_remote(
                    path.to_vec(),
                    Some(l.to_vec()),
                    Some(r.to_vec()),
                    false,
                    None,
                )?;
            }
            _ => {
                // Merge the lines as a list.
                let (Side::Diff(l), Side::Diff(r)) = (local, remote) else {
                    return invalid("a parent-deleted marker is not a diff");
                };
                let lines: Vec<Value> = splitlines_keepends(base)
                    .into_iter()
                    .map(Value::from)
                    .collect();
                self.merging_lines = true;
                let merged = self.merge_lists(&lines, l, r, path);
                self.merging_lines = false;
                decisions = merged?;
            }
        }
        resolve_conflicted_decisions_strings(path, &mut decisions, strategy);
        Ok(decisions)
    }
}

/// Merges `base` three ways given the diffs from the base to the local and remote versions.
///
/// `local` and `remote` only matter for the ids that marker cells must not repeat: the diffs
/// carry everything else. Inserted items are aligned
/// with the default notebook predicates, whatever the diffs were made with (upstream copies the
/// notebook diff configuration, which loses any configured ignores).
pub fn decide_merge_with_diff(
    base: &Value,
    local: &Value,
    remote: &Value,
    local_diff: &[DiffEntry],
    remote_diff: &[DiffEntry],
    strategies: &Strategies,
) -> Result<Vec<MergeDecision>, MergeError> {
    let notebook_config = NotebookDiffConfig::new();
    let mut merger = Merger {
        strategies,
        config: notebook_config.config(),
        merging_lines: false,
        ids: MarkerIds::default(),
    };
    for notebook in [base, local, remote] {
        merger.ids.avoid_notebook(notebook);
    }
    let mut decisions = merger.merge(
        Some(base),
        Side::Diff(local_diff),
        Side::Diff(remote_diff),
        &[],
    )?;
    // The root strategy resolves what is left.
    resolve_strategy_generic(&[], &mut decisions, strategies.get("/"));
    Ok(decisions.validated())
}

/// Three-way merge of the same kind of collection (list, dict or string): the decisions that
/// make up the merge of `local` and `remote` against `base`.
///
/// Applying the decisions to the base gives the merged value. If any decision is marked
/// `conflict`, the merged value uses the suggested action, which may not be right.
pub fn decide_merge(
    base: &Value,
    local: &Value,
    remote: &Value,
    strategies: &Strategies,
) -> Result<Vec<MergeDecision>, MergeError> {
    let local_diff = diffing::diff(base, local)?;
    let remote_diff = diffing::diff(base, remote)?;
    decide_merge_with_diff(base, local, remote, &local_diff, &remote_diff, strategies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;
    use crate::merging::apply_decisions;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    fn merged(base: &str, local: &str, remote: &str) -> (Value, Vec<MergeDecision>) {
        merged_with(base, local, remote, &Strategies::new())
    }

    fn merged_with(
        base: &str,
        local: &str,
        remote: &str,
        strategies: &Strategies,
    ) -> (Value, Vec<MergeDecision>) {
        let (b, l, r) = (v(base), v(local), v(remote));
        let decisions = decide_merge(&b, &l, &r, strategies).unwrap();
        (apply_decisions(&b, &decisions).unwrap(), decisions)
    }

    #[test]
    fn independent_dict_changes_merge_without_conflict() {
        let (m, d) = merged(
            r#"{"a":1,"b":2,"c":3}"#,
            r#"{"a":9,"b":2,"c":3}"#,
            r#"{"a":1,"b":2}"#,
        );
        assert_eq!(m, v(r#"{"a":9,"b":2}"#));
        assert!(d.iter().all(|d| !d.conflict));
    }

    #[test]
    fn replace_replace_conflicts_unless_equal() {
        let (m, d) = merged(r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#);
        assert_eq!(m, v(r#"{"a":1}"#));
        assert_eq!(d.len(), 1);
        assert!(d[0].conflict);
        let (m, d) = merged(r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":2.0}"#);
        assert_eq!(m, v(r#"{"a":2}"#));
        assert!(!d[0].conflict);
    }

    #[test]
    fn strategies_resolve_conflicts() {
        let mut s = Strategies::new();
        s.insert("/a", Some("use-remote"));
        let (m, d) = merged_with(r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#, &s);
        assert_eq!(m, v(r#"{"a":3}"#));
        assert!(!d[0].conflict);
        let mut s = Strategies::new();
        s.insert("/", Some("use-local"));
        let (m, _) = merged_with(r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#, &s);
        assert_eq!(m, v(r#"{"a":2}"#));
        let mut s = Strategies::new();
        s.insert("/a", Some("fail"));
        let (b, l, r) = (v(r#"{"a":1}"#), v(r#"{"a":2}"#), v(r#"{"a":3}"#));
        assert!(matches!(
            decide_merge(&b, &l, &r, &s),
            Err(MergeError::Runtime(_))
        ));
    }

    #[test]
    fn list_inserts_on_both_sides_are_combined() {
        let (m, d) = merged("[1,2,3]", "[0,1,2,3]", "[1,2,3,4]");
        assert_eq!(m, v("[0,1,2,3,4]"));
        assert!(d.iter().all(|d| !d.conflict));
    }

    #[test]
    fn different_inserts_at_one_place_conflict_with_both_applied_by_union() {
        let (m, d) = merged("[1]", "[1,2]", "[1,3]");
        assert!(d.iter().any(|d| d.conflict));
        assert_eq!(m, v("[1]"));
        let mut s = Strategies::new();
        s.insert("/", Some("union"));
        let (m, _) = merged_with("[1]", "[1,2]", "[1,3]", &s);
        assert_eq!(m, v("[1,2,3]"));
    }

    #[test]
    fn patch_against_removal_conflicts_and_transients_give_way() {
        let (m, d) = merged(r#"{"a":{"k":1}}"#, r#"{"a":{"k":2}}"#, "{}");
        assert!(d.iter().any(|d| d.conflict));
        assert_eq!(m, v(r#"{"a":{"k":1}}"#));
        let mut s = Strategies::new();
        s.transients.push("/a/k".into());
        let (m, d) = merged_with(r#"{"a":{"k":1}}"#, r#"{"a":{"k":2}}"#, "{}", &s);
        assert_eq!(m, v("{}"));
        assert!(d.iter().all(|d| !d.conflict));
    }

    #[test]
    fn strings_merge_by_line() {
        let (m, d) = merged(r#""a\nb\nc\n""#, r#""A\nb\nc\n""#, r#""a\nb\nC\n""#);
        assert_eq!(m, v(r#""A\nb\nC\n""#));
        assert!(d.iter().all(|d| !d.conflict));
        // Two edits of one line conflict on that line.
        let (m, d) = merged(r#""a\nb\n""#, r#""x\nb\n""#, r#""y\nb\n""#);
        assert_eq!(m, v(r#""a\nb\n""#));
        assert!(d.iter().any(|d| d.conflict));
    }

    #[test]
    fn empty_collections_fail_like_upstream() {
        let (e, strategies) = (v("\"\""), Strategies::new());
        assert!(matches!(
            decide_merge(&e, &e, &e, &strategies),
            Err(MergeError::Assertion(_))
        ));
        let l = v("[]");
        assert!(matches!(
            decide_merge(&l, &l, &l, &strategies),
            Err(MergeError::Assertion(_))
        ));
        let n = v("1");
        assert!(decide_merge(&n, &n, &n, &strategies).is_err());
    }

    #[test]
    fn marker_ids_do_not_collide_with_cells() {
        let strategies = Strategies::new();
        let b = v(r#"{"cells":[]}"#);
        let l = v(r#"{"cells":[{"cell_type":"code","source":"a"}]}"#);
        let r = v(r#"{"cells":[{"cell_type":"code","source":"zzzzzzzzzzzzzzz"}]}"#);
        let d = decide_merge(&b, &l, &r, &strategies).unwrap();
        assert_eq!(d.len(), 1);
        assert!(d[0].conflict);
    }
}
