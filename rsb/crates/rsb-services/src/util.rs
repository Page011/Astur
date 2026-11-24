//! Small helpers shared by the services.

use std::collections::BTreeMap;

use rsb_core::value::Value;

/// A `Value::Map` from key/value pairs.
pub(crate) fn map<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<BTreeMap<_, _>>())
}

pub(crate) fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

/// Rounds to one decimal so a value that wobbles in the noise does not
/// produce an event every sample.
pub(crate) fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Cuts `s` to at most `max` chars.
pub(crate) fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max).collect() }
}
