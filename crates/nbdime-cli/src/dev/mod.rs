//! Function table for `nbdime-rs-dev batch`, used by the parity harness.
//!
//! Each entry takes the request's `args` object and returns a JSON value that the harness
//! compares with what upstream nbdime returns for the same call.

use anyhow::{anyhow, bail, Context, Result};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::Value;

/// Stack size for threads that run the core on untrusted input.
pub const WORKER_STACK: usize = 512 << 20;

pub fn dispatch(name: &str, args: &Value) -> Result<Value> {
    match name {
        "ping" => Ok(Value::from("pong")),
        "json.loads_dumps" => json_loads_dumps(args),
        "splitlines" => {
            let s = str_arg(args, "s")?;
            Ok(Value::List(
                nbdime_core::py::splitlines_keepends(s)
                    .into_iter()
                    .map(Value::from)
                    .collect(),
            ))
        }
        _ => bail!("unknown function {name:?}"),
    }
}

/// Parses `text` and serializes it with Python `json.dumps` options; returns the text.
fn json_loads_dumps(args: &Value) -> Result<Value> {
    let text = str_arg(args, "text")?;
    let value = json::parse(text).context("parse")?;
    let options = dump_options(args.get("options"))?;
    Ok(Value::from(json::dumps(&value, &options)))
}

fn dump_options(v: Option<&Value>) -> Result<DumpOptions> {
    let mut options = DumpOptions::python_default();
    let Some(v) = v else { return Ok(options) };
    if let Some(indent) = v.get("indent") {
        options.indent = match indent {
            Value::Null => None,
            Value::Int(i) => Some(
                i.as_i64()
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| anyhow!("bad indent"))?,
            ),
            _ => bail!("bad indent"),
        };
        if options.indent.is_some() {
            options.item_separator = ",".into();
        }
    }
    if let Some(Value::List(seps)) = v.get("separators") {
        let [item, key] = seps.as_slice() else {
            bail!("separators must have two items")
        };
        options.item_separator = item.as_str().context("separator")?.to_owned();
        options.key_separator = key.as_str().context("separator")?.to_owned();
    }
    if let Some(Value::Bool(b)) = v.get("ensure_ascii") {
        options.ensure_ascii = *b;
    }
    if let Some(Value::Bool(b)) = v.get("sort_keys") {
        options.sort_keys = *b;
    }
    Ok(options)
}

pub fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing string argument {name:?}"))
}

pub fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a Value> {
    args.get(name)
        .ok_or_else(|| anyhow!("missing argument {name:?}"))
}
