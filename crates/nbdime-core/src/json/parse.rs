//! A JSON parser that accepts what Python's `json.loads` accepts and builds the same values.
//!
//! Differences from strict RFC 8259 parsers that matter for parity: `NaN`, `Infinity` and
//! `-Infinity` are accepted, integers have arbitrary precision, and a duplicated object key keeps
//! the position of its first occurrence with the value of its last. The parser is iterative and
//! accepts the same maximum nesting depth as CPython's `json` module ([`MAX_DEPTH`]).

use crate::value::{Int, Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}: line {line} column {column} (char {offset})")]
pub struct ParseError {
    pub message: String,
    /// Offset in code points, as Python reports it.
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

/// How lone UTF-16 surrogates in `\u` escapes are handled.
///
/// Python keeps them in the string; Rust strings cannot hold them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoneSurrogates {
    /// Replace each with U+FFFD and count it in [`Parsed::replaced_surrogates`].
    #[default]
    Replace,
    Reject,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ParseOptions {
    pub lone_surrogates: LoneSurrogates,
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub value: Value,
    pub replaced_surrogates: usize,
}

/// Deepest nesting CPython 3.13's `json.loads` accepts from a shallow call stack.
pub const MAX_DEPTH: usize = 9998;

pub fn parse(text: &str) -> Result<Value, ParseError> {
    parse_with(text, ParseOptions::default()).map(|p| p.value)
}

pub fn parse_with(text: &str, options: ParseOptions) -> Result<Parsed, ParseError> {
    let mut parser = Parser {
        src: text,
        bytes: text.as_bytes(),
        pos: 0,
        options,
        replaced_surrogates: 0,
    };
    if text.starts_with('\u{feff}') {
        return Err(parser.error_at(0, "Unexpected UTF-8 BOM (decode using utf-8-sig)"));
    }
    parser.skip_ws();
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.error("Extra data"));
    }
    Ok(Parsed {
        value,
        replaced_surrogates: parser.replaced_surrogates,
    })
}

