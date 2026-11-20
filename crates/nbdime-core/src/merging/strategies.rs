//! Conflict resolution strategies: the table that maps paths to strategies, and the code that
//! resolves the conflicts recorded at one level with the strategy of that level.

use std::collections::{BTreeSet, HashMap};

use crate::diff_format::{Diff, DiffEntry, Key, Op};
use crate::nbformat::IdGenerator;
use crate::patch::patch;
use crate::path::star_path_str;
use crate::value::{Map, Value};

use super::chunks::chunk_typename;
use super::decisions::{
    push_patch_decision, resolve_path_chars, Action, MergeDecision, MergeDecisionBuilder,
};
use super::{assertion, invalid, MergeError};
use crate::prettyprint::render::merge_render;
use crate::prettyprint::PrettyPrintConfig;

/// Strategies by starred path, plus the paths whose changes never count as conflicts.
///
/// Paths are looked up in starred form: an index (or a key that reads as an integer) matches
/// `*`. The keys given to [`Strategies::insert`] are not normalized, so they should be written
/// starred.
#[derive(Clone, Debug, Default)]
pub struct Strategies {
    map: HashMap<String, String>,
    /// Starred paths of values that change without meaning (execution counts and the like).
    pub transients: Vec<String>,
    /// Unused by the merge itself; kept for API compatibility.
    pub fall_back: Option<String>,
}

impl Strategies {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the strategy of `path`; `None` removes it.
    pub fn insert(&mut self, path: &str, strategy: Option<&str>) {
        match strategy {
            Some(s) => {
                self.map.insert(path.to_owned(), s.to_owned());
            }
            None => {
                self.map.remove(path);
            }
        }
    }

    /// The strategy for `path` (any path; it is starred first).
    pub fn get(&self, path: &str) -> Option<&str> {
        self.map.get(&star_path_str(path)).map(String::as_str)
    }

    pub fn is_transient(&self, starred_path: &str) -> bool {
        self.transients.iter().any(|t| t == starred_path)
    }
}

/// Rewrites diffs in canonical form where only one patch applies to one key and the entries
/// are sorted by key.
pub fn combine_patches(diffs: Diff) -> Result<Diff, MergeError> {
    let mut patches: HashMap<Key, usize> = HashMap::new();
    let mut newdiffs: Diff = Vec::with_capacity(diffs.len());
    for d in diffs {
        match d.op {
            Op::Patch { diff } => match patches.get(&d.key) {
                None => {
                    patches.insert(d.key.clone(), newdiffs.len());
                    newdiffs.push(DiffEntry::patch(d.key, combine_patches(diff)?));
                }
                Some(&i) => {
                    let Op::Patch { diff: existing } = &mut newdiffs[i].op else {
                        unreachable!("patches maps keys to patch entries");
                    };
                    let mut joined = std::mem::take(existing);
                    joined.extend(diff);
                    *existing = combine_patches(joined)?;
                }
            },
            op => newdiffs.push(DiffEntry { key: d.key, op }),
        }
    }
    sort_by_key(&mut newdiffs)?;
    Ok(newdiffs)
}

/// A stable sort by key; mixing integer and string keys is a `TypeError`, as in Python.
fn sort_by_key(diff: &mut Diff) -> Result<(), MergeError> {
    let indices = diff.iter().filter(|e| e.key.as_index().is_some()).count();
    if indices != 0 && indices != diff.len() {
        return invalid("'<' not supported between instances of 'str' and 'int'");
    }
    diff.sort_by(|a, b| match (&a.key, &b.key) {
        (Key::Index(x), Key::Index(y)) => x.cmp(y),
        (Key::Name(x), Key::Name(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    });
    Ok(())
}

/// The diff of a decision, after checking that its path starts with `target_path`.
fn adjust_patch_level<'a>(
    target_path: &[Key],
    common_path: &[Key],
    diff: &'a Option<Diff>,
) -> Result<&'a Diff, MergeError> {
    if !common_path.starts_with(target_path) {
        return assertion("decision path does not start with the target path");
    }
    match diff {
        Some(d) => Ok(d),
        None => invalid("'NoneType' object is not iterable"),
    }
}

/// The local and the remote diffs of `decisions` as two canonical diffs.
fn collect_diffs<'a>(
    path: &[Key],
    decisions: impl IntoIterator<Item = &'a MergeDecision>,
) -> Result<(Diff, Diff), MergeError> {
    let (mut local, mut remote) = (Vec::new(), Vec::new());
    for d in decisions {
        local.extend(
            adjust_patch_level(path, &d.common_path, &d.local_diff)?
                .iter()
                .cloned(),
        );
        remote.extend(
            adjust_patch_level(path, &d.common_path, &d.remote_diff)?
                .iter()
                .cloned(),
        );
    }
    Ok((combine_patches(local)?, combine_patches(remote)?))
}

/// The diffs of the conflicted decisions only, not combined.
fn collect_conflicting_diffs(
    path: &[Key],
    decisions: &[MergeDecision],
) -> Result<(Diff, Diff), MergeError> {
    let (mut local, mut remote) = (Vec::new(), Vec::new());
    for d in decisions.iter().filter(|d| d.conflict) {
        local.extend(
            adjust_patch_level(path, &d.common_path, &d.local_diff)?
                .iter()
                .cloned(),
        );
        remote.extend(
            adjust_patch_level(path, &d.common_path, &d.remote_diff)?
                .iter()
                .cloned(),
        );
    }
    Ok((local, remote))
}

