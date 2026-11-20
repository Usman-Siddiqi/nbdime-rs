//! Helpers for resolving conflicts after the decisions are made: bundling the decisions below a
//! path, and building the diffs a callback needs to resolve them.

use std::collections::HashMap;

use crate::diff_format::{Diff, DiffEntry, Key, Op};
use crate::patch::patch;
use crate::path::split_path;
use crate::value::Value;

use super::decisions::{build_diffs, filter_decisions, resolve_path, MergeDecision, Which};
use super::{invalid, MergeError};

/// `patch_item` for a single entry: the value after replacing it or patching it, `None` when
/// it is removed (upstream's `Deleted`). No entry leaves the value alone.
pub fn patch_item(value: &Value, entry: Option<&DiffEntry>) -> Result<Option<Value>, MergeError> {
    let Some(entry) = entry else {
        return Ok(Some(value.clone()));
    };
    match &entry.op {
        Op::Replace { value } => Ok(Some(value.clone())),
        Op::Patch { diff } => Ok(Some(patch(value, diff)?)),
        Op::Remove => Ok(None),
        op => invalid(format!("Invalid item patch op {}", op.name())),
    }
}

/// The value with both sides' item changes appended to it, as when joining outputs.
pub fn make_join_value(
    value: &[Value],
    local: Option<&DiffEntry>,
    remote: Option<&DiffEntry>,
) -> Result<Vec<Value>, MergeError> {
    let whole = Value::List(value.to_vec());
    let mut joined = value.to_vec();
    for entry in [local, remote] {
        match patch_item(&whole, entry)? {
            // A deleted side contributes nothing.
            None => {}
            Some(Value::List(items)) => joined.extend(items),
            Some(_) => return invalid("can only concatenate list"),
        }
    }
    Ok(joined)
}

/// Splits decisions into those outside the cells and those on them.
pub fn split_decisions_by_cell(
    decisions: Vec<MergeDecision>,
) -> (Vec<MergeDecision>, Vec<MergeDecision>) {
    decisions
        .into_iter()
        .partition(|d| d.common_path.first() != Some(&Key::from("cells")))
}

/// Bundles the decisions that share a prefix. Without conflicts the decisions pass through
/// (their bundling level cleared); with, `callback` resolves them from the local and remote
/// diffs the decisions amount to on the base at the prefix.
fn make_bundled_decisions<F>(
    base: &Value,
    prefix: &[Key],
    mut decisions: Vec<MergeDecision>,
    callback: &mut F,
) -> Result<Vec<MergeDecision>, MergeError>
where
    F: FnMut(&Value, &[Key], Option<Diff>, Option<Diff>) -> Result<Vec<MergeDecision>, MergeError>,
{
    if !decisions.iter().any(|d| d.conflict) {
        for d in &mut decisions {
            if d.level.take().is_none() {
                return invalid("KeyError: '_level'");
            }
        }
        return Ok(decisions);
    }
    let resolved_base = resolve_path(base, prefix)?;
    let local_diff = build_diffs(resolved_base, &decisions, Which::Local)?;
    let remote_diff = build_diffs(resolved_base, &decisions, Which::Remote)?;
    callback(resolved_base, prefix, local_diff, remote_diff)
}

