//! JSON values with Python semantics.
//!
//! nbdime operates on the objects produced by Python's `json` module, so equality, truthiness
//! and type checks here follow Python rather than JSON: `1 == 1.0 == True`, object key order is
//! preserved, and integers have arbitrary precision.

use std::cmp::Ordering;
use std::fmt;

use indexmap::IndexMap;

/// An insertion-ordered JSON object, like a Python `dict`.
pub type Map = IndexMap<String, Value>;

/// A JSON integer of arbitrary size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Int {
    Small(i64),
    /// Canonical decimal digits with an optional leading `-`, outside the `i64` range.
    Big(String),
}

impl Int {
    /// Parses a JSON integer literal (`-?(0|[1-9][0-9]*)`).
    pub fn from_literal(text: &str) -> Int {
        match text.parse::<i64>() {
            Ok(v) => Int::Small(v),
            Err(_) => Int::Big(text.to_owned()),
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Int::Small(v) => Some(*v),
            Int::Big(_) => None,
        }
    }

    fn is_zero(&self) -> bool {
        matches!(self, Int::Small(0))
    }

    fn is_negative(&self) -> bool {
        match self {
            Int::Small(v) => *v < 0,
            Int::Big(s) => s.starts_with('-'),
        }
    }

    /// Exact numeric comparison with an integer-valued, finite float.
    fn eq_float(&self, f: f64) -> bool {
        if !f.is_finite() || f.fract() != 0.0 {
            return false;
        }
        match self {
            // `f as i128` is exact for integral floats below 2^127 and saturates above, where no
            // i64 can be equal anyway.
            Int::Small(v) => f as i128 == i128::from(*v),
            Int::Big(s) => {
                let mut digits = format!("{f:.0}");
                if digits == "-0" {
                    digits = "0".into();
                }
                digits == *s
            }
        }
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Int::Small(v) => write!(f, "{v}"),
            Int::Big(s) => f.write_str(s),
        }
    }
}

impl From<i64> for Int {
    fn from(v: i64) -> Self {
        Int::Small(v)
    }
}

/// A JSON value as Python's `json` module represents it.
///
/// `PartialEq` is structural (types distinguished, object order significant) and is meant for
/// tests and exact output comparison. Use [`Value::py_eq`] wherever nbdime uses Python `==`.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(Int),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Object(Map),
}

/// Python type of a value, for `type(a) is type(b)` checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyType {
    None,
    Bool,
    Int,
    Float,
    Str,
    List,
    Dict,
}

impl Value {
    pub fn py_type(&self) -> PyType {
        match self {
            Value::Null => PyType::None,
            Value::Bool(_) => PyType::Bool,
            Value::Int(_) => PyType::Int,
            Value::Float(_) => PyType::Float,
            Value::Str(_) => PyType::Str,
            Value::List(_) => PyType::List,
            Value::Object(_) => PyType::Dict,
        }
    }

    /// `type(self) is type(other)` in Python.
    pub fn same_type(&self, other: &Value) -> bool {
        self.py_type() == other.py_type()
    }

