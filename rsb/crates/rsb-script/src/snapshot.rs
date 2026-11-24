//! Reads an instance's live node tree (Luau tables the prelude keeps) into
//! the plain `Element` tree the scene consumes. Caps on node count and depth
//! stop the instance instead of letting a huge tree reach layout.

use std::sync::Arc;

use mlua::{Table, Value as Lv};
use rsb_core::diag::SourcePos;
use rsb_core::element::{Element, ElementKind, HandlerId, IconRef, ImageRef, InstanceId};
use rsb_core::style::FlexDirection;

use crate::shared::{Shared, Stop};

struct Walk<'a> {
    sh: &'a Shared,
    inst: InstanceId,
    count: usize,
    max_nodes: usize,
    max_depth: usize,
    max_text: usize,
}

pub(crate) fn snapshot(sh: &Shared, inst: InstanceId, root: &Table, error_class: bool) -> mlua::Result<Element> {
    let lim = &sh.env.limits;
    let mut w = Walk {
        sh,
        inst,
        count: 0,
        max_nodes: lim.max_nodes_per_instance,
        max_depth: lim.max_element_depth,
        max_text: lim.max_text_chars,
    };
    let mut e = node(&mut w, root, 1)?;
    if error_class {
        e.classes.push(Arc::from("error"));
    }
    Ok(e)
}

fn get_str(t: &Table, key: &str) -> mlua::Result<Option<String>> {
    Ok(match t.raw_get::<Lv>(key)? {
        Lv::String(s) => Some(s.to_string_lossy()),
        _ => None,
    })
}

fn get_num(t: &Table, key: &str) -> mlua::Result<Option<f64>> {
    Ok(match t.raw_get::<Lv>(key)? {
        Lv::Integer(i) => Some(i as f64),
        Lv::Number(n) => Some(n),
        _ => None,
    })
}

fn handler(t: &Table, key: &str) -> mlua::Result<Option<HandlerId>> {
    Ok(get_num(t, key)?.map(|n| HandlerId(n as u32)))
}

/// A text element is one line: what comes before the first line break (command output ends in one).
fn one_line(text: &str) -> &str {
    text.split(['\n', '\r']).next().unwrap_or("")
}

fn cut(text: &str, max: usize) -> Arc<str> {
    if text.chars().count() <= max { Arc::from(text) } else { Arc::from(text.chars().take(max).collect::<String>()) }
}

fn node(w: &mut Walk, t: &Table, depth: usize) -> mlua::Result<Element> {
    w.count += 1;
    if w.count > w.max_nodes {
        return Err(w.sh.kill(Stop::Nodes(w.max_nodes)));
    }
    if depth > w.max_depth {
        return Err(w.sh.kill(Stop::Depth(w.max_depth)));
    }
    let kind = match get_str(t, "kind")?.as_deref() {
        Some("text") => ElementKind::Text,
        Some("icon") => ElementKind::Icon,
        Some("image") => ElementKind::Image,
        _ => ElementKind::Box,
    };
    let mut e = Element::new(kind);
    e.key = get_str(t, "key")?.map(|s| Arc::from(s.as_str()));
    e.id = get_str(t, "id")?.map(|s| Arc::from(s.as_str()));
    if let Some(cls) = get_str(t, "cls")? {
        e.classes = cls.split_whitespace().map(Arc::from).collect();
    }
    if let Lv::Table(style) = t.raw_get::<Lv>("style")? {
        for pair in style.pairs::<String, String>() {
            e.style.push(pair?);
        }
        e.style.sort();
    }
    if matches!(t.raw_get::<Lv>("vis")?, Lv::Boolean(false)) {
        e.style.retain(|(k, _)| k != "display");
        e.style.push(("display".into(), "none".into()));
    }
    if let Some(file) = get_str(t, "file")? {
        let line = get_num(t, "line")?.unwrap_or(0.0) as u32;
        e.pos = Some(SourcePos::new(&file, line, 0));
    }
    if get_str(t, "dir")?.as_deref() == Some("column") {
        e.direction = FlexDirection::Column;
    }
    match kind {
        ElementKind::Text => {
            if let Some(text) = get_str(t, "text")? {
                e.text = Some(cut(one_line(&text), w.max_text));
            } else {
                e.text = Some(Arc::from(""));
            }
        }
        ElementKind::Icon => {
            if let Some(name) = get_str(t, "iname")?.filter(|s| !s.is_empty()) {
                e.icon = Some(IconRef::Named(Arc::from(name.as_str())));
            } else if let Some(glyph) = get_str(t, "iglyph")? {
                e.icon = Some(IconRef::Glyph(cut(&glyph, w.max_text)));
            }
        }
        ElementKind::Image => {
            if let Some(src) = get_str(t, "src")? {
                let path = w.sh.permitted_path(w.inst, &src)?;
                e.image = Some(ImageRef {
                    path: Arc::from(path.to_string_lossy().as_ref()),
                    width: get_num(t, "w")?.map(|n| n as f32),
                    height: get_num(t, "h")?.map(|n| n as f32),
                });
            }
        }
        _ => {}
    }
    e.handlers.click = handler(t, "h_click")?;
    e.handlers.right_click = handler(t, "h_right_click")?;
    e.handlers.middle_click = handler(t, "h_middle_click")?;
    e.handlers.wheel = handler(t, "h_wheel")?;
    e.handlers.enter = handler(t, "h_enter")?;
    e.handlers.leave = handler(t, "h_leave")?;
    if let Lv::Table(slots) = t.raw_get::<Lv>("slots")? {
        let n = slots.raw_len();
        for i in 1..=n {
            let Lv::Table(slot) = slots.raw_get::<Lv>(i)? else { continue };
            if matches!(slot.raw_get::<Lv>("dyn")?, Lv::Boolean(true)) {
                if let Lv::Table(nodes) = slot.raw_get::<Lv>("nodes")? {
                    for j in 1..=nodes.raw_len() {
                        if let Lv::Table(child) = nodes.raw_get::<Lv>(j)? {
                            e.children.push(node(w, &child, depth + 1)?);
                        }
                    }
                }
            } else {
                e.children.push(node(w, &slot, depth + 1)?);
            }
        }
    }
    Ok(e)
}
