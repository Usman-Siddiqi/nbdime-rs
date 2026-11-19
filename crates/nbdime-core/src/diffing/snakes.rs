//! Snakes: maximal runs of consecutive aligned items, computed under several predicates of
//! increasing strictness.

use std::convert::Infallible;

use crate::diff_format::{Diff, SequenceDiffBuilder};
use crate::value::Value;

use super::sequence::{never_fails, try_lcs_indices};

/// `n` consecutive items starting at `a[i]` and `b[j]` that are aligned with each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snake {
    pub i: usize,
    pub j: usize,
    pub n: usize,
}

/// An infallible compare predicate on sequence items.
pub type Compare<'a, T> = &'a dyn Fn(&T, &T) -> bool;

/// `a[i0..i1]` x `b[j0..j1]`.
#[derive(Clone, Copy)]
struct Rect {
    i0: usize,
    j0: usize,
    i1: usize,
    j1: usize,
}

/// Snakes of a brute-force LCS of the items inside `rect`, in absolute coordinates.
///
/// Upstream extends the last snake when the new pair equals its *start* rather than its end, which
/// only ever happens for a pair at (0, 0) of the rectangle. Every other pair is a snake of its
/// own, and this is reproduced because the snake list is observable.
fn compute_snakes<T, E>(
    a: &[T],
    b: &[T],
    compare: impl Fn(&T, &T) -> Result<bool, E>,
    rect: Rect,
) -> Result<Vec<Snake>, E> {
    let pairs = try_lcs_indices(&a[rect.i0..rect.i1], &b[rect.j0..rect.j1], compare)?;
    let mut snakes = vec![Snake { i: 0, j: 0, n: 0 }];
    for (i, j) in pairs {
        match snakes.last_mut() {
            Some(last) if last.i == i && last.j == j => last.n += 1,
            _ => snakes.push(Snake { i, j, n: 1 }),
        }
    }
    if snakes[0].n == 0 {
        snakes.remove(0);
    }
    for snake in &mut snakes {
        snake.i += rect.i0;
        snake.j += rect.j0;
    }
    Ok(snakes)
}

/// Snakes under several predicates, ordered from the loosest to the strictest.
///
/// The strictest predicate aligns first; the gaps between its snakes are then filled in with
/// the next looser predicate, and so on down to the loosest.
///
/// # Panics
/// If `compares` is empty.
pub fn compute_snakes_multilevel<T>(a: &[T], b: &[T], compares: &[Compare<T>]) -> Vec<Snake> {
    let fallible: Vec<_> = compares
        .iter()
        .map(|compare| move |x: &T, y: &T| Ok::<_, Infallible>(compare(x, y)))
        .collect();
    never_fails(try_compute_snakes_multilevel(a, b, &fallible))
}

/// [`compute_snakes_multilevel`] with predicates that can fail; the first error is returned.
///
/// # Panics
/// If `compares` is empty.
pub fn try_compute_snakes_multilevel<T, E, C>(
    a: &[T],
    b: &[T],
    compares: &[C],
) -> Result<Vec<Snake>, E>
where
    C: Fn(&T, &T) -> Result<bool, E>,
{
    assert!(!compares.is_empty(), "at least one predicate is required");
    let rect = Rect {
        i0: 0,
        j0: 0,
        i1: a.len(),
        j1: b.len(),
    };
    multilevel(a, b, compares, rect, compares.len() - 1)
}

fn multilevel<T, E, C>(
    a: &[T],
    b: &[T],
    compares: &[C],
    rect: Rect,
    level: usize,
) -> Result<Vec<Snake>, E>
where
    C: Fn(&T, &T) -> Result<bool, E>,
{
    let snakes = compute_snakes(a, b, &compares[level], rect)?;
    if level == 0 {
        return Ok(snakes);
    }

    let mut merged = vec![Snake { i: 0, j: 0, n: 0 }];
    let (mut i0, mut j0) = (rect.i0, rect.j0);
    let closing = Snake {
        i: rect.i1,
        j: rect.j1,
        n: 0,
    };
    for snake in snakes.into_iter().chain([closing]) {
        if snake.i > i0 && snake.j > j0 {
            let gap = Rect {
                i0,
                j0,
                i1: snake.i,
                j1: snake.j,
            };
            merged.extend(multilevel(a, b, compares, gap, level - 1)?);
        }
        if snake.n > 0 {
            match merged.last_mut() {
                Some(last) if last.i + last.n == snake.i && last.j + last.n == snake.j => {
                    last.n += snake.n;
                }
                _ => merged.push(snake),
            }
        }
        i0 = snake.i + snake.n;
        j0 = snake.j + snake.n;
    }
    if merged[0].n == 0 {
        merged.remove(0);
    }
    Ok(merged)
}

