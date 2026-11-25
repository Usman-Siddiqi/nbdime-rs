//! Running the engine with the GIL released.

use std::error::Error;
use std::panic::{catch_unwind, AssertUnwindSafe};

use pyo3::prelude::*;

use crate::EngineError;

pub type Failure = Box<dyn Error + Send + Sync>;

/// Stack space for deeply nested input (F-009, F-011).
const ENGINE_STACK: usize = 32 << 20;

/// Shallow input can use the existing stack if it has enough space.
const INLINE_DEPTH: usize = 24;
const INLINE_STACK: usize = 512 << 10;

/// Runs `work` without the GIL, switching stacks for deep input or low remaining stack space.
/// Failures and panics become `EngineError`.
pub fn run<T: Send>(
    py: Python<'_>,
    depth: usize,
    work: impl FnOnce() -> Result<T, Failure> + Send,
) -> PyResult<T> {
    let outcome = py.detach(|| {
        guarded(|| {
            if depth > INLINE_DEPTH {
                stacker::grow(ENGINE_STACK, work)
            } else {
                stacker::maybe_grow(INLINE_STACK, ENGINE_STACK, work)
            }
        })
    });
    outcome.map_err(EngineError::new_err)
}

fn guarded<T>(work: impl FnOnce() -> Result<T, Failure>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(format!("panic: {message}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_and_panics_become_engine_errors() {
        Python::attach(|py| {
            for depth in [0, INLINE_DEPTH + 1] {
                let err =
                    run(py, depth, || -> Result<(), Failure> { Err("boom".into()) }).unwrap_err();
                assert!(err.is_instance_of::<EngineError>(py));
                assert_eq!(err.value(py).to_string(), "boom");
                let err = run(py, depth, || -> Result<(), Failure> { panic!("oops") }).unwrap_err();
                assert_eq!(err.value(py).to_string(), "panic: oops");
                assert_eq!(run(py, depth, || Ok(7)).unwrap(), 7);
            }
        });
    }

    #[test]
    fn deep_work_gets_a_big_stack() {
        fn recurse(n: usize) -> usize {
            let pad = [n as u8; 1024];
            if n == 0 {
                0
            } else {
                recurse(n - 1) + usize::from(std::hint::black_box(pad)[0] == 255)
            }
        }
        Python::attach(|py| {
            // 10k frames of over 1 KB each would overflow a 2 MB stack.
            let n = run(py, INLINE_DEPTH + 1, || Ok(recurse(10_000))).unwrap();
            assert!(n <= 10_000);
        });
    }
}