/// Groups decisions on the items of the list at `base_path` by item index, in index order.
///
/// Decisions below an item have their path pushed into their diffs, so that all of them
/// are at the list.
fn bundle_decisions_by_index(
    base_path: &[Key],
    decisions: &[MergeDecision],
) -> Result<Vec<(Key, Vec<MergeDecision>)>, MergeError> {
    let level = base_path.len();
    let mut positions: HashMap<Key, usize> = HashMap::new();
    let mut groups: Vec<(Key, Vec<MergeDecision>)> = Vec::new();
    for d in decisions {
        if d.common_path.get(..level) != Some(base_path) {
            return assertion("decision has incorrect base path");
        }
        let (key, d) = if d.common_path.len() > level {
            let prefix = &d.common_path[level..];
            (prefix[0].clone(), push_patch_decision(d, prefix)?)
        } else {
            let mut keys: Vec<&Key> = Vec::new();
            let diffs = [
                d.local_diff.as_ref(),
                d.remote_diff.as_ref(),
                d.custom_diff.as_ref(),
            ];
            for (i, diff) in diffs.into_iter().enumerate() {
                let diff = match (diff, i) {
                    (Some(diff), _) => diff.as_slice(),
                    (None, 2) => &[],
                    (None, _) => return invalid("'NoneType' object is not iterable"),
                };
                for e in diff {
                    if !keys.contains(&&e.key) {
                        keys.push(&e.key);
                    }
                }
            }
            let [key] = keys.as_slice() else {
                return assertion("expected the diffs to share one key");
            };
            ((*key).clone(), d.clone())
        };
        let i = *positions.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(d);
    }
    if groups.iter().any(|(k, _)| k.as_index().is_none())
        && groups.iter().any(|(k, _)| k.as_index().is_some())
    {
        return invalid("'<' not supported between instances of 'str' and 'int'");
    }
    groups.sort_by(|a, b| match (&a.0, &b.0) {
        (Key::Index(x), Key::Index(y)) => x.cmp(y),
        (Key::Name(x), Key::Name(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    });
    Ok(groups)
}

/// Ids for the cells that conflict markers add. They are derived from the marker's text and
/// position (findings F-021) and avoid every id the notebook is known to have.
pub struct MarkerIds {
    generator: IdGenerator,
}

impl Default for MarkerIds {
    fn default() -> Self {
        MarkerIds {
            generator: IdGenerator::new(false),
        }
    }
}

impl MarkerIds {
    /// Marks the id of `cell`, or of each cell if it is a list of them, as taken.
    fn avoid(&mut self, cell: &Value) {
        match cell {
            Value::List(items) => items.iter().for_each(|c| self.avoid(c)),
            _ => {
                if let Some(Value::Str(id)) = cell.get("id") {
                    self.generator.taken.insert(id.clone());
                }
            }
        }
    }

    /// Marks every cell id of the notebook `nb` as taken.
    pub(super) fn avoid_notebook(&mut self, nb: &Value) {
        if let Some(cells) = nb.get("cells") {
            self.avoid(cells);
        }
    }

    /// Marks the ids that `diff` introduces (in inserted or replaced cells, and replaced ids) as
    /// taken.
    fn avoid_diff(&mut self, diff: &[DiffEntry]) {
        for e in diff {
            match &e.op {
                Op::Add { value } | Op::Replace { value } => match (&e.key, value) {
                    (Key::Name(name), Value::Str(id)) if name == "id" => {
                        self.generator.taken.insert(id.clone());
                    }
                    _ => self.avoid(value),
                },
                Op::AddRange { valuelist } => self.avoid(valuelist),
                Op::Patch { diff } => self.avoid_diff(diff),
                Op::Remove | Op::RemoveRange { .. } | Op::ParentDeleted => {}
            }
        }
    }

    /// Marks the ids of `cells` and every id that `decisions` bring in as taken.
    fn avoid_all(&mut self, cells: &[Value], decisions: &[MergeDecision]) {
        cells.iter().for_each(|c| self.avoid(c));
        for d in decisions {
            let diffs = [&d.local_diff, &d.remote_diff, &d.custom_diff];
            diffs
                .into_iter()
                .flatten()
                .for_each(|diff| self.avoid_diff(diff));
        }
    }

    /// A markdown marker cell. `position` says where it goes: the insert key and the number of
    /// markers made before it.
    fn cell_marker(&mut self, text: &str, position: (usize, usize)) -> Value {
        let source = format!("<span style=\"color:red\"><b>{text}</b></span>");
        let mut cell = Map::new();
        cell.insert("cell_type".into(), Value::from("markdown"));
        cell.insert("source".into(), Value::from(source));
        cell.insert("metadata".into(), Value::Object(Map::new()));
        let mut hashed = Map::new();
        hashed.insert("cell".into(), Value::Object(cell.clone()));
        hashed.insert(
            "position".into(),
            Value::List(vec![Value::from(position.0), Value::from(position.1)]),
        );
        let id = self.generator.generate(&Value::Object(hashed));
        let mut marked = Map::new();
        marked.insert("id".into(), Value::from(id));
        marked.extend(cell);
        Value::Object(marked)
    }
}

/// A stderr stream output carrying `text`.
fn output_marker(text: String) -> Value {
    let mut m = Map::new();
    m.insert("output_type".into(), Value::from("stream"));
    m.insert("name".into(), Value::from("stderr"));
    m.insert("text".into(), Value::from(text));
    Value::Object(m)
}

fn name_of(key: &Key) -> Result<&str, MergeError> {
    match key {
        Key::Name(n) => Ok(n),
        Key::Index(_) => invalid("sequence item expected str instance, int found"),
    }
}

/// The outputs to show for one side of a conflicting output, and a note on what changed.
fn get_outputs_and_note(
    base: Option<&Value>,
    removes: &[&DiffEntry],
    patches: &[&DiffEntry],
) -> Result<(Vec<Value>, String), MergeError> {
    if !removes.is_empty() {
        return Ok((Vec::new(), " <removed>".into()));
    }
    let Some(&e) = patches.first() else {
        return Ok((
            vec![base.cloned().unwrap_or(Value::Null)],
            " <unchanged>".into(),
        ));
    };
    if patches.len() != 1 {
        return invalid("too many values to unpack (expected 1)");
    }
    let Op::Patch { diff: ediff } = &e.op else {
        unreachable!("patches holds patch entries");
    };

    // Which mime types are modified, and which other keys.
    let mut mkeys: BTreeSet<&str> = BTreeSet::new();
    let mut has_keys = false;
    for d in ediff {
        if matches!(&d.key, Key::Name(n) if n == "data") {
            let Op::Patch { diff } = &d.op else {
                return assertion("assert d.op == DiffOp.PATCH");
            };
            for f in diff {
                mkeys.insert(name_of(&f.key)?);
            }
        } else {
            has_keys = true;
        }
    }
    let Some(base_map) = base.and_then(Value::as_object) else {
        return invalid("'NoneType' object has no attribute 'get'");
    };
    let ukeys: BTreeSet<&str> = match base_map.get("data") {
        Some(data) if data.truthy() => {
            let Value::Object(data) = data else {
                return invalid("object has no attribute 'keys'");
            };
            data.keys()
                .map(String::as_str)
                .filter(|k| !mkeys.contains(k))
                .collect()
        }
        _ => BTreeSet::new(),
    };

    let mut notes = Vec::new();
    if !mkeys.is_empty() || has_keys {
        notes.push(format!(
            "modified: {}",
            mkeys.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    if !ukeys.is_empty() {
        notes.push(format!(
            "unchanged: {}",
            ukeys.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    let note = if notes.is_empty() {
        String::new()
    } else {
        format!(" <{}>", notes.join("; "))
    };
    Ok((vec![patch(base.expect("checked above"), ediff)?], note))
}

/// Outputs with conflict markers for conflicting changes to one base output, and whether the
/// base output itself stays (when both sides only insert next to it).
fn make_inline_output_conflict(
    base_output: Option<&Value>,
    local_diff: &[DiffEntry],
    remote_diff: &[DiffEntry],
) -> Result<(Vec<Value>, bool), MergeError> {
    let (m0, m1, m2) = ("<".repeat(7), "=".repeat(7), ">".repeat(7));
    fn split(d: &[DiffEntry]) -> Result<[Vec<&DiffEntry>; 3], MergeError> {
        let pick = |f: fn(&Op) -> bool| d.iter().filter(|e| f(&e.op)).collect::<Vec<_>>();
        let parts = [
            pick(|op| matches!(op, Op::Patch { .. })),
            pick(|op| matches!(op, Op::AddRange { .. })),
            pick(|op| matches!(op, Op::RemoveRange { .. })),
        ];
        if parts.iter().map(Vec::len).sum::<usize>() != d.len() {
            return assertion("unexpected op in an output conflict");
        }
        Ok(parts)
    }
    let [lpatches, linserts, lremoves] = split(local_diff)?;
    let [rpatches, rinserts, rremoves] = split(remote_diff)?;

    let mut outputs = Vec::new();

    // Inserts from both sides, kept apart.
    if !linserts.is_empty() || !rinserts.is_empty() {
        let items = |inserts: &[&DiffEntry]| -> Result<Vec<Value>, MergeError> {
            let mut out = Vec::new();
            for e in inserts {
                let Op::AddRange { valuelist } = &e.op else {
                    unreachable!("inserts holds addranges");
                };
                match valuelist {
                    Value::List(items) => out.extend(items.iter().cloned()),
                    _ => return invalid("addrange of outputs needs a list"),
                }
            }
            Ok(out)
        };
        outputs.push(output_marker(format!("{m0} local\n")));
        outputs.extend(items(&linserts)?);
        outputs.push(output_marker(format!("{m1}\n")));
        outputs.extend(items(&rinserts)?);
        outputs.push(output_marker(format!("{m2} remote\n")));
    }

    // Keep the base output if both sides only insert.
    let keep_base =
        lremoves.is_empty() && rremoves.is_empty() && lpatches.is_empty() && rpatches.is_empty();
    if !lremoves.is_empty() && !rremoves.is_empty() {
        // Both removed it: nothing to add.
    } else if !keep_base {
        if !lremoves.is_empty() && !lpatches.is_empty() {
            return assertion("a side both removes and patches an output");
        }
        if !rremoves.is_empty() && !rpatches.is_empty() {
            return assertion("a side both removes and patches an output");
        }
        let (loutputs, lnote) = get_outputs_and_note(base_output, &lremoves, &lpatches)?;
        let (routputs, rnote) = get_outputs_and_note(base_output, &rremoves, &rpatches)?;
        outputs.push(output_marker(format!("{m0} local{lnote}\n")));
        outputs.extend(loutputs);
        outputs.push(output_marker(format!("{m1}\n")));
        outputs.extend(routputs);
        outputs.push(output_marker(format!("{m2} remote{rnote}\n")));
    }
    Ok((outputs, keep_base))
}

/// Cells with conflict markers around conflicting, dissimilar inserts (and replacements).
fn make_inline_cell_conflict(
    base_cells: &[Value],
    local_diff: &[DiffEntry],
    remote_diff: &[DiffEntry],
    ids: &mut MarkerIds,
    marker_count: &mut usize,
) -> Result<Vec<Value>, MergeError> {
    let well_formed = |d: &[DiffEntry]| {
        (1..=2).contains(&d.len())
            && matches!(d[0].op, Op::AddRange { .. })
            && (d.len() == 1 || matches!(d[1].op, Op::RemoveRange { .. }))
    };
    if !well_formed(local_diff) || !well_formed(remote_diff) {
        return assertion("unexpected shape of an inline cell conflict");
    }
    let removed = |d: &[DiffEntry]| match d.get(1).map(|e| &e.op) {
        Some(Op::RemoveRange { length }) => *length,
        _ => 0,
    };
    let (lremove, rremove) = (removed(local_diff), removed(remote_diff));
    let Some(start) = local_diff[0].key.as_index() else {
        return invalid("diff key must be an integer");
    };
    let lkeep = lremove.saturating_sub(rremove);
    let rkeep = rremove.saturating_sub(lremove);

    let side = |diff: &[DiffEntry], keep: usize| -> Result<Vec<Value>, MergeError> {
        let Op::AddRange {
            valuelist: Value::List(inserted),
        } = &diff[0].op
        else {
            return invalid("addrange of cells needs a list");
        };
        let base_slice = base_cells.get(start..).unwrap_or(&[]).iter().take(keep);
        Ok(inserted.iter().chain(base_slice).cloned().collect())
    };
    let (lcells, rcells) = (side(local_diff, lkeep)?, side(remote_diff, rkeep)?);

    let mut marker = |text: &str| {
        let cell = ids.cell_marker(text, (start, *marker_count));
        *marker_count += 1;
        cell
    };
    let (m0, m1, m2) = ("<".repeat(7), "=".repeat(7), ">".repeat(7));
    let mut cells = Vec::with_capacity(lcells.len() + rcells.len() + 3);
    cells.push(marker(&format!("{m0} local")));
    cells.extend(lcells);
    cells.push(marker(&m1));
    cells.extend(rcells);
    cells.push(marker(&format!("{m2} remote")));
    Ok(cells)
}

fn is_remove(d: &DiffEntry) -> bool {
    matches!(d.op, Op::Remove)
}

/// Conflicts on attachments: keep the other side's edit of a removal, and store both versions
/// of two edits under new names.
pub fn resolve_strategy_inline_attachments(
    base_path: &[Key],
    attachments: &Map,
    decisions: &mut MergeDecisionBuilder,
) -> Result<(), MergeError> {
    let strategy = "inline-attachments";
    let (local_conflicts, remote_conflicts) =
        collect_conflicting_diffs(base_path, &decisions.decisions)?;
    decisions.decisions.retain(|d| !d.conflict);

    // The last entry of a key wins, as when upstream builds a dict from them.
    let by_key = |diffs: Vec<DiffEntry>| -> HashMap<Key, DiffEntry> {
        diffs.into_iter().map(|e| (e.key.clone(), e)).collect()
    };
    let (lbykey, rbykey) = (by_key(local_conflicts), by_key(remote_conflicts));
    let mut keys: Vec<&Key> = lbykey
        .keys()
        .chain(rbykey.keys().filter(|k| !lbykey.contains_key(*k)))
        .collect();
    if keys.iter().any(|k| k.as_index().is_some()) && keys.iter().any(|k| k.as_index().is_none()) {
        return invalid("'<' not supported between instances of 'int' and 'str'");
    }
    keys.sort_by(|a, b| match (a, b) {
        (Key::Index(x), Key::Index(y)) => x.cmp(y),
        (Key::Name(x), Key::Name(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    });

    for key in keys {
        let (Some(ld), Some(rd)) = (lbykey.get(key), rbykey.get(key)) else {
            return invalid(format!("KeyError: {key}"));
        };
        if is_remove(ld) {
            if is_remove(rd) {
                return assertion("assert rd.op != DiffOp.REMOVE");
            }
            // The other side edited what this side removed: keep the edit, but flag it.
            decisions.remote(
                base_path.to_vec(),
                Some(vec![ld.clone()]),
                Some(vec![rd.clone()]),
                true,
                Some(strategy.into()),
            )?;
        } else if is_remove(rd) {
            decisions.local(
                base_path.to_vec(),
                Some(vec![ld.clone()]),
                Some(vec![rd.clone()]),
                true,
                Some(strategy.into()),
            )?;
        } else {
            // Not merging attachment contents, but adding attachments with new names
            // LOCAL_oldname and REMOTE_oldname instead.
            let name = name_of(key)?;
            let Some(base) = attachments.get(name) else {
                return invalid(format!("KeyError: {name:?}"));
            };
            let version = |d: &DiffEntry, other: &DiffEntry| -> Result<Value, MergeError> {
                match &d.op {
                    Op::Add { value } => {
                        if !matches!(other.op, Op::Add { .. }) {
                            return assertion("assert rd.op == DiffOp.ADD");
                        }
                        Ok(value.clone())
                    }
                    Op::Replace { value } => Ok(value.clone()),
                    Op::Patch { diff } => Ok(patch(base, diff)?),
                    _ => assertion("assert ld.op == DiffOp.PATCH"),
                }
            };
            let local = version(ld, rd)?;
            let remote = match &rd.op {
                Op::Add { value } | Op::Replace { value } => value.clone(),
                Op::Patch { diff } => patch(base, diff)?,
                _ => return assertion("assert rd.op == DiffOp.PATCH"),
            };
            let (local_name, remote_name) = (format!("LOCAL_{name}"), format!("REMOTE_{name}"));
            let put = |new_name: String, value: Value| {
                if attachments.contains_key(&new_name) {
                    DiffEntry::replace(new_name, value)
                } else {
                    DiffEntry::add(new_name, value)
                }
            };
            let custom_diff = vec![put(local_name, local), put(remote_name, remote)];
            decisions.custom(
                base_path.to_vec(),
                Some(vec![ld.clone()]),
                Some(vec![rd.clone()]),
                custom_diff,
                true,
                Some(strategy.into()),
            );
        }
    }
    Ok(())
}

/// Replaces the conflicted decisions on outputs by decisions that remove the conflicting
/// output (a no-op when both sides insert).
pub fn resolve_strategy_remove_outputs(
    base_path: &[Key],
    decisions: &mut MergeDecisionBuilder,
) -> Result<(), MergeError> {
    let strategy = "remove";
    let groups = bundle_decisions_by_index(base_path, &decisions.decisions)?;
    decisions.decisions = Vec::new();
    for (key, decs) in groups {
        if !decs.iter().any(|d| d.conflict) {
            decisions.decisions.extend(decs);
            continue;
        }
        let (local_diff, remote_diff) = collect_diffs(base_path, &decs)?;
        let both_insert = matches!(
            (local_diff.as_slice(), remote_diff.as_slice()),
            ([l], [r]) if matches!(l.op, Op::AddRange { .. }) && matches!(r.op, Op::AddRange { .. })
        );
        let custom_diff = match (both_insert, &key) {
            (true, _) => Vec::new(),
            (false, Key::Index(i)) => vec![DiffEntry::removerange(*i, 1)],
            (false, Key::Name(_)) => return invalid("list key must be integer"),
        };
        decisions.custom(
            base_path.to_vec(),
            Some(local_diff),
            Some(remote_diff),
            custom_diff,
            false,
            Some(strategy.into()),
        );
    }
    Ok(())
}

/// Replaces the conflicted decisions on outputs by markers around both versions.
pub fn resolve_strategy_inline_outputs(
    base_path: &[Key],
    outputs: &[Value],
    decisions: &mut MergeDecisionBuilder,
) -> Result<(), MergeError> {
    let strategy = "inline-outputs";
    let groups = bundle_decisions_by_index(base_path, &decisions.decisions)?;
    decisions.decisions = Vec::new();
    for (key, decs) in groups {
        if !decs.iter().any(|d| d.conflict) {
            decisions.decisions.extend(decs);
            continue;
        }
        let Key::Index(index) = key else {
            return invalid("list index must be an integer");
        };
        let (local_diff, remote_diff) = collect_diffs(base_path, &decs)?;
        // A conflict appended after the last output has no base output (F-027).
        let base_output = outputs.get(index);
        let (inlined, keep_base) =
            make_inline_output_conflict(base_output, &local_diff, &remote_diff)?;
        let mut custom_diff = vec![DiffEntry::addrange(index, Value::List(inlined))];
        if !keep_base {
            custom_diff.push(DiffEntry::removerange(index, 1));
        }
        decisions.custom(
            base_path.to_vec(),
            Some(local_diff),
            Some(remote_diff),
            custom_diff,
            true,
            Some(strategy.into()),
        );
    }
    Ok(())
}

/// Drops the conflicted decisions of a dict and records their diffs in its `nbdime-conflicts`.
pub fn resolve_strategy_record_conflicts(
    base_path: &[Key],
    base: &Map,
    decisions: &mut MergeDecisionBuilder,
) -> Result<(), MergeError> {
    let strategy = "record-conflict";
    let pushed = decisions
        .decisions
        .iter()
        .map(|d| {
            let prefix = d.common_path.get(base_path.len()..).unwrap_or(&[]);
            push_patch_decision(d, prefix)
        })
        .collect::<Result<Vec<_>, _>>()?;
    decisions.decisions = pushed;

    let (local_conflicts, remote_conflicts) =
        collect_conflicting_diffs(base_path, &decisions.decisions)?;
    let local_conflicts = combine_patches(local_conflicts)?;
    let remote_conflicts = combine_patches(remote_conflicts)?;
    decisions.decisions.retain(|d| !d.conflict);

    let mut conflicts = Map::new();
    conflicts.insert(
        "local_diff".into(),
        crate::diff_format::diff_to_value(&local_conflicts),
    );
    conflicts.insert(
        "remote_diff".into(),
        crate::diff_format::diff_to_value(&remote_conflicts),
    );
    let conflicts = Value::Object(conflicts);
    let op = if base.contains_key("nbdime-conflicts") {
        DiffEntry::replace("nbdime-conflicts", conflicts)
    } else {
        DiffEntry::add("nbdime-conflicts", conflicts)
    };
    decisions.custom(
        base_path.to_vec(),
        Some(local_conflicts),
        Some(remote_conflicts),
        vec![op],
        true,
        Some(strategy.into()),
    );
    Ok(())
}

/// A cell source whose cell was deleted on one side: the other side's edits keep the cell and
/// a marker line says what happened. Otherwise both versions are merged as text.
pub fn resolve_strategy_inline_source(
    path: &[Key],
    base: &str,
    local_diff: Option<&[DiffEntry]>,
    remote_diff: Option<&[DiffEntry]>,
) -> Result<MergeDecisionBuilder, MergeError> {
    let strategy = Some("inline-source".to_owned());
    let mut decisions = MergeDecisionBuilder::new();
    let marker = |text: &str| vec![DiffEntry::addrange(0, Value::List(vec![Value::from(text)]))];
    match (local_diff, remote_diff) {
        // A side that is `None` stands for the parent-deleted marker.
        (None, Some(remote)) => decisions.local_then_remote(
            path.to_vec(),
            Some(marker("<<<<<<< LOCAL CELL DELETED >>>>>>>\n")),
            Some(remote.to_vec()),
            true,
            strategy,
        )?,
        (Some(local), None) => decisions.remote_then_local(
            path.to_vec(),
            Some(local.to_vec()),
            Some(marker("<<<<<<< REMOTE CELL DELETED >>>>>>>\n")),
            true,
            strategy,
        )?,
        (Some(local_diff), Some(remote_diff)) => {
            let local = patch(&Value::from(base), local_diff)?;
            let remote = patch(&Value::from(base), remote_diff)?;
            let (Value::Str(local), Value::Str(remote)) = (&local, &remote) else {
                return invalid("a string patch gives a string");
            };
            let (merged, status) =
                merge_render(base, local, remote, None, &PrettyPrintConfig::default())?;
            if path.last() != Some(&Key::from("source")) {
                return assertion("assert path[-1] == \"source\"");
            }
            decisions.custom(
                path[..path.len() - 1].to_vec(),
                Some(vec![DiffEntry::patch("source", local_diff.to_vec())]),
                Some(vec![DiffEntry::patch("source", remote_diff.to_vec())]),
                vec![DiffEntry::replace("source", Value::from(merged))],
                status != 0,
                strategy,
            );
        }
        (None, None) => return invalid("cannot merge two deleted parents"),
    }
    Ok(decisions)
}

/// Resolves conflicting inserts of cells with markers around the two versions, or by merging
/// the cells that are similar.
fn resolve_strategy_inline_recurse(
    path: &[Key],
    base: &[Value],
    decisions: &mut MergeDecisionBuilder,
    ids: &mut MarkerIds,
) -> Result<(), MergeError> {
    let strategy = "inline-cells";
    let old_decisions = std::mem::take(&mut decisions.decisions);
    ids.avoid_all(base, &old_decisions);
    let mut marker_count = 0;

    for d in old_decisions {
        if !d.conflict {
            decisions.decisions.push(d);
            continue;
        }
        let (Some(local_diff), Some(remote_diff)) = (d.local_diff.clone(), d.remote_diff.clone())
        else {
            return assertion("assert d.local_diff and d.remote_diff");
        };
        if local_diff.is_empty() || remote_diff.is_empty() {
            return assertion("assert d.local_diff and d.remote_diff");
        }
        let (laname, lpname) = chunk_typename(&local_diff);
        let (raname, rpname) = chunk_typename(&remote_diff);
        let chunktype = format!("{laname}{lpname}/{raname}{rpname}");
        let cells_path = [Key::from("cells")];
        if !matches!(chunktype.as_str(), "AR/A" | "A/AR" | "A/A" | "AR/AR")
            || d.common_path != cells_path
        {
            decisions.decisions.push(d);
            continue;
        }

        let Some(lr_diff) = d.similar_insert.clone() else {
            // Inserts not similar, cannot recurse. Markup block.
            let cells =
                make_inline_cell_conflict(base, &local_diff, &remote_diff, ids, &mut marker_count)?;
            let rdiff: Vec<DiffEntry> = if local_diff.len() > 1 {
                vec![local_diff[1].clone()]
            } else if remote_diff.len() > 1 {
                vec![remote_diff[1].clone()]
            } else {
                Vec::new()
            };
            let Some(start) = local_diff[0].key.as_index() else {
                return invalid("diff key must be an integer");
            };
            let mut custom_diff = vec![DiffEntry::addrange(start, Value::List(cells))];
            custom_diff.extend(rdiff);
            decisions.custom(
                path.to_vec(),
                Some(local_diff),
                Some(remote_diff),
                custom_diff,
                true,
                Some(strategy.into()),
            );
            continue;
        };

        // Should only be insert range vs insert range here now.
        if chunktype != "A/A" {
            return assertion(format!("Unexpected chunk type: {chunktype:?}"));
        }
        let single = |diff: &[DiffEntry]| match &diff[0].op {
            Op::AddRange {
                valuelist: Value::List(items),
            } if items.len() == 1 => Some(items[0].clone()),
            _ => None,
        };
        let (Some(lcell), Some(rcell)) = (single(&local_diff), single(&remote_diff)) else {
            return assertion(
                "Unexpected diff length. Expected both local and remote inserts to have length 1, as they are assumed similar.",
            );
        };
        let [DiffEntry {
            op: Op::Patch { diff: changes },
            ..
        }, ..] = lr_diff.as_slice()
        else {
            return assertion("assert lr_diff[0].op == 'patch'");
        };
        let (Value::Object(lmap), Value::Object(rmap)) = (&lcell, &rcell) else {
            return invalid("cells must be dicts");
        };
        let (Some(ltype), Some(rtype)) = (lmap.get("cell_type"), rmap.get("cell_type")) else {
            return invalid("KeyError: 'cell_type'");
        };
        if !ltype.py_eq(rtype) {
            return assertion("cell types cannot differ");
        }

        let keys: Vec<&Key> = changes.iter().map(|e| &e.key).collect();
        let changed = |k: &String| keys.iter().any(|c| matches!(c, Key::Name(n) if n == k));
        let mut cell = Map::new();
        for (k, v) in lmap {
            if !changed(k) {
                // Either identical, or one-way from local.
                cell.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in rmap {
            if !changed(k) && !lmap.contains_key(k) {
                // One-way from remote.
                cell.insert(k.clone(), v.clone());
            }
        }
        for key in keys {
            let name = name_of(key)?;
            let value = match name {
                "source" => {
                    let (Some(Value::Str(l)), Some(Value::Str(r))) =
                        (lmap.get(name), rmap.get(name))
                    else {
                        return invalid("cell sources must be strings");
                    };
                    Value::from(merge_render("", l, r, None, &PrettyPrintConfig::default())?.0)
                }
                "metadata" | "id" => {
                    let (Some(l), Some(r)) = (lmap.get(name), rmap.get(name)) else {
                        return invalid(format!("KeyError: {name:?}"));
                    };
                    let mut m = Map::new();
                    m.insert(format!("local_{name}"), l.clone());
                    m.insert(format!("remote_{name}"), r.clone());
                    Value::Object(m)
                }
                "execution_count" => Value::Null,
                "outputs" => Value::List(Vec::new()),
                other => return invalid(format!("Conflict on unrecognized key: {other:?}")),
            };
            cell.insert(name.to_owned(), value);
        }

        let Some(start) = local_diff[0].key.as_index() else {
            return invalid("diff key must be an integer");
        };
        decisions.custom(
            path.to_vec(),
            Some(local_diff),
            Some(remote_diff),
            vec![DiffEntry::addrange(
                start,
                Value::List(vec![Value::Object(cell)]),
            )],
            true,
            Some(strategy.into()),
        );
    }
    Ok(())
}

fn should_resolve(strategy: Option<&str>, decisions: &MergeDecisionBuilder) -> Option<String> {
    let strategy = strategy.filter(|s| !s.is_empty() && *s != "mergetool")?;
    decisions.has_conflicted().then(|| strategy.to_owned())
}

/// Resolves the conflicts that no strategy has been applied to, for the `use-*` strategies.
pub fn resolve_strategy_generic(
    _path: &[Key],
    decisions: &mut MergeDecisionBuilder,
    strategy: Option<&str>,
) {
    let Some(strategy) = should_resolve(strategy, decisions) else {
        return;
    };
    if strategy.starts_with("use-") {
        // For example `use-local` on /cells turns a patch/remove conflict on /cells/3 into the
        // action `local`. Conflicts a strategy has already been applied to are left alone.
        let action = Action::parse(&strategy.replace("use-", ""));
        for d in &mut decisions.decisions {
            if d.conflict && d.strategy.as_deref().is_none_or(str::is_empty) {
                d.action = action.clone();
                d.conflict = false;
            }
        }
    }
    // Any other strategy is only logged upstream.
}

/// Resolves the conflicts recorded for a list with the list's strategy.
pub fn resolve_conflicted_decisions_list(
    path: &[Key],
    base: &[Value],
    decisions: &mut MergeDecisionBuilder,
    strategy: Option<&str>,
    ids: &mut MarkerIds,
) -> Result<(), MergeError> {
    let Some(strategy) = should_resolve(strategy, decisions) else {
        return Ok(());
    };
    match strategy.as_str() {
        "inline-outputs" => resolve_strategy_inline_outputs(path, base, decisions)?,
        "inline-cells" => resolve_strategy_inline_recurse(path, base, decisions, ids)?,
        "remove" => resolve_strategy_remove_outputs(path, decisions)?,
        "union" => {
            for d in &mut decisions.decisions {
                if d.conflict {
                    // Not applied to subdecisions on dicts.
                    let sub = d.common_path.get(path.len()..).unwrap_or(&[]);
                    let is_dict = match sub.split_first() {
                        None => false,
                        Some((Key::Index(i), rest)) => match base.get(*i) {
                            Some(item) => resolve_path_chars(item, rest)?.is_object(),
                            None => return invalid("list index out of range"),
                        },
                        Some(_) => return invalid("list indices must be integers"),
                    };
                    if !is_dict {
                        d.action = Action::LocalThenRemote;
                        d.conflict = false;
                    }
                }
            }
        }
        "clear-all" => {
            // Drop every decision and remove the whole range instead.
            let (local_diff, remote_diff) = collect_diffs(path, &decisions.decisions)?;
            decisions.decisions = Vec::new();
            decisions.custom(
                path.to_vec(),
                Some(local_diff),
                Some(remote_diff),
                vec![DiffEntry::removerange(0, base.len())],
                false,
                Some("clear-all".into()),
            );
        }
        // Only reached for the lines of a string, resolved with the string's strategy.
        "clear" => {}
        _ => resolve_strategy_generic(path, decisions, Some(&strategy)),
    }
    Ok(())
}

/// Resolves the conflicts recorded for a dict with the dict's strategy.
pub fn resolve_conflicted_decisions_dict(
    path: &[Key],
    base: &Map,
    decisions: &mut MergeDecisionBuilder,
    strategy: Option<&str>,
) -> Result<(), MergeError> {
    let Some(strategy) = should_resolve(strategy, decisions) else {
        return Ok(());
    };
    match strategy.as_str() {
        "record-conflict" => resolve_strategy_record_conflicts(path, base, decisions)?,
        "inline-attachments" => resolve_strategy_inline_attachments(path, base, decisions)?,
        // The strategy cannot be applied to dicts; upstream only logs it.
        "union" => {}
        _ => resolve_strategy_generic(path, decisions, Some(&strategy)),
    }
    Ok(())
}

/// Resolves the conflicts recorded for a string with the string's strategy.
pub fn resolve_conflicted_decisions_strings(
    path: &[Key],
    decisions: &mut MergeDecisionBuilder,
    strategy: Option<&str>,
) {
    let Some(strategy) = should_resolve(strategy, decisions) else {
        return;
    };
    match strategy.as_str() {
        "clear" => {
            for d in &mut decisions.decisions {
                if d.conflict && d.strategy.as_deref().is_none_or(str::is_empty) {
                    d.action = Action::Clear;
                    d.conflict = false;
                }
            }
        }
        // Resolved by the strategy itself.
        "inline-source" => {}
        _ => resolve_strategy_generic(path, decisions, Some(&strategy)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::diff_from_value;
    use crate::json::parse;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    fn diff(text: &str) -> Diff {
        diff_from_value(&v(text)).unwrap()
    }

    fn key(s: &str) -> Key {
        Key::from(s)
    }

    #[test]
    fn strategy_lookup_stars_the_path() {
        let mut s = Strategies::new();
        s.insert("/cells/*/source", Some("inline-source"));
        s.insert("/", Some("use-local"));
        s.insert("/gone", Some("use-base"));
        s.insert("/gone", None);
        assert_eq!(s.get("/cells/12/source"), Some("inline-source"));
        assert_eq!(s.get("cells/x/source"), None);
        assert_eq!(s.get("//"), Some("use-local"));
        assert_eq!(s.get("/gone"), None);
        s.transients.push("/cells/*/execution_count".into());
        assert!(s.is_transient("/cells/*/execution_count"));
    }

    #[test]
    fn combine_patches_merges_patches_per_key_and_sorts() {
        let d = diff(
            r#"[{"op":"removerange","key":3,"length":1},
                {"op":"patch","key":1,"diff":[{"op":"remove","key":"a"}]},
                {"op":"addrange","key":0,"valuelist":["x"]},
                {"op":"patch","key":1,"diff":[{"op":"remove","key":"b"}]}]"#,
        );
        let combined = combine_patches(d).unwrap();
        let keys: Vec<_> = combined
            .iter()
            .map(|e| (e.key.clone(), e.op.name()))
            .collect();
        assert_eq!(
            keys,
            [
                (Key::Index(0), "addrange"),
                (Key::Index(1), "patch"),
                (Key::Index(3), "removerange")
            ]
        );
        let Op::Patch { diff: inner } = &combined[1].op else {
            panic!()
        };
        assert_eq!(inner.len(), 2);
        let mixed = vec![DiffEntry::remove("a"), DiffEntry::removerange(0, 1)];
        assert!(combine_patches(mixed).is_err());
    }

    #[test]
    fn generic_resolution_skips_conflicts_that_have_a_strategy() {
        let l = diff(r#"[{"op":"replace","key":"a","value":1}]"#);
        let r = diff(r#"[{"op":"replace","key":"a","value":2}]"#);
        let mut b = MergeDecisionBuilder::new();
        b.conflict(vec![], Some(l.clone()), Some(r.clone()), None)
            .unwrap();
        b.add_decision(
            vec![],
            Action::Custom,
            Some(l),
            Some(r),
            super::super::decisions::Extras {
                conflict: true,
                strategy: Some("inline-source".into()),
                custom_diff: Some(vec![]),
                ..Default::default()
            },
        );
        resolve_strategy_generic(&[], &mut b, Some("use-remote"));
        assert_eq!(
            (b.decisions[0].conflict, &b.decisions[0].action),
            (false, &Action::Remote)
        );
        assert!(b.decisions[1].conflict);
        // `mergetool` and unknown strategies leave everything alone.
        let mut c = MergeDecisionBuilder::new();
        let l = diff(r#"[{"op":"replace","key":"a","value":1}]"#);
        let r = diff(r#"[{"op":"replace","key":"a","value":2}]"#);
        c.conflict(vec![], Some(l), Some(r), None).unwrap();
        resolve_strategy_generic(&[], &mut c, Some("mergetool"));
        resolve_strategy_generic(&[], &mut c, Some("bogus"));
        assert!(c.decisions[0].conflict);
    }

    #[test]
    fn remove_outputs_replaces_conflicts_by_removal() {
        // Both sides patch output 1 differently, nothing else happens.
        let l = diff(r#"[{"op":"patch","key":1,"diff":[{"op":"replace","key":"x","value":1}]}]"#);
        let r = diff(r#"[{"op":"patch","key":1,"diff":[{"op":"replace","key":"x","value":2}]}]"#);
        let mut b = MergeDecisionBuilder::new();
        b.conflict(vec![key("outputs")], Some(l), Some(r), None)
            .unwrap();
        assert_eq!(
            b.decisions[0].common_path,
            [key("outputs"), Key::Index(1), key("x")][..2]
        );
        resolve_strategy_remove_outputs(&[key("outputs")], &mut b).unwrap();
        assert_eq!(b.decisions.len(), 1);
        let d = &b.decisions[0];
        assert_eq!((d.conflict, &d.action), (false, &Action::Custom));
        assert_eq!(
            d.custom_diff,
            Some(diff(r#"[{"op":"removerange","key":1,"length":1}]"#))
        );
    }

    #[test]
    fn inline_outputs_wrap_both_versions_in_markers() {
        let base = v(r#"[{"output_type":"stream","name":"stdout","text":"a"}]"#);
        let l = diff(
            r#"[{"op":"patch","key":0,"diff":[{"op":"patch","key":"text","diff":[{"op":"addrange","key":0,"valuelist":["L"]}]}]}]"#,
        );
        let r = diff(r#"[{"op":"removerange","key":0,"length":1}]"#);
        let mut b = MergeDecisionBuilder::new();
        b.conflict(vec![key("outputs")], Some(l), Some(r), None)
            .unwrap();
        resolve_strategy_inline_outputs(&[key("outputs")], base.as_list().unwrap(), &mut b)
            .unwrap();
        let d = &b.decisions[0];
        assert!(d.conflict);
        let custom = d.custom_diff.as_ref().unwrap();
        assert_eq!(custom.len(), 2);
        let Op::AddRange {
            valuelist: Value::List(outputs),
        } = &custom[0].op
        else {
            panic!()
        };
        let texts: Vec<_> = outputs
            .iter()
            .map(|o| o.get("text").unwrap().as_str().unwrap().to_owned())
            .collect();
        assert_eq!(texts[0], "<<<<<<< local <modified: >\n");
        assert_eq!(texts[2], "=======\n");
        assert_eq!(texts[3], ">>>>>>> remote <removed>\n");
        assert_eq!(outputs.len(), 4);
    }

    #[test]
    fn inline_outputs_conflict_appended_after_the_last_output_has_no_base() {
        let base = v(r#"[{"output_type":"stream","name":"stdout","text":"a"}]"#);
        let append = |text: &str| {
            diff(&format!(
                r#"[{{"op":"addrange","key":1,"valuelist":[{{"output_type":"stream","name":"stdout","text":"{text}"}}]}}]"#
            ))
        };
        let mut b = MergeDecisionBuilder::new();
        b.conflict(
            vec![key("outputs")],
            Some(append("L")),
            Some(append("R")),
            None,
        )
        .unwrap();
        resolve_strategy_inline_outputs(&[key("outputs")], base.as_list().unwrap(), &mut b)
            .unwrap();
        let d = &b.decisions[0];
        assert!(d.conflict);
        // Only inserts, so the base stays and nothing is removed.
        assert_eq!(d.custom_diff.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn marker_cells_have_distinct_deterministic_ids() {
        let mut ids = MarkerIds::default();
        ids.avoid(&v(r#"{"id":"abcd1234"}"#));
        let a = ids.cell_marker("<<<<<<< local", (3, 0));
        let b = ids.cell_marker("<<<<<<< local", (3, 1));
        assert_ne!(a.get("id"), b.get("id"));
        let again = MarkerIds::default().cell_marker("<<<<<<< local", (3, 0));
        assert_eq!(a.get("id"), again.get("id"));
        let id = a.get("id").unwrap().as_str().unwrap();
        assert!(
            id.len() == 8
                && id
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        let keys: Vec<_> = a.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["id", "cell_type", "source", "metadata"]);
        assert_eq!(
            a.get("source").unwrap().as_str().unwrap(),
            "<span style=\"color:red\"><b><<<<<<< local</b></span>"
        );
    }

    #[test]
    fn marker_ids_avoid_every_id_the_merge_can_bring_in() {
        let id_of = |ids: &mut MarkerIds| {
            let cell = ids.cell_marker("<<<<<<< local", (0, 0));
            cell.get("id").unwrap().as_str().unwrap().to_owned()
        };
        let first = id_of(&mut MarkerIds::default());
        let cell = |id: &str| format!(r#"{{"id":"{id}","cell_type":"code","source":""}}"#);

        let mut local_only = MarkerIds::default();
        local_only.avoid_notebook(&v(&format!(r#"{{"cells":[{}]}}"#, cell(&first))));
        assert_ne!(id_of(&mut local_only), first);

        let diffs = [
            format!(
                r#"[{{"op":"addrange","key":0,"valuelist":[{}]}}]"#,
                cell(&first)
            ),
            format!(
                r#"[{{"op":"patch","key":0,"diff":[{{"op":"replace","key":"id","value":"{first}"}}]}}]"#
            ),
            format!(r#"[{{"op":"replace","key":0,"value":{}}}]"#, cell(&first)),
            format!(
                r#"[{{"op":"patch","key":"cells","diff":[{{"op":"add","key":"id","value":"{first}"}}]}}]"#
            ),
        ];
        for text in diffs {
            let mut ids = MarkerIds::default();
            ids.avoid_all(&[], &[decision_with_diffs(diff(&text))]);
            assert_ne!(id_of(&mut ids), first, "{text}");
        }
    }

    fn decision_with_diffs(local: Diff) -> MergeDecision {
        let mut b = MergeDecisionBuilder::new();
        b.add_decision(
            vec![],
            Action::Local,
            Some(local),
            Some(vec![]),
            crate::merging::decisions::Extras::default(),
        );
        b.decisions.remove(0)
    }

    #[test]
    fn record_conflict_adds_nbdime_conflicts() {
        let l = diff(r#"[{"op":"replace","key":"a","value":1}]"#);
        let r = diff(r#"[{"op":"replace","key":"a","value":2}]"#);
        let mut b = MergeDecisionBuilder::new();
        b.conflict(vec![key("metadata")], Some(l), Some(r), None)
            .unwrap();
        let base = v(r#"{"a":0}"#);
        resolve_strategy_record_conflicts(&[key("metadata")], base.as_object().unwrap(), &mut b)
            .unwrap();
        assert_eq!(b.decisions.len(), 1);
        let d = &b.decisions[0];
        assert!(d.conflict);
        let Op::Add { value } = &d.custom_diff.as_ref().unwrap()[0].op else {
            panic!()
        };
        assert_eq!(value.get("local_diff").unwrap().as_list().unwrap().len(), 1);
    }
}
