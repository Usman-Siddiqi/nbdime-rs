//! Library half of the nbdime-rs command line tools.

pub mod argparse;
mod argparse_help;
pub mod args;
pub mod config;
pub mod dev;
pub mod diffdriver;
pub mod difftool;
pub mod dispatch;
pub mod filter_integration;
pub mod gitattributes;
pub mod gitfiles;
pub mod logging;
pub mod mergedriver;
pub mod mergetool;
pub mod nbdiffapp;
pub mod nbmergeapp;
pub mod nbpatchapp;
pub mod nbshowapp;
pub mod osstr;
pub mod pathexpand;
pub mod revparse;
#[cfg(test)]
mod testutil;
pub mod utils;

use std::io::{self, Write};
use std::path::Path;

/// What `--version` prints after the program name.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (nbdime-rs; compatible with nbdime 4.0.4)"
);

/// The command line tools run the core on untrusted input; give them a roomy stack, like the
/// developer tool does. Where the address space is limited the stack shrinks until a thread can
/// be had, and the main thread is the last resort.
const STACK_SIZES: [usize; 5] = [512 << 20, 128 << 20, 32 << 20, 8 << 20, 2 << 20];

type Entry = fn(&str, &[String], &mut dyn Write) -> anyhow::Result<i32>;

fn run_entry(entry: Entry, prog: &str, args: &[String]) -> i32 {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| entry(prog, args, &mut out)));
    let _ = out.flush();
    match result {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => match error.downcast_ref::<args::SystemExit>() {
            Some(args::SystemExit(code)) => *code,
            None => {
                osstr::eprint(&format!("{prog}: error: {error:#}\n"));
                1
            }
        },
        Err(_) => {
            osstr::eprint(&format!("{prog}: internal error\n"));
            1
        }
    }
}

/// Runs one of the commands as the process `argv` describes it and returns the exit status.
fn run(entry: Entry) -> i32 {
    let argv: Vec<String> = std::env::args_os()
        .map(|arg| osstr::from_os(&arg))
        .collect();
    let prog = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let args: Vec<String> = argv.into_iter().skip(1).collect();
    for &size in &stack_sizes_to_try() {
        let (prog, args) = (prog.clone(), args.clone());
        let spawned = std::thread::Builder::new()
            .stack_size(size)
            .spawn(move || run_entry(entry, &prog, &args));
        if let Ok(handle) = spawned {
            return handle.join().unwrap_or(1);
        }
    }
    run_entry(entry, &prog, &args)
}

/// The stack sizes to try, in order. When the largest size does not map, the address space is
/// limited, and the largest size that does map would leave the heap nothing: the first size that
/// maps is only a probe, and the run starts from the next smaller one.
fn stack_sizes_to_try() -> Vec<usize> {
    for (i, &size) in STACK_SIZES.iter().enumerate() {
        let probe = std::thread::Builder::new().stack_size(size).spawn(|| ());
        if let Ok(handle) = probe {
            let _ = handle.join();
            let start = if i == 0 {
                0
            } else {
                (i + 1).min(STACK_SIZES.len() - 1)
            };
            return STACK_SIZES[start..].to_vec();
        }
    }
    Vec::new()
}

pub fn nbdiff_main() -> i32 {
    run(nbdiffapp::main)
}

pub fn nbmerge_main() -> i32 {
    run(nbmergeapp::main)
}

pub fn nbpatch_main() -> i32 {
    run(nbpatchapp::main)
}

pub fn nbshow_main() -> i32 {
    run(nbshowapp::main)
}

pub fn nbdime_main() -> i32 {
    run(dispatch::main)
}

pub fn git_nbdiffdriver_main() -> i32 {
    run(diffdriver::main)
}

pub fn git_nbmergedriver_main() -> i32 {
    run(mergedriver::main)
}

pub fn git_nbdifftool_main() -> i32 {
    run(difftool::main)
}

pub fn git_nbmergetool_main() -> i32 {
    run(mergetool::main)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_size_is_always_on_offer_and_never_the_probe_alone() {
        let sizes = stack_sizes_to_try();
        assert!(!sizes.is_empty());
        assert!(sizes.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(STACK_SIZES.contains(&sizes[0]));
    }
}
