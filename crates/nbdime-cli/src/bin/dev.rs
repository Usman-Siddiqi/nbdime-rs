//! `nbdime-rs-dev`: developer tool used by the parity harness.
//!
//! `nbdime-rs-dev batch` reads JSON Lines requests `{"id", "fn", "args"}` from stdin and writes
//! one response `{"id", "ok", "result" | "error"}` per line to stdout.

use std::io::{self, BufRead, BufWriter, Write};

use anyhow::{bail, Context, Result};
use nbdime_cli::dev;
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::{Map, Value};

fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode != "batch" {
        bail!("usage: nbdime-rs-dev batch < requests.jsonl");
    }
    // Deeply nested inputs are processed recursively; give the worker a roomy stack.
    std::thread::Builder::new()
        .stack_size(dev::WORKER_STACK)
        .spawn(run_batch)
        .context("spawning worker thread")?
        .join()
        .map_err(|_| anyhow::anyhow!("worker thread panicked"))?
}

fn run_batch() -> Result<()> {
    let stdin = io::stdin();
    let mut out = BufWriter::new(io::stdout().lock());
    let compact = DumpOptions::compact();
    for line in stdin.lock().lines() {
        let line = line.context("reading request")?;
        if line.trim().is_empty() {
            continue;
        }
        let mut response = Map::new();
        match json::parse(&line) {
            Ok(request) => {
                response.insert(
                    "id".into(),
                    request.get("id").cloned().unwrap_or(Value::Null),
                );
                let name = request.get("fn").and_then(Value::as_str).unwrap_or("");
                let args = request.get("args").cloned().unwrap_or(Value::Null);
                let outcome = std::panic::catch_unwind(|| dev::dispatch(name, &args))
                    .unwrap_or_else(|panic| {
                        Err(anyhow::anyhow!("panic: {}", panic_message(&panic)))
                    });
                match outcome {
                    Ok(result) => {
                        response.insert("ok".into(), Value::Bool(true));
                        response.insert("result".into(), result);
                    }
                    Err(e) => {
                        response.insert("ok".into(), Value::Bool(false));
                        response.insert("error".into(), Value::from(format!("{e:#}")));
                    }
                }
            }
            Err(e) => {
                response.insert("id".into(), Value::Null);
                response.insert("ok".into(), Value::Bool(false));
                response.insert("error".into(), Value::from(format!("bad request: {e}")));
            }
        }
        writeln!(out, "{}", json::dumps(&Value::Object(response), &compact))?;
        out.flush()?;
    }
    Ok(())
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}