    /// Python truthiness: `bool(self)`.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => !i.is_zero(),
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Object(m) => !m.is_empty(),
        }
    }

    /// Python `self == other` for values parsed from JSON.
    ///
    /// Numbers compare by mathematical value across `bool`, `int` and `float`; containers compare
    /// element-wise, dicts ignoring key order. `NaN` never equals anything: two values parsed
    /// independently are never the same Python object, so the identity shortcut of container
    /// comparison never applies.
    pub fn py_eq(&self, other: &Value) -> bool {
        use Value::*;
        match (self, other) {
            (Null, Null) => true,
            (Str(a), Str(b)) => a == b,
            (List(a), List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.py_eq(y)),
            (Object(a), Object(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| v.py_eq(w)))
            }
            (Bool(a), Bool(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Int(i), Float(f)) | (Float(f), Int(i)) => i.eq_float(*f),
            (Bool(b), Int(i)) | (Int(i), Bool(b)) => *i == crate::value::Int::Small(i64::from(*b)),
            (Bool(b), Float(f)) | (Float(f), Bool(b)) => *f == f64::from(u8::from(*b)),
            _ => false,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// `self[key]` for objects; `None` for missing keys and non-objects.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|m| m.get(key))
    }

    pub fn is_str(&self) -> bool {
        matches!(self, Value::Str(_))
    }

    pub fn is_list(&self) -> bool {
        matches!(self, Value::List(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }
}

/// Python's ordering of `int` values, used where nbdime sorts integer keys.
pub fn cmp_int(a: &Int, b: &Int) -> Ordering {
    match (a, b) {
        (Int::Small(x), Int::Small(y)) => x.cmp(y),
        _ => {
            let (an, bn) = (a.is_negative(), b.is_negative());
            if an != bn {
                return if an {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            let (sa, sb) = (a.to_string(), b.to_string());
            let (da, db) = (sa.trim_start_matches('-'), sb.trim_start_matches('-'));
            let magnitude = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if an {
                magnitude.reverse()
            } else {
                magnitude
            }
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(Int::Small(v))
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(Int::Small(i64::from(v)))
    }
}

impl From<usize> for Value {
    fn from(v: usize) -> Self {
        match i64::try_from(v) {
            Ok(v) => Value::Int(Int::Small(v)),
            Err(_) => Value::Int(Int::Big(v.to_string())),
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}

impl From<Map> for Value {
    fn from(m: Map) -> Self {
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i64) -> Value {
        Value::from(v)
    }

    #[test]
    fn numeric_cross_type_equality() {
        assert!(int(1).py_eq(&Value::Float(1.0)));
        assert!(Value::Bool(true).py_eq(&int(1)));
        assert!(Value::Bool(true).py_eq(&Value::Float(1.0)));
        assert!(Value::Bool(false).py_eq(&Value::Float(-0.0)));
        assert!(!int(1).py_eq(&Value::Float(1.5)));
        assert!(!Value::Float(f64::NAN).py_eq(&Value::Float(f64::NAN)));
        assert!(!int(1).py_eq(&Value::from("1")));
        assert!(!Value::Null.py_eq(&Value::Bool(false)));
    }

    #[test]
    fn big_int_equality() {
        let big = Value::Int(Int::from_literal("9007199254740993"));
        assert!(big.py_eq(&Value::Int(Int::Small(9007199254740993))));
        // 2**53 + 1 is not representable as a float; Python compares exactly.
        assert!(!big.py_eq(&Value::Float(9007199254740992.0)));
        let huge = Value::Int(Int::from_literal("100000000000000000000"));
        assert!(huge.py_eq(&Value::Float(1e20)));
        assert!(!huge.py_eq(&Value::Float(1e21)));
    }

    #[test]
    fn container_equality_ignores_dict_order() {
        let mut a = Map::new();
        a.insert("x".into(), int(1));
        a.insert("y".into(), Value::List(vec![Value::Float(2.0)]));
        let mut b = Map::new();
        b.insert("y".into(), Value::List(vec![int(2)]));
        b.insert("x".into(), Value::Bool(true));
        assert!(Value::Object(a.clone()).py_eq(&Value::Object(b)));
        let mut c = a.clone();
        c.insert("z".into(), Value::Null);
        assert!(!Value::Object(a).py_eq(&Value::Object(c)));
    }

    #[test]
    fn truthiness() {
        assert!(!Value::from("").truthy());
        assert!(Value::from(" ").truthy());
        assert!(!Value::List(vec![]).truthy());
        assert!(!Value::Float(-0.0).truthy());
        assert!(Value::Float(f64::NAN).truthy());
        assert!(!int(0).truthy());
    }

    #[test]
    fn same_type_distinguishes_numbers() {
        assert!(!Value::Bool(true).same_type(&int(1)));
        assert!(!int(1).same_type(&Value::Float(1.0)));
        assert!(Value::Int(Int::from_literal("99999999999999999999")).same_type(&int(3)));
    }

    #[test]
    fn int_ordering() {
        let a = Int::from_literal("-99999999999999999999");
        let b = Int::Small(-5);
        let c = Int::from_literal("99999999999999999999");
        assert_eq!(cmp_int(&a, &b), Ordering::Less);
        assert_eq!(cmp_int(&c, &b), Ordering::Greater);
        assert_eq!(cmp_int(&a, &c), Ordering::Less);
    }
}
