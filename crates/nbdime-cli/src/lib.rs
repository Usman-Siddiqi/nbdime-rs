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
/// developer tool does.
const MAIN_STACK: usize = 512 << 20;

type Entry = fn(&str, &[String], &mut dyn Write) -> anyhow::Result<i32>;

/// Runs one of the commands as the process `argv` describes it and returns the exit status.
fn run(entry: Entry) -> i32 {
    let argv: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let prog = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let args: Vec<String> = argv.into_iter().skip(1).collect();
    let worker = std::thread::Builder::new()
        .stack_size(MAIN_STACK)
        .spawn(move || {
            let stdout = io::stdout();
            let mut out = io::BufWriter::new(stdout.lock());
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                entry(&prog, &args, &mut out)
            }));
            let _ = out.flush();
            match result {
                Ok(Ok(code)) => code,
                Ok(Err(error)) => {
                    eprintln!("{prog}: error: {error:#}");
                    1
                }
                Err(_) => {
                    eprintln!("{prog}: internal error");
                    1
                }
            }
        });
    match worker.map(|handle| handle.join()) {
        Ok(Ok(code)) => code,
        _ => 1,
    }
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
