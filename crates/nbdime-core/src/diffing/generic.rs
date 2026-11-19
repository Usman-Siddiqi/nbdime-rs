//! The type-directed recursive diff of JSON values.

use crate::diff_format::{validate_diff, Diff, MappingDiffBuilder, SequenceDiffBuilder};
use crate::diff_utils::count_consumed_symbols;
use crate::difflib::{self, SequenceMatcher};
use crate::py::py_len;
use crate::value::{Map, Value};

use super::config::{join_path, DiffConfig, Differ, Predicate};
use super::sequence::diff_sequence_bruteforce;
use super::snakes::{compute_diff_from_snakes, compute_snakes_multilevel, Compare};
use super::strings::{diff_strings_by_char, diff_strings_linewise};
use super::DiffError;

impl Predicate {
    pub fn apply(self, a: &Value, b: &Value) -> bool {
        match self {
            Predicate::Eq => a.py_eq(b),
            Predicate::StringsApproximate => match (a, b) {
                (Value::Str(x), Value::Str(y)) => compare_strings_approximate(x, y, 0.7, None),
                _ => false,
            },
        }
    }
}

impl Differ {
    pub fn apply(
        self,
        a: &Value,
        b: &Value,
        path: &str,
        config: &DiffConfig,
    ) -> Result<Diff, DiffError> {
        match self {
            Differ::Generic => diff_with_config(a, b, path, config),
            Differ::StringsByChar => match (a, b) {
                (Value::Str(a), Value::Str(b)) => Ok(diff_strings_by_char(a, b)),
                _ => Err(DiffError::NotStrings),
            },
        }
    }
}

/// Whether two strings are similar enough to be considered the same item.
///
/// Empty versus non-empty never matches and equal strings always do. Otherwise the cheap upper
/// bounds on the difflib ratio are checked against `threshold` before the full ratio, which must
/// then be strictly greater. With `maxlen`, strings that are both longer are only accepted when
/// equal.
pub fn compare_strings_approximate(
    x: &str,
    y: &str,
    threshold: f64,
    maxlen: Option<usize>,
) -> bool {
    if x.is_empty() != y.is_empty() {
        return false;
    }
    if x == y {
        return true;
    }
    let (len_x, len_y) = (py_len(x), py_len(y));
    if difflib::real_quick_ratio(len_x, len_y) < threshold {
        return false;
    }
    if maxlen.is_some_and(|maxlen| len_x > maxlen && len_y > maxlen) {
        return false;
    }
    let (x, y): (Vec<char>, Vec<char>) = (x.chars().collect(), y.chars().collect());
    if difflib::quick_ratio(&x, &y) < threshold {
        return false;
    }
    SequenceMatcher::new(&x, &y).ratio() > threshold
}

/// `nbdime.diff(a, b)` with the default configuration.
pub fn diff(a: &Value, b: &Value) -> Result<Diff, DiffError> {
    diff_with_config(a, b, "", &DiffConfig::default())
}

/// Diff of two lists, dicts or strings of the same kind found at `path`.
pub fn diff_with_config(
    a: &Value,
    b: &Value,
    path: &str,
    config: &DiffConfig,
) -> Result<Diff, DiffError> {
    let d = match (a, b) {
        (Value::List(a), Value::List(b)) => diff_lists(a, b, path, config)?,
        (Value::Object(a), Value::Object(b)) => diff_dicts(a, b, path, config)?,
        // Line diffs use their own configuration: only the character differ and the line
        // predicates make sense inside a string.
        (Value::Str(a), Value::Str(b)) => diff_strings_linewise(a, b),
        _ => return Err(DiffError::UnsupportedTypes),
    };
    validate_diff(&d, false)?;
    Ok(d)
}

/// Diff of two lists: aligned items are recursed into, the rest become range operations.
pub fn diff_lists(
    a: &[Value],
    b: &[Value],
    path: &str,
    config: &DiffConfig,
) -> Result<Diff, DiffError> {
    let predicates = config.predicates(root_if_empty(path));
    if predicates.len() > 1 {
        return diff_sequence_multilevel(a, b, path, config);
    }

    let predicate = predicates[0];
    let shallow = diff_sequence_bruteforce(a, b, |x, y| predicate.apply(x, y));

    let subpath = join_path(path, "*");
    let differ = config.differ(&subpath);

    // Items consumed from `a` and `b` so far.
    let (mut i, mut j) = (0, 0);
    let mut di = SequenceDiffBuilder::new();
    for entry in shallow.iter().map(Some).chain([None]) {
        let (n, askip, bskip) = match entry {
            Some(e) => {
                let index = e.key.as_index().expect("sequence diff has index keys");
                let (askip, bskip) = count_consumed_symbols(e)?;
                (index.saturating_sub(i), askip, bskip)
            }
            None => (a.len() - i, 0, 0),
        };

        // The n items before this entry are aligned; diff them pairwise.
        for k in 0..n {
            let (aval, bval) = (&a[i + k], &b[j + k]);
            if !config.is_atomic(aval, &subpath) {
                let cd = differ.apply(aval, bval, &subpath, config)?;
                di.patch(i + k, cd);
            }
        }
        i += n + askip;
        j += n + bskip;
        if let Some(e) = entry {
            di.append(e.clone());
        }
    }
    debug_assert_eq!((i, j), (a.len(), b.len()));
    Ok(di.validated())
}

