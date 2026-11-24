//! Values crossing between Rust and Luau. Both directions refuse nesting
//! deeper than `value::MAX_DEPTH` and tables that contain themselves, with a
//! Luau error, so a hostile or buggy script cannot recurse the host stack.
//! Long walks call a deadline check every 1024 items.

use mlua::{Error, Lua, Table, Value as Lv};
use rsb_core::value::{MAX_DEPTH, Value};

/// How often a long walk looks at the clock.
const CHECK_EVERY: u32 = 1024;

pub(crate) type Check<'a> = &'a dyn Fn() -> mlua::Result<()>;

pub(crate) struct Walk<'a> {
    check: Check<'a>,
    items: u32,
    path: Vec<usize>,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(check: Check<'a>) -> Walk<'a> {
        Walk { check, items: 0, path: Vec::new() }
    }

    fn tick(&mut self) -> mlua::Result<()> {
        self.items += 1;
        if self.items.is_multiple_of(CHECK_EVERY) { (self.check)() } else { Ok(()) }
    }
}

fn err(msg: impl Into<String>) -> Error {
    Error::runtime(msg.into())
}

pub(crate) fn from_lua(v: &Lv, walk: &mut Walk) -> mlua::Result<Value> {
    from_lua_at(v, walk, 0)
}

fn from_lua_at(v: &Lv, walk: &mut Walk, depth: usize) -> mlua::Result<Value> {
    walk.tick()?;
    Ok(match v {
        Lv::Nil => Value::Nil,
        Lv::Boolean(b) => Value::Bool(*b),
        Lv::Integer(i) => Value::Int(*i),
        Lv::Number(n) => {
            if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 {
                Value::Int(*n as i64)
            } else {
                Value::Num(*n)
            }
        }
        Lv::String(s) => Value::Str(s.to_string_lossy()),
        Lv::Table(t) => {
            if depth >= MAX_DEPTH {
                return Err(err(format!("a table is nested over {MAX_DEPTH} levels")));
            }
            let id = t.to_pointer() as usize;
            if walk.path.contains(&id) {
                return Err(err("a table contains itself"));
            }
            walk.path.push(id);
            let out = table_to_value(t, walk, depth);
            walk.path.pop();
            out?
        }
        other => return Err(err(format!("a {} cannot be sent here", lua_type(other)))),
    })
}

fn table_to_value(t: &Table, walk: &mut Walk, depth: usize) -> mlua::Result<Value> {
    let n = t.raw_len();
    let mut list = Vec::with_capacity(n);
    let mut other = std::collections::BTreeMap::new();
    let mut seq = true;
    let mut count = 0usize;
    for pair in t.pairs::<Lv, Lv>() {
        let (k, val) = pair?;
        walk.tick()?;
        count += 1;
        match &k {
            Lv::String(s) => {
                seq = false;
                other.insert(s.to_string_lossy(), from_lua_at(&val, walk, depth + 1)?);
            }
            Lv::Integer(_) | Lv::Number(_) => {
                let idx = match &k {
                    Lv::Integer(i) => *i as f64,
                    Lv::Number(x) => *x,
                    _ => unreachable!(),
                };
                if seq && idx >= 1.0 && idx <= n as f64 && idx.fract() == 0.0 {
                    // Filled below in order.
                } else {
                    seq = false;
                    other.insert(format!("{idx}"), from_lua_at(&val, walk, depth + 1)?);
                }
            }
            _ => return Err(err(format!("a table key of type {} cannot be sent here", lua_type(&k)))),
        }
    }
    if seq && count == n && n > 0 {
        for i in 1..=n {
            let val: Lv = t.raw_get(i)?;
            list.push(from_lua_at(&val, walk, depth + 1)?);
        }
        return Ok(Value::List(list));
    }
    if seq && count == 0 {
        return Ok(Value::Map(other));
    }
    // Mixed or sparse tables: integer keys become string keys.
    for i in 1..=n {
        let val: Lv = t.raw_get(i)?;
        if !matches!(val, Lv::Nil) {
            other.entry(i.to_string()).or_insert(from_lua_at(&val, walk, depth + 1)?);
        }
    }
    Ok(Value::Map(other))
}

