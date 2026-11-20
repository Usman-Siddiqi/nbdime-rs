//! Splitting the diffs of two sides into chunks of the base that can be merged independently.

use std::collections::BTreeSet;

use crate::diff_format::{Diff, DiffEntry, Op, SequenceDiffBuilder};

use super::{assertion, invalid, MergeError};

/// The part `base[start..end]` of a sequence together with the entries of both diffs that begin
/// at `start`. Each side has at most an `addrange` at `start` followed by one `removerange` or
/// `patch` covering the whole range.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub start: usize,
    pub end: usize,
    pub local: Diff,
    pub remote: Diff,
}

fn index(e: &DiffEntry) -> Result<usize, MergeError> {
    match e.key.as_index() {
        Some(i) => Ok(i),
        None => invalid(format!(
            "sequence diff key must be an integer, not {}",
            e.key
        )),
    }
}

/// The keys at which `diffs` start or stop affecting the sequence.
pub fn get_section_boundaries(diffs: &[DiffEntry]) -> Result<BTreeSet<usize>, MergeError> {
    let mut boundaries = BTreeSet::new();
    for e in diffs {
        let j = index(e)?;
        boundaries.insert(j);
        match &e.op {
            Op::RemoveRange { length } => {
                boundaries.insert(j + length);
            }
            Op::Patch { .. } => {
                boundaries.insert(j + 1);
            }
            _ => {}
        }
    }
    Ok(boundaries)
}

/// Splits every `removerange` at the boundaries inside it.
pub fn split_diffs_on_boundaries(
    diffs: &[DiffEntry],
    boundaries: &[usize],
) -> Result<Diff, MergeError> {
    let mut newdiffs = SequenceDiffBuilder::new();
    // Next relevant boundary index.
    let mut b = 0;
    for e in diffs {
        match &e.op {
            Op::AddRange { .. } | Op::Patch { .. } => newdiffs.append(e.clone()),
            Op::RemoveRange { length } => {
                let key = index(e)?;
                while boundaries.get(b).is_some_and(|&x| x < key) {
                    b += 1;
                }
                if boundaries.get(b) != Some(&key) {
                    return assertion("key not found in boundaries");
                }
                while b + 1 < boundaries.len() && boundaries[b + 1] <= key + length {
                    newdiffs.removerange(boundaries[b], boundaries[b + 1] - boundaries[b]);
                    b += 1;
                }
            }
            op => return invalid(format!("Unhandled diff entry op {}.", op.name())),
        }
    }
    Ok(newdiffs.validated())
}

/// The chunks `(j, k, local, remote)` over `boundaries`, without the empty ones.
fn make_chunks(boundaries: &[usize], local: &[DiffEntry], remote: &[DiffEntry]) -> Vec<Chunk> {
    let mut at = [0usize; 2];
    let diffs = [local, remote];
    let mut chunks = Vec::new();
    for (i, &j) in boundaries.iter().enumerate() {
        let k = boundaries.get(i + 1).copied().unwrap_or(j);
        let mut sub: [Diff; 2] = [Vec::new(), Vec::new()];
        for (m, d) in diffs.iter().enumerate() {
            while at[m] < d.len() && d[at[m]].key.as_index() == Some(j) {
                sub[m].push(d[at[m]].clone());
                at[m] += 1;
            }
        }
        if j < k || sub.iter().any(|s| !s.is_empty()) {
            let [local, remote] = sub;
            chunks.push(Chunk {
                start: j,
                end: k,
                local,
                remote,
            });
        }
    }
    chunks
}

/// The chunks of a base of `base_len` items that the two diffs partition into.
///
/// The whole range `0..base_len` is covered. A chunk with empty diffs on both sides is
/// unmodified. With `single_item`, no chunk spans more than one base item.
///
/// Like upstream, this fails for an empty base and no entries at all: no chunk is produced.
pub fn make_merge_chunks(
    base_len: usize,
    local: &[DiffEntry],
    remote: &[DiffEntry],
    single_item: bool,
) -> Result<Vec<Chunk>, MergeError> {
    let boundaries: Vec<usize> = if single_item {
        (0..=base_len).collect()
    } else {
        // Including 0 and the length makes the loop over chunks cleaner.
        let mut set = BTreeSet::from([0, base_len]);
        set.extend(get_section_boundaries(local)?);
        set.extend(get_section_boundaries(remote)?);
        set.into_iter().collect()
    };
    let local = split_diffs_on_boundaries(local, &boundaries)?;
    let remote = split_diffs_on_boundaries(remote, &boundaries)?;
    let chunks = make_chunks(&boundaries, &local, &remote);
    let (Some(first), Some(last)) = (chunks.first(), chunks.last()) else {
        return assertion("no merge chunks produced");
    };
    if first.start != 0 {
        return assertion("invalid range start of first merge chunk");
    }
    if last.end != base_len {
        return assertion("invalid range end of final merge chunk");
    }
    Ok(chunks)
}

