//! JSON text <-> [`Value`](crate::Value) with Python `json` module semantics.

mod dump;
mod parse;
mod scan;

pub use dump::{dumps, py_float_repr, write_float, write_string, write_value, DumpOptions};
pub use parse::{parse, parse_with, LoneSurrogates, ParseError, ParseOptions, Parsed, MAX_DEPTH};