enum Frame {
    List(Vec<Value>),
    Object(Map, String),
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    options: ParseOptions,
    replaced_surrogates: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        self.error_at(self.pos, message)
    }

    fn error_at(&self, pos: usize, message: &str) -> ParseError {
        let before = &self.src[..pos.min(self.src.len())];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        ParseError {
            message: message.to_owned(),
            offset: before.chars().count(),
            line,
            column: before[line_start..].chars().count() + 1,
        }
    }

    fn skip_ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.bytes.get(self.pos) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat_literal(&mut self, lit: &str) -> bool {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        let mut stack: Vec<Frame> = Vec::new();
        'outer: loop {
            self.skip_ws();
            // Parse one value, or open a container and continue with its first element.
            if matches!(self.peek(), Some(b'{' | b'[')) && stack.len() >= MAX_DEPTH {
                return Err(self.error("maximum recursion depth exceeded while decoding JSON"));
            }
            let mut value = match self.peek() {
                Some(b'{') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        Value::Object(Map::new())
                    } else {
                        let key = self.parse_key()?;
                        stack.push(Frame::Object(Map::new(), key));
                        continue 'outer;
                    }
                }
                Some(b'[') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        self.pos += 1;
                        Value::List(Vec::new())
                    } else {
                        stack.push(Frame::List(Vec::new()));
                        continue 'outer;
                    }
                }
                _ => self.parse_scalar()?,
            };

            // Attach the finished value to its parent, closing containers as they end.
            loop {
                let Some(frame) = stack.last_mut() else {
                    return Ok(value);
                };
                self.skip_ws();
                match frame {
                    Frame::List(items) => {
                        items.push(value);
                        match self.peek() {
                            Some(b',') => {
                                self.pos += 1;
                                continue 'outer;
                            }
                            Some(b']') => {
                                self.pos += 1;
                                let Some(Frame::List(items)) = stack.pop() else {
                                    unreachable!("top frame is a list")
                                };
                                value = Value::List(items);
                            }
                            _ => return Err(self.error("Expecting ',' delimiter")),
                        }
                    }
                    Frame::Object(map, key) => {
                        // `dict` assignment: a repeated key keeps its first position.
                        map.insert(std::mem::take(key), value);
                        match self.peek() {
                            Some(b',') => {
                                self.pos += 1;
                                self.skip_ws();
                                *key = self.parse_key()?;
                                continue 'outer;
                            }
                            Some(b'}') => {
                                self.pos += 1;
                                let Some(Frame::Object(map, _)) = stack.pop() else {
                                    unreachable!("top frame is an object")
                                };
                                value = Value::Object(map);
                            }
                            _ => return Err(self.error("Expecting ',' delimiter")),
                        }
                    }
                }
            }
        }
    }

    /// Parses `"key" :` with the colon consumed.
    fn parse_key(&mut self) -> Result<String, ParseError> {
        if self.peek() != Some(b'"') {
            return Err(self.error("Expecting property name enclosed in double quotes"));
        }
        let key = self.parse_string()?;
        self.skip_ws();
        if self.peek() != Some(b':') {
            return Err(self.error("Expecting ':' delimiter"));
        }
        self.pos += 1;
        Ok(key)
    }

    fn parse_scalar(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some(b'"') => self.parse_string().map(Value::Str),
            Some(b'n') if self.eat_literal("null") => Ok(Value::Null),
            Some(b't') if self.eat_literal("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat_literal("false") => Ok(Value::Bool(false)),
            Some(b'N') if self.eat_literal("NaN") => Ok(Value::Float(f64::NAN)),
            Some(b'I') if self.eat_literal("Infinity") => Ok(Value::Float(f64::INFINITY)),
            Some(b'-') if self.bytes[self.pos..].starts_with(b"-Infinity") => {
                self.pos += "-Infinity".len();
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(self.error("Expecting value")),
        }
    }

    /// Python's `NUMBER_RE`: `(-?(?:0|[1-9]\d*))(\.\d+)?([eE][-+]?\d+)?`.
    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        let digits = |p: &mut Self| {
            let s = p.pos;
            while let Some(b'0'..=b'9') = p.peek() {
                p.pos += 1;
            }
            p.pos - s
        };
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => {
                self.pos = start;
                return Err(self.error("Expecting value"));
            }
        }
        let int_end = self.pos;
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            let save = self.pos;
            self.pos += 1;
            if digits(self) == 0 {
                self.pos = save;
            } else {
                is_float = true;
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            let save = self.pos;
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if digits(self) == 0 {
                self.pos = save;
            } else {
                is_float = true;
            }
        }
        let text = &self.src[start..self.pos];
        if is_float {
            // Rust's float parsing is correctly rounded, like Python's `float()`; overflow gives
            // infinity in both.
            let f: f64 = text
                .parse()
                .map_err(|_| self.error_at(start, "Invalid number"))?;
            Ok(Value::Float(f))
        } else {
            debug_assert_eq!(int_end, self.pos);
            let canonical = if text == "-0" { "0" } else { text };
            Ok(Value::Int(Int::from_literal(canonical)))
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        let open = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let chunk_start = self.pos;
            while let Some(&b) = self.bytes.get(self.pos) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(&self.src[chunk_start..self.pos]);
            match self.peek() {
                None => return Err(self.error_at(open, "Unterminated string starting at")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    self.parse_escape(&mut out)?;
                }
                Some(_) => return Err(self.error("Invalid control character at")),
            }
        }
    }

    fn parse_escape(&mut self, out: &mut String) -> Result<(), ParseError> {
        let esc_pos = self.pos - 1;
        let c = match self.peek() {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                self.pos += 1;
                let hi = self.parse_hex4(esc_pos)?;
                let c = if (0xd800..0xdc00).contains(&hi)
                    && self.bytes[self.pos..].starts_with(b"\\u")
                {
                    let save = self.pos;
                    self.pos += 2;
                    let lo = self.parse_hex4(esc_pos)?;
                    if (0xdc00..0xe000).contains(&lo) {
                        char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00))
                    } else {
                        self.pos = save;
                        None
                    }
                } else {
                    char::from_u32(hi)
                };
                let c = match c {
                    Some(c) => c,
                    None => match self.options.lone_surrogates {
                        LoneSurrogates::Replace => {
                            self.replaced_surrogates += 1;
                            char::REPLACEMENT_CHARACTER
                        }
                        LoneSurrogates::Reject => {
                            return Err(self.error_at(esc_pos, "Lone surrogate in string"))
                        }
                    },
                };
                out.push(c);
                return Ok(());
            }
            _ => return Err(self.error_at(esc_pos, "Invalid \\escape")),
        };
        self.pos += 1;
        out.push(c);
        Ok(())
    }

    fn parse_hex4(&mut self, esc_pos: usize) -> Result<u32, ParseError> {
        let hex = self
            .bytes
            .get(self.pos..self.pos + 4)
            .and_then(|h| std::str::from_utf8(h).ok())
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.error_at(esc_pos, "Invalid \\uXXXX escape"))?;
        self.pos += 4;
        Ok(u32::from_str_radix(hex, 16).expect("validated hex digits"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: &[(&str, Value)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn scalars() {
        assert_eq!(parse("null").unwrap(), Value::Null);
        assert_eq!(parse(" true ").unwrap(), Value::Bool(true));
        assert_eq!(parse("-0").unwrap(), Value::from(0));
        assert_eq!(parse("-0.0").unwrap(), Value::Float(-0.0));
        assert_eq!(parse("1E2").unwrap(), Value::Float(100.0));
        assert_eq!(parse("1e400").unwrap(), Value::Float(f64::INFINITY));
        assert!(matches!(parse("NaN").unwrap(), Value::Float(f) if f.is_nan()));
        assert_eq!(parse("-Infinity").unwrap(), Value::Float(f64::NEG_INFINITY));
        assert_eq!(
            parse("123456789012345678901234567890").unwrap(),
            Value::Int(Int::Big("123456789012345678901234567890".into()))
        );
    }

    #[test]
    fn rejects_what_python_rejects() {
        for bad in [
            "",
            "01",
            "1.",
            ".5",
            "+1",
            "[1,]",
            "{\"a\":1,}",
            "'a'",
            "\"\t\"",
            "[1 2]",
            "nul",
            "\u{feff}{}",
            "{\"a\" 1}",
            "\"\\x\"",
            "\"\\u12\"",
            "1 2",
        ] {
            assert!(parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn duplicate_keys_keep_first_position_last_value() {
        assert_eq!(
            parse(r#"{"a": 1, "b": 2, "a": 3}"#).unwrap(),
            obj(&[("a", Value::from(3)), ("b", Value::from(2))])
        );
    }

    #[test]
    fn escapes_and_surrogates() {
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), Value::from("😀"));
        assert_eq!(parse(r#""a\/b\n""#).unwrap(), Value::from("a/b\n"));
        let parsed = parse_with(r#""\ud800x""#, ParseOptions::default()).unwrap();
        assert_eq!(parsed.value, Value::from("\u{fffd}x"));
        assert_eq!(parsed.replaced_surrogates, 1);
        let reject = ParseOptions {
            lone_surrogates: LoneSurrogates::Reject,
        };
        assert!(parse_with(r#""\udc00""#, reject).is_err());
    }

    #[test]
    fn nesting_limit_matches_python() {
        // Values this deep are dropped recursively, so use a thread with a roomy stack.
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(|| {
                let nested = |d: usize| "[".repeat(d) + &"]".repeat(d);
                assert!(parse(&nested(MAX_DEPTH)).is_ok());
                assert!(parse(&nested(MAX_DEPTH + 1)).is_err());
                let objects = "{\"a\":".repeat(MAX_DEPTH) + "1" + &"}".repeat(MAX_DEPTH);
                assert!(parse(&objects).is_ok());
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