pub(crate) fn lua_type(v: &Lv) -> &'static str {
    match v {
        Lv::Nil => "nil",
        Lv::Boolean(_) => "boolean",
        Lv::Integer(_) | Lv::Number(_) => "number",
        Lv::String(_) => "string",
        Lv::Table(_) => "table",
        Lv::Function(_) => "function",
        Lv::Thread(_) => "thread",
        Lv::UserData(_) | Lv::LightUserData(_) => "userdata",
        Lv::Vector(_) => "vector",
        Lv::Buffer(_) => "buffer",
        _ => "value",
    }
}

/// Builds the Luau value. `freeze` makes every table read-only (service
/// data is shared between the instances of one VM).
pub(crate) fn to_lua(lua: &Lua, v: &Value, freeze: bool) -> mlua::Result<Lv> {
    to_lua_at(lua, v, freeze, 0)
}

fn to_lua_at(lua: &Lua, v: &Value, freeze: bool, depth: usize) -> mlua::Result<Lv> {
    Ok(match v {
        Value::Nil => Lv::Nil,
        Value::Bool(b) => Lv::Boolean(*b),
        Value::Int(i) => Lv::Number(*i as f64),
        Value::Num(n) => Lv::Number(*n),
        Value::Str(s) => Lv::String(lua.create_string(s)?),
        Value::List(l) => {
            if depth >= MAX_DEPTH {
                return Err(err(format!("a value is nested over {MAX_DEPTH} levels")));
            }
            let t = lua.create_table_with_capacity(l.len(), 0)?;
            for (i, item) in l.iter().enumerate() {
                t.raw_set(i + 1, to_lua_at(lua, item, freeze, depth + 1)?)?;
            }
            if freeze {
                t.set_readonly(true);
            }
            Lv::Table(t)
        }
        Value::Map(m) => {
            if depth >= MAX_DEPTH {
                return Err(err(format!("a value is nested over {MAX_DEPTH} levels")));
            }
            let t = lua.create_table_with_capacity(0, m.len())?;
            for (k, item) in m {
                t.raw_set(k.as_str(), to_lua_at(lua, item, freeze, depth + 1)?)?;
            }
            if freeze {
                t.set_readonly(true);
            }
            Lv::Table(t)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> mlua::Result<()> {
        Ok(())
    }

    #[test]
    fn round_trip_and_shapes() {
        let lua = Lua::new();
        let v: Lv = lua.load(r#"return { a = 1, b = { 1, 2, 3 }, c = "x", d = true, e = {} }"#).eval().unwrap();
        let mut w = Walk::new(&ok);
        let got = from_lua(&v, &mut w).unwrap();
        assert_eq!(got.get("a"), Some(&Value::Int(1)));
        assert_eq!(got.get("b"), Some(&Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])));
        assert_eq!(got.get("e"), Some(&Value::Map(Default::default())));
        let back = to_lua(&lua, &got, true).unwrap();
        let mut w = Walk::new(&ok);
        assert_eq!(from_lua(&back, &mut w).unwrap(), got);
        if let Lv::Table(t) = back {
            assert!(t.is_readonly());
        }
    }

    #[test]
    fn cycles_and_depth_are_refused() {
        let lua = Lua::new();
        let cyc: Lv = lua.load("local t = {} t.self = t return t").eval().unwrap();
        let e = from_lua(&cyc, &mut Walk::new(&ok)).unwrap_err().to_string();
        assert!(e.contains("contains itself"), "{e}");
        let deep: Lv = lua.load("local t = {} for i = 1, 70 do t = { t } end return t").eval().unwrap();
        let e = from_lua(&deep, &mut Walk::new(&ok)).unwrap_err().to_string();
        assert!(e.contains("nested over 64"), "{e}");
        let f: Lv = lua.load("return function() end").eval().unwrap();
        assert!(from_lua(&f, &mut Walk::new(&ok)).is_err());
    }

    #[test]
    fn deadline_is_checked_on_big_inputs() {
        let lua = Lua::new();
        let big: Lv = lua.load("local t = {} for i = 1, 5000 do t[i] = i end return t").eval().unwrap();
        let calls = std::cell::Cell::new(0);
        let check = || {
            calls.set(calls.get() + 1);
            Ok(())
        };
        from_lua(&big, &mut Walk::new(&check)).unwrap();
        assert!(calls.get() >= 4, "{}", calls.get());
    }
}
