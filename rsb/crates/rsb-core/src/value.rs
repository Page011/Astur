//! A small dynamic value: widget options from `bar.toml`, service values,
//! event payloads. Converts losslessly to and from JSON and to Luau tables.
//!
//! Conversions to and from Luau (in rsb-script) refuse nesting deeper than
//! `MAX_DEPTH` and tables that contain themselves, with a Luau error, so a
//! hostile or buggy script cannot recurse the host thread's stack.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

/// Deepest nesting of lists and maps accepted anywhere a `Value` crosses
/// into or out of Luau (event data, JSON, HTTP bodies, options).
pub const MAX_DEPTH: usize = 64;

impl Value {
    /// Nesting depth: 0 for a scalar, 1 for a flat list or map.
    pub fn depth(&self) -> usize {
        match self {
            Value::List(l) => 1 + l.iter().map(Value::depth).max().unwrap_or(0),
            Value::Map(m) => 1 + m.values().map(Value::depth).max().unwrap_or(0),
            _ => 0,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Luau type name of the value, for messages ("expected string, got number").
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::List(_) | Value::Map(_) => "table",
        }
    }

    /// JSON objects become maps, arrays lists, integers that fit i64 `Int`.
    pub fn from_json(v: &serde_json::Value) -> Value {
        match v {
            serde_json::Value::Null => Value::Nil,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::Num(n.as_f64().unwrap_or(0.0)),
            },
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Array(a) => Value::List(a.iter().map(Value::from_json).collect()),
            serde_json::Value::Object(o) => {
                Value::Map(o.iter().map(|(k, v)| (k.clone(), Value::from_json(v))).collect())
            }
        }
    }

    /// Non-finite numbers become null.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Nil => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Int(i) => serde_json::Value::from(*i),
            Value::Num(n) => {
                serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, serde_json::Value::Number)
            }
            Value::Str(s) => serde_json::Value::String(s.clone()),
            Value::List(l) => serde_json::Value::Array(l.iter().map(Value::to_json).collect()),
            Value::Map(m) => serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let j: serde_json::Value = serde_json::from_str(r#"{"a":[1,2.5,"x",null,true],"b":{}}"#).unwrap();
        assert_eq!(Value::from_json(&j).to_json(), j);
    }
}
