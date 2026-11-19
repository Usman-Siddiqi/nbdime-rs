//! Helpers shared by diff consumers.

use crate::diff_format::{DiffEntry, DiffFormatError, Op};
use crate::value::Value;

/// How many items a single sequence diff entry consumes from the base and the remote sequence.
///
/// For character diffs `valuelist` is a string and its length is counted in code points.
pub fn count_consumed_symbols(e: &DiffEntry) -> Result<(usize, usize), DiffFormatError> {
    match &e.op {
        Op::AddRange { valuelist } => Ok((
            0,
            match valuelist {
                Value::List(items) => items.len(),
                Value::Str(s) => s.chars().count(),
                _ => {
                    return Err(DiffFormatError(
                        "addrange valuelist is not a sequence".into(),
                    ))
                }
            },
        )),
        Op::RemoveRange { length } => Ok((*length, 0)),
        Op::Patch { .. } => Ok((1, 1)),
        op => Err(DiffFormatError(format!("Invalid op '{}'", op.name()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumed_symbols() {
        let add = DiffEntry::addrange(0, Value::from("a\u{1F600}b"));
        assert_eq!(count_consumed_symbols(&add).unwrap(), (0, 3));
        let add = DiffEntry::addrange(0, Value::List(vec![Value::Null; 2]));
        assert_eq!(count_consumed_symbols(&add).unwrap(), (0, 2));
        assert_eq!(
            count_consumed_symbols(&DiffEntry::removerange(4, 7)).unwrap(),
            (7, 0)
        );
        assert_eq!(
            count_consumed_symbols(&DiffEntry::patch(1, vec![])).unwrap(),
            (1, 1)
        );
        assert!(count_consumed_symbols(&DiffEntry::remove("k")).is_err());
    }
}
