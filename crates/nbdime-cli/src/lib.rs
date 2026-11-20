//! Library half of the nbdime-rs command line tools.

pub mod argparse;
pub mod args;
pub mod config;
pub mod dev;
pub mod dispatch;
pub mod gitfiles;
pub mod logging;
pub mod nbdiffapp;
pub mod nbmergeapp;
pub mod osstr;
pub mod pathexpand;
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
    let mut out = osstr::StdoutEscaped(io::BufWriter::new(stdout.lock()));
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| entry(prog, args, &mut out)));
    let _ = out.flush();
    match result {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            osstr::eprint(&format!("{prog}: error: {error:#}\n"));
            1
        }
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
    for size in STACK_SIZES {
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

pub fn nbdiff_main() -> i32 {
    run(nbdiffapp::main)
}

pub fn nbmerge_main() -> i32 {
    run(nbmergeapp::main)
}

pub fn nbdime_main() -> i32 {
    run(dispatch::main)
}