/// The kinds of entries in a chunk's diff as two strings: `A` for each `addrange` and `a` for
/// each `add`, then `P` for each patch, `R` for a `removerange`, `r` for a `remove` and `c` for
/// a `replace`.
pub fn chunk_typename(diffs: &[DiffEntry]) -> (String, String) {
    let mut aname = String::new();
    let mut pname = String::new();
    for e in diffs {
        match &e.op {
            Op::AddRange { .. } => aname.push('A'),
            Op::Add { .. } => aname.push('a'),
            Op::Patch { .. } => pname.push('P'),
            Op::RemoveRange { .. } => pname.push('R'),
            Op::Remove => pname.push('r'),
            Op::Replace { .. } => pname.push('c'),
            Op::ParentDeleted => {}
        }
    }
    (aname, pname)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::diff_from_value;
    use crate::json::parse;

    fn diff(text: &str) -> Diff {
        diff_from_value(&parse(text).unwrap()).unwrap()
    }

    fn shape(chunks: &[Chunk]) -> Vec<(usize, usize, String, String)> {
        chunks
            .iter()
            .map(|c| {
                let (la, lp) = chunk_typename(&c.local);
                let (ra, rp) = chunk_typename(&c.remote);
                (c.start, c.end, la + &lp, ra + &rp)
            })
            .collect()
    }

    #[test]
    fn removeranges_are_split_at_the_other_sides_boundaries() {
        let local = diff(r#"[{"op":"removerange","key":1,"length":3}]"#);
        let remote = diff(
            r#"[{"op":"addrange","key":2,"valuelist":["x"]},{"op":"patch","key":2,"diff":[]}]"#,
        );
        let chunks = make_merge_chunks(5, &local, &remote, false).unwrap();
        assert_eq!(
            shape(&chunks),
            [
                (0, 1, String::new(), String::new()),
                (1, 2, "R".into(), String::new()),
                (2, 3, "R".into(), "AP".into()),
                (3, 4, "R".into(), String::new()),
                (4, 5, String::new(), String::new()),
            ]
        );
    }

    #[test]
    fn inserts_at_the_end_form_an_empty_final_chunk() {
        let local = diff(r#"[{"op":"addrange","key":2,"valuelist":["x"]}]"#);
        let chunks = make_merge_chunks(2, &local, &[], false).unwrap();
        assert_eq!(
            shape(&chunks),
            [
                (0, 2, String::new(), String::new()),
                (2, 2, "A".into(), String::new())
            ]
        );
    }

    #[test]
    fn single_item_chunks_cover_each_base_item() {
        let local = diff(r#"[{"op":"removerange","key":0,"length":2}]"#);
        let chunks = make_merge_chunks(2, &local, &[], true).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(shape(&chunks)[1], (1, 2, "R".into(), String::new()));
    }

    #[test]
    fn empty_base_without_changes_is_an_assertion_failure() {
        assert!(matches!(
            make_merge_chunks(0, &[], &[], false),
            Err(MergeError::Assertion(_))
        ));
        let past_end = diff(r#"[{"op":"addrange","key":5,"valuelist":["x"]}]"#);
        assert!(matches!(
            make_merge_chunks(2, &past_end, &[], false),
            Err(MergeError::Assertion(_))
        ));
    }

    #[test]
    fn other_ops_and_mapping_keys_are_errors() {
        let replace = diff(r#"[{"op":"replace","key":"a","value":1}]"#);
        assert!(make_merge_chunks(2, &replace, &[], false).is_err());
        let add = vec![DiffEntry::add(0usize, crate::Value::Null)];
        assert!(make_merge_chunks(2, &add, &[], false).is_err());
    }
}