/// Converts snakes to a diff: the items between snakes are removed and added (`to_value` makes
/// the added items), and every aligned pair becomes a `patch` of `diff_pair`'s result when that
/// is not empty.
pub fn compute_diff_from_snakes<T, E>(
    a: &[T],
    b: &[T],
    snakes: &[Snake],
    to_value: impl Fn(&T) -> Value,
    mut diff_pair: impl FnMut(&T, &T) -> Result<Diff, E>,
) -> Result<Diff, E> {
    let mut di = SequenceDiffBuilder::new();
    let (mut i0, mut j0) = (0, 0);
    let closing = Snake {
        i: a.len(),
        j: b.len(),
        n: 0,
    };
    for snake in snakes.iter().chain([&closing]) {
        let Snake { i, j, n } = *snake;
        if i > i0 {
            di.removerange(i0, i - i0);
        }
        if j > j0 {
            di.addrange(i0, Value::List(b[j0..j].iter().map(&to_value).collect()));
        }
        for k in 0..n {
            di.patch(i + k, diff_pair(&a[i + k], &b[j + k])?);
        }
        i0 = i + n;
        j0 = j + n;
    }
    Ok(di.validated())
}

#[cfg(test)]
mod tests {
    use super::super::strings::diff_strings_by_char;
    use super::*;

    fn snakes<'a>(
        a: &[&'a str],
        b: &[&'a str],
        compares: &[Compare<&'a str>],
    ) -> Vec<(usize, usize, usize)> {
        compute_snakes_multilevel(a, b, compares)
            .into_iter()
            .map(|s| (s.i, s.j, s.n))
            .collect()
    }

    fn eq(x: &&str, y: &&str) -> bool {
        x == y
    }

    fn same_first_letter(x: &&str, y: &&str) -> bool {
        x.chars().next() == y.chars().next()
    }

    #[test]
    fn single_level_is_plain_lcs() {
        let (a, b) = (["a", "b", "c"], ["a", "c"]);
        assert_eq!(snakes(&a, &b, &[&eq]), [(0, 0, 1), (2, 1, 1)]);
        assert!(snakes(&[], &b, &[&eq]).is_empty());
    }

    #[test]
    fn loose_predicate_fills_gaps_between_strict_snakes() {
        let a = ["x1", "ant", "z1"];
        let b = ["y9", "apple", "z1"];
        // Strict alignment pairs only "z1"; the gap before it pairs the two "a" words.
        assert_eq!(snakes(&a, &b, &[&same_first_letter, &eq]), [(1, 1, 2)]);
    }

    #[test]
    fn recursion_results_are_not_merged_with_the_snake_before() {
        let a = ["a", "bb"];
        let b = ["a", "bc"];
        assert_eq!(
            snakes(&a, &b, &[&same_first_letter, &eq]),
            [(0, 0, 1), (1, 1, 1)]
        );
    }

    #[test]
    fn gaps_must_be_nonempty_in_both_dimensions() {
        let a = ["a", "q"];
        let b = ["q"];
        assert_eq!(snakes(&a, &b, &[&same_first_letter, &eq]), [(1, 0, 1)]);
    }

    #[test]
    fn diff_from_snakes_patches_aligned_pairs() {
        let a = ["ab\n", "x\n"];
        let b = ["ac\n", "x\n", "new\n"];
        let s = [Snake { i: 0, j: 0, n: 1 }, Snake { i: 1, j: 1, n: 1 }];
        let d = compute_diff_from_snakes(
            &a,
            &b,
            &s,
            |line| Value::from(*line),
            |x, y| Ok::<_, std::convert::Infallible>(diff_strings_by_char(x, y)),
        )
        .unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].key.as_index(), Some(0));
        assert_eq!(d[1].key.as_index(), Some(2));
    }
}
