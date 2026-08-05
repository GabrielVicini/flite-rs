//! The dynamically typed value used throughout linguistic feature evaluation.
//!
//! Feature values flow between three worlds that disagree about types: item
//! features set by the pipeline, feature *functions* computed on demand, and
//! constants baked into the CART trees. Comparison semantics follow the
//! Festival lineage the trees were trained under, and are easy to get subtly
//! wrong; see [`Value::equals`].

use std::fmt;
use std::sync::Arc;

/// Largest count the models can represent as a string.
///
/// Feature values reach the trained trees as strings drawn from a fixed table,
/// so counts above this saturate rather than growing. Distinguishing "22
/// syllables from the phrase start" from "37" would be meaningless to a model
/// that only ever saw the former.
pub const COUNT_MAX: i32 = 24;

/// A linguistic feature value.
///
/// `Str` holds an `Arc<str>` because values are read far more often than they
/// are created (a single CART walk may re-read the same item feature dozens of
/// times), and cloning must stay free of allocation.
#[derive(Clone, Debug)]
pub enum Value {
    Int(i32),
    Float(f32),
    Str(Arc<str>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(Arc::from(s))
    }

    /// The canonical "feature not found" value. Absent features are not an
    /// error in this model: trees routinely ask items for features that only
    /// exist on other item types, and the trained models expect `"0"` back.
    pub fn zero() -> Value {
        Value::str("0")
    }

    /// A count, as the string the models expect.
    ///
    /// Counts saturate at [`COUNT_MAX`] and negatives read as `"0"`. The
    /// models were trained against a fixed table of small-integer strings and
    /// never saw anything larger, so a long sentence must not suddenly hand
    /// them `"37"`. See [`COUNT_MAX`].
    pub fn int_str(n: i32) -> Value {
        const SMALL: [&str; COUNT_MAX as usize + 1] = [
            "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
            "16", "17", "18", "19", "20", "21", "22", "23", "24",
        ];
        Value::str(SMALL[n.clamp(0, COUNT_MAX) as usize])
    }

    /// String contents, or `""` for numeric values.
    ///
    /// Upstream raises an error when a non-string is read as a string; every
    /// caller here is a comparison or a name lookup where an empty string is
    /// the harmless answer, so we degrade instead of panicking.
    pub fn as_str(&self) -> &str {
        match self {
            Value::Str(s) => s,
            _ => "",
        }
    }

    /// Numeric coercion. Strings parse as far as they can and yield `0.0`
    /// otherwise, matching `atof`. Trees compare string-valued features such
    /// as `syl_break` against float thresholds and rely on this.
    pub fn as_f32(&self) -> f32 {
        match self {
            Value::Int(i) => *i as f32,
            Value::Float(f) => *f,
            Value::Str(s) => parse_leading_f32(s),
        }
    }

    pub fn as_i32(&self) -> i32 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i32,
            Value::Str(s) => parse_leading_f32(s) as i32,
        }
    }

    /// Equality *including* type: a string `"1"` is not equal to an integer 1.
    ///
    /// This mirrors the trained trees, whose `IS` questions were compiled
    /// against string constants. A feature function that returns an integer
    /// therefore never matches such a question, which is why several
    /// count-like features here deliberately return [`Value::int_str`].
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            _ => false,
        }
    }

    /// Ordering questions coerce both sides to float regardless of type.
    pub fn less_than(&self, other: &Value) -> bool {
        self.as_f32() < other.as_f32()
    }

    pub fn greater_than(&self, other: &Value) -> bool {
        self.as_f32() > other.as_f32()
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Str(s) => f.write_str(s),
        }
    }
}

/// Parse the longest numeric prefix of `s`, like C's `atof`.
fn parse_leading_f32(s: &str) -> f32 {
    let bytes = s.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    while end < bytes.len() {
        match bytes[end] {
            b'0'..=b'9' => seen_digit = true,
            b'-' | b'+' if end == 0 => {}
            b'.' if !seen_dot => seen_dot = true,
            _ => break,
        }
        end += 1;
    }
    if !seen_digit {
        return 0.0;
    }
    s[..end].parse().unwrap_or(0.0)
}

/// An ordered set of named values, as attached to an item or an utterance.
///
/// Items carry a handful of features at most, so a linear scan beats hashing
/// and keeps insertion order stable for debugging.
#[derive(Clone, Debug, Default)]
pub struct Features {
    entries: Vec<(Box<str>, Value)>,
}

impl Features {
    pub fn new() -> Features {
        Features::default()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(k, _)| &**k == name)
            .map(|(_, v)| v)
    }

    pub fn set(&mut self, name: &str, value: Value) {
        match self.entries.iter_mut().find(|(k, _)| &**k == name) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((Box::from(name), value)),
        }
    }

    pub fn set_str(&mut self, name: &str, value: &str) {
        self.set(name, Value::str(value));
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|(k, _)| &**k != name);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (&**k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_is_type_sensitive() {
        assert!(Value::str("1").equals(&Value::str("1")));
        assert!(!Value::str("1").equals(&Value::Int(1)));
        assert!(Value::Int(1).equals(&Value::Int(1)));
    }

    #[test]
    fn ordering_coerces_strings() {
        assert!(Value::str("1").less_than(&Value::Float(1.5)));
        assert!(Value::str("-0.5").less_than(&Value::Float(0.0)));
        assert!(!Value::str("mid").less_than(&Value::Float(0.0)));
    }

    #[test]
    fn atof_style_parsing() {
        assert_eq!(parse_leading_f32("12abc"), 12.0);
        assert_eq!(parse_leading_f32("abc"), 0.0);
        assert_eq!(parse_leading_f32("-3.25"), -3.25);
    }
}