/// Bundles the decisions whose path matches `pattern` by their path prefix of the pattern's
/// length, and lets `callback` resolve the bundles that have conflicts.
///
/// Returns the other decisions first, then the bundled ones.
pub fn bundle_decisions<F>(
    base: &Value,
    decisions: &[MergeDecision],
    pattern: &str,
    mut callback: F,
) -> Result<Vec<MergeDecision>, MergeError>
where
    F: FnMut(&Value, &[Key], Option<Diff>, Option<Diff>) -> Result<Vec<MergeDecision>, MergeError>,
{
    let indices = filter_decisions(pattern, decisions, false);
    let mut bundled = vec![false; decisions.len()];
    indices.iter().for_each(|&i| bundled[i] = true);
    let mut out: Vec<MergeDecision> = decisions
        .iter()
        .zip(&bundled)
        .filter(|(_, &b)| !b)
        .map(|(d, _)| d.clone())
        .collect();

    // Group the decisions of each prefix, in order of first appearance.
    let level = split_path(pattern).len();
    let mut positions: HashMap<Vec<Key>, usize> = HashMap::new();
    let mut groups: Vec<(Vec<Key>, Vec<MergeDecision>)> = Vec::new();
    for &i in &indices {
        let mut dec = decisions[i].clone();
        let prefix: Vec<Key> = dec.common_path.iter().take(level).cloned().collect();
        dec.level = Some(level);
        let slot = *positions.entry(prefix.clone()).or_insert_with(|| {
            groups.push((prefix, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(dec);
    }

    for (prefix, group) in groups {
        out.extend(make_bundled_decisions(base, &prefix, group, &mut callback)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::diff_from_value;
    use crate::json::parse;
    use crate::merging::decisions::Action;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    fn decision(path: &[Key], conflict: bool, local: &str) -> MergeDecision {
        MergeDecision {
            common_path: path.to_vec(),
            conflict,
            action: Action::Local,
            local_diff: Some(diff_from_value(&v(local)).unwrap()),
            remote_diff: None,
            similar_insert: None,
            custom_diff: None,
            strategy: None,
            level: None,
        }
    }

    #[test]
    fn patch_item_replaces_patches_and_deletes() {
        let value = v(r#"{"a":1}"#);
        let entry = |text: &str| diff_from_value(&v(text)).unwrap().remove(0);
        assert_eq!(patch_item(&value, None).unwrap(), Some(value.clone()));
        assert_eq!(
            patch_item(
                &value,
                Some(&entry(r#"[{"op":"replace","key":"k","value":5}]"#))
            )
            .unwrap(),
            Some(Value::from(5))
        );
        assert_eq!(
            patch_item(&value, Some(&entry(r#"[{"op":"remove","key":"k"}]"#))).unwrap(),
            None
        );
        assert!(patch_item(
            &value,
            Some(&entry(r#"[{"op":"add","key":"k","value":1}]"#))
        )
        .is_err());
    }

    #[test]
    fn join_value_appends_both_sides() {
        let outputs = [Value::from(1)];
        let entry = |text: &str| diff_from_value(&v(text)).unwrap().remove(0);
        let l = entry(r#"[{"op":"replace","key":"k","value":[2]}]"#);
        let r = entry(r#"[{"op":"remove","key":"k"}]"#);
        assert_eq!(
            make_join_value(&outputs, Some(&l), Some(&r)).unwrap(),
            [Value::from(1), Value::from(2)]
        );
    }

    #[test]
    fn splits_cell_decisions() {
        let (generic, cells) = split_decisions_by_cell(vec![
            decision(&[Key::from("metadata")], false, "[]"),
            decision(&[Key::from("cells"), Key::Index(0)], false, "[]"),
        ]);
        assert_eq!((generic.len(), cells.len()), (1, 1));
    }

    #[test]
    fn unconflicted_bundles_pass_through() {
        let base = v(r#"{"cells":[{"source":"a"}]}"#);
        let decisions = [
            decision(&[Key::from("metadata")], false, "[]"),
            decision(
                &[Key::from("cells"), Key::Index(0)],
                false,
                r#"[{"op":"patch","key":"source","diff":[]}]"#,
            ),
        ];
        let out = bundle_decisions(&base, &decisions, "/cells/*/source", |_, _, _, _| {
            panic!("no conflicts to resolve")
        })
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].common_path, [Key::from("metadata")]);
        assert_eq!(out[1].level, None);
    }

    #[test]
    fn conflicted_bundles_go_to_the_callback_with_their_diffs() {
        let base = v(r#"{"cells":[{"source":"a"}]}"#);
        let decisions = [decision(
            &[Key::from("cells"), Key::Index(0)],
            true,
            r#"[{"op":"patch","key":"source","diff":[]}]"#,
        )];
        let mut seen = Vec::new();
        let out = bundle_decisions(&base, &decisions, "/cells/*/source", |b, prefix, l, r| {
            seen.push((b.clone(), prefix.to_vec(), l.is_some(), r.is_some()));
            Ok(Vec::new())
        })
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1, [Key::from("cells"), Key::Index(0)]);
        assert!(seen[0].2 && !seen[0].3);
    }
}