/// Diff of two lists aligned with several predicates, loosest first.
pub fn diff_sequence_multilevel(
    a: &[Value],
    b: &[Value],
    path: &str,
    config: &DiffConfig,
) -> Result<Diff, DiffError> {
    let predicates = config.predicates(root_if_empty(path));
    let closures: Vec<_> = predicates
        .iter()
        .map(|&p| move |x: &Value, y: &Value| p.apply(x, y))
        .collect();
    let compares: Vec<Compare<Value>> = closures.iter().map(|c| c as Compare<Value>).collect();
    let snakes = compute_snakes_multilevel(a, b, &compares);
    compute_diff_from_snakes(a, b, &snakes, path, config)
}

/// Diff of two dicts: removed keys, changed values and added keys, sorted by key.
pub fn diff_dicts(a: &Map, b: &Map, path: &str, config: &DiffConfig) -> Result<Diff, DiffError> {
    let mut di = MappingDiffBuilder::new();

    // Keys are processed in sorted order for a deterministic diff (byte order of UTF-8 equals
    // Python's code point order).
    let sorted_keys = |m: &Map, other: &Map, in_other: bool| -> Vec<String> {
        let mut keys: Vec<String> = m
            .keys()
            .filter(|k| other.contains_key(*k) == in_other)
            .cloned()
            .collect();
        keys.sort_unstable();
        keys
    };

    for key in sorted_keys(a, b, false) {
        di.remove(&key);
    }
    for key in sorted_keys(a, b, true) {
        let (avalue, bvalue) = (&a[&key], &b[&key]);
        let subpath = join_path(path, &key);
        if avalue.same_type(bvalue) && !config.is_atomic(avalue, &subpath) {
            let dd = config
                .differ(&subpath)
                .apply(avalue, bvalue, &subpath, config)?;
            di.patch(&key, dd);
        } else if !avalue.py_eq(bvalue) {
            di.replace(&key, bvalue.clone());
        }
    }
    for key in sorted_keys(b, a, false) {
        di.add(&key, b[&key].clone());
    }
    Ok(di.validated())
}

/// Predicates are looked up under `/` for the root path.
fn root_if_empty(path: &str) -> &str {
    if path.is_empty() {
        "/"
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::diff_to_value;
    use crate::json::{dumps, parse, DumpOptions};

    fn diff_json(a: &str, b: &str) -> String {
        let d = diff(&parse(a).unwrap(), &parse(b).unwrap()).unwrap();
        dumps(&diff_to_value(&d), &DumpOptions::compact())
    }

    #[test]
    fn approximate_comparison_rules() {
        assert!(!compare_strings_approximate("", "x", 0.7, None));
        assert!(compare_strings_approximate("", "", 0.7, None));
        assert!(compare_strings_approximate("abc", "abc", 1.0, Some(0)));
        // ratio("ab", "ac") is exactly 0.5: passes the bounds, fails the strict final test.
        assert!(!compare_strings_approximate("ab", "ac", 0.5, None));
        assert!(compare_strings_approximate("ab", "ac", 0.4, None));
        // maxlen rejects long non-identical strings even when similar.
        assert!(compare_strings_approximate(
            "abcdefgh", "abcdefgx", 0.7, None
        ));
        assert!(!compare_strings_approximate(
            "abcdefgh",
            "abcdefgx",
            0.7,
            Some(5)
        ));
        assert!(compare_strings_approximate(
            "abcdefgh",
            "abcdefgx",
            0.7,
            Some(8)
        ));
    }

    #[test]
    fn dict_diff_orders_keys_and_uses_python_equality() {
        assert_eq!(
            diff_json(r#"{"b":1,"a":true,"d":1}"#, r#"{"b":1.0,"a":2,"c":0}"#),
            r#"[{"op":"replace","key":"a","value":2},{"op":"add","key":"c","value":0},{"op":"remove","key":"d"}]"#
        );
    }

    #[test]
    fn type_change_under_a_path_that_also_holds_lists_is_not_an_error() {
        // Upstream raises here because the list at "/*" registered the path (findings F-001).
        assert_eq!(diff_json(r#"[[1],{"k":1}]"#, r#"[[1],{"k":true}]"#), "[]");
    }

    #[test]
    fn list_diff_aligns_with_python_equality() {
        assert_eq!(
            diff_json(r#"[1,{"a":2},"x"]"#, r#"[true,{"a":2.0},"y"]"#),
            r#"[{"op":"addrange","key":2,"valuelist":["y"]},{"op":"removerange","key":2,"length":1}]"#
        );
    }

    #[test]
    fn string_diff_is_linewise() {
        assert_eq!(
            diff_json(r#""a\nb""#, r#""a\nc""#),
            r#"[{"op":"addrange","key":1,"valuelist":["c"]},{"op":"removerange","key":1,"length":1}]"#
        );
        assert_eq!(
            diff_json(r#""abc\n""#, r#""abd\n""#),
            r#"[{"op":"patch","key":0,"diff":[{"op":"addrange","key":2,"valuelist":"d"},{"op":"removerange","key":2,"length":1}]}]"#
        );
    }

    #[test]
    fn mismatched_or_scalar_inputs_are_errors() {
        let (list, dict, int) = (
            parse("[]").unwrap(),
            parse("{}").unwrap(),
            parse("1").unwrap(),
        );
        assert_eq!(diff(&list, &dict), Err(DiffError::UnsupportedTypes));
        assert_eq!(diff(&int, &int), Err(DiffError::UnsupportedTypes));
    }

    #[test]
    fn nested_scalars_never_recurse() {
        assert_eq!(
            diff_json(r#"[1,2]"#, r#"[1,3]"#),
            r#"[{"op":"addrange","key":1,"valuelist":[3]},{"op":"removerange","key":1,"length":1}]"#
        );
    }
}
