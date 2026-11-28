//! Reads an instance's live node tree (Luau tables the prelude keeps) into
//! the plain `Element` tree the scene consumes. Caps on node count and depth
//! stop the instance instead of letting a huge tree reach layout.

use std::sync::Arc;

use mlua::{Table, Value as Lv};
use rsb_core::diag::SourcePos;
use rsb_core::element::{
    AnimateSpec, Element, ElementKind, HandlerId, IconRef, ImageRef, InputProps, InstanceId, NavAxis, SliderProps,
};
use rsb_core::pixels::IMAGE_KEY_PREFIX;
use rsb_core::popup::{
    DEFAULT_GROUP, DismissRules, FocusMode, MENU_GROUP, Placement, PopupAlign, PopupProps, TOOLTIP_GROUP,
    TOOLTIP_OFFSET,
};
use rsb_core::services::ServiceId;
use rsb_core::style::FlexDirection;

use crate::shared::{Shared, Stop};

struct Walk<'a> {
    sh: &'a Shared,
    inst: InstanceId,
    count: usize,
    max_nodes: usize,
    max_depth: usize,
    max_text: usize,
    /// A built-in widget, or one holding `overlay`: its popups may turn the dismiss rules off.
    trusted: bool,
}

pub(crate) fn snapshot(sh: &Shared, inst: InstanceId, root: &Table, error_class: bool) -> mlua::Result<Element> {
    let lim = &sh.env.limits;
    let trusted = sh.st.borrow().instances.get(&inst).is_some_and(|i| i.builtin || i.perms.overlay);
    let mut w = Walk {
        sh,
        inst,
        count: 0,
        max_nodes: lim.max_nodes_per_instance,
        max_depth: lim.max_element_depth,
        max_text: lim.max_text_chars,
        trusted,
    };
    let mut e = node(&mut w, root, 1)?;
    if error_class {
        e.classes.push(Arc::from("error"));
        // A popup whose content failed would be an empty stub: say what broke inside it.
        let why = sh.st.borrow().instances.get(&inst).and_then(|i| i.last_error.clone());
        if let Some(why) = why {
            plate_in_empty_popups(&mut e, &format!("WIDGET ERROR - {why} (see log)"));
        }
    }
    Ok(e)
}

fn plate_in_empty_popups(e: &mut Element, text: &str) {
    if e.kind == ElementKind::Popup && e.children.is_empty() {
        let mut plate = Element::new(ElementKind::Plate);
        plate.text = Some(Arc::from(text));
        e.children.push(plate);
        return;
    }
    for c in &mut e.children {
        plate_in_empty_popups(c, text);
    }
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

fn get_bool(t: &Table, key: &str) -> mlua::Result<Option<bool>> {
    Ok(match t.raw_get::<Lv>(key)? {
        Lv::Boolean(b) => Some(b),
        _ => None,
    })
}

/// A bundled icon name is lower-case words; anything else is a path to an `.svg` file.
fn is_bundled_name(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn popup_props(t: &Table, kind: ElementKind, trusted: bool) -> mlua::Result<PopupProps> {
    let group = match kind {
        ElementKind::Tooltip => TOOLTIP_GROUP,
        ElementKind::Menu => MENU_GROUP,
        _ => DEFAULT_GROUP,
    };
    let mut p = PopupProps { group: Arc::from(group), ..PopupProps::default() };
    if kind == ElementKind::Tooltip {
        p.offset = TOOLTIP_OFFSET;
        p.focus = FocusMode::None;
        p.dismiss = DismissRules { outside: false, escape: false };
    }
    let Lv::Table(pop) = t.raw_get::<Lv>("pop")? else { return Ok(p) };
    if let Lv::Table(names) = pop.raw_get::<Lv>("names")? {
        for n in names.sequence_values::<String>() {
            p.names.push(Arc::from(n?.as_str()));
        }
    }
    if let Some(g) = get_str(&pop, "group")? {
        p.group = Arc::from(g.as_str());
    }
    p.placement = match get_str(&pop, "placement")?.as_deref() {
        Some("below") => Placement::Below,
        Some("above") => Placement::Above,
        Some("left") => Placement::Left,
        Some("right") => Placement::Right,
        Some("screen") => Placement::Screen,
        Some("pointer") => Placement::Pointer {
            x: get_num(t, "px")?.unwrap_or(0.0) as f32,
            y: get_num(t, "py")?.unwrap_or(0.0) as f32,
        },
        _ => Placement::Auto,
    };
    p.align = match get_str(&pop, "align")?.as_deref() {
        Some("start") => PopupAlign::Start,
        Some("end") => PopupAlign::End,
        _ => PopupAlign::Center,
    };
    if let Some(o) = get_num(&pop, "offset")? {
        p.offset = o as f32;
    }
    p.attach = get_bool(&pop, "attach")?.unwrap_or(false);
    p.focus = match get_str(&pop, "focus")?.as_deref() {
        Some("none") => FocusMode::None,
        Some("always") => FocusMode::Always,
        _ => p.focus,
    };
    p.dismiss = DismissRules {
        outside: get_bool(&pop, "dismiss_outside")?.unwrap_or(p.dismiss.outside),
        escape: get_bool(&pop, "dismiss_escape")?.unwrap_or(p.dismiss.escape),
    };
    p.open = get_bool(t, "open")?.unwrap_or(false);
    p.open_seq = get_num(t, "oseq")?.unwrap_or(0.0) as u64;
    p.gesture = get_num(t, "ogest")?.unwrap_or(0.0) as u64;
    p.trusted = trusted;
    if !trusted && p.gesture == 0 && kind == ElementKind::Popup {
        // A popup nobody asked for (a timer, a service update) can always be dismissed.
        p.dismiss = DismissRules::default();
    }
    p.on_close = handler(t, "h_close")?;
    p.on_open = handler(t, "h_open")?;
    Ok(p)
}

/// A text element is one line: what comes before the first line break (command output ends in one).
fn one_line(text: &str) -> &str {
    text.split(['\n', '\r']).next().unwrap_or("")
}

fn cut(text: &str, max: usize) -> Arc<str> {
    if text.chars().count() <= max { Arc::from(text) } else { Arc::from(text.chars().take(max).collect::<String>()) }
}

/// The element's kind and what every kind has: key, id, classes, inline style, where it was written, direction.
#[inline(never)]
fn base(t: &Table) -> mlua::Result<(ElementKind, Element)> {
    let kind = match get_str(t, "kind")?.as_deref() {
        Some("text") => ElementKind::Text,
        Some("icon") => ElementKind::Icon,
        Some("image") => ElementKind::Image,
        Some("popup") => ElementKind::Popup,
        Some("menu") => ElementKind::Menu,
        Some("menuitem") => ElementKind::MenuItem,
        Some("tooltip") => ElementKind::Tooltip,
        Some("input") => ElementKind::Input,
        Some("slider") => ElementKind::Slider,
        Some("scroll") => ElementKind::Scroll,
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
    Ok((kind, e))
}

/// What only some kinds have: text, icons, images, popup props, input and slider values.
#[inline(never)]
fn kind_fields(w: &Walk, t: &Table, kind: ElementKind, e: &mut Element) -> mlua::Result<()> {
    match kind {
        ElementKind::Text => {
            if let Some(text) = get_str(t, "text")? {
                e.text = Some(cut(one_line(&text), w.max_text));
            } else {
                e.text = Some(Arc::from(""));
            }
        }
        ElementKind::Icon | ElementKind::MenuItem => {
            if kind == ElementKind::MenuItem {
                e.text = Some(cut(one_line(&get_str(t, "text")?.unwrap_or_default()), w.max_text));
            }
            if let Some(svg) = get_str(t, "isvg")?.filter(|s| !s.is_empty()) {
                if is_bundled_name(&svg) {
                    e.icon = Some(IconRef::Svg(Arc::from(svg.as_str())));
                } else {
                    let path = w.sh.permitted_path(w.inst, &svg)?;
                    e.icon = Some(IconRef::Svg(Arc::from(path.to_string_lossy().as_ref())));
                }
            } else if let Some(name) = get_str(t, "iname")?.filter(|s| !s.is_empty()) {
                e.icon = Some(IconRef::Named(Arc::from(name.as_str())));
            } else if let Some(glyph) = get_str(t, "iglyph")? {
                e.icon = Some(IconRef::Glyph(cut(&glyph, w.max_text)));
            }
        }
        ElementKind::Image => {
            if let Some(src) = get_str(t, "src")? {
                let path: Arc<str> = if let Some(rest) = src.strip_prefix(IMAGE_KEY_PREFIX) {
                    // A key a service published: the widget needs to be allowed to read that service.
                    let service = rest.split('/').next().unwrap_or("");
                    let svc = ServiceId::from_name(service)
                        .ok_or_else(|| mlua::Error::runtime("rsb.image: the image key names no service"))?;
                    w.sh.require_read(w.inst, svc)?;
                    Arc::from(src.as_str())
                } else {
                    let path = w.sh.permitted_path(w.inst, &src)?;
                    Arc::from(path.to_string_lossy().as_ref())
                };
                e.image = Some(ImageRef {
                    path,
                    width: get_num(t, "w")?.map(|n| n as f32),
                    height: get_num(t, "h")?.map(|n| n as f32),
                });
            }
        }
        ElementKind::Popup | ElementKind::Menu | ElementKind::Tooltip => {
            e.popup = Some(Box::new(popup_props(t, kind, w.trusted)?));
        }
        ElementKind::Input => {
            let mut i = InputProps {
                value: cut(&get_str(t, "ival")?.unwrap_or_default(), w.max_text),
                value_seq: get_num(t, "iseq")?.unwrap_or(0.0) as u64,
                password: get_bool(t, "pw")?.unwrap_or(false),
                max_length: get_num(t, "maxlen")?.unwrap_or(0.0) as u32,
                on_change: handler(t, "h_change")?,
                on_submit: handler(t, "h_submit")?,
                ..InputProps::default()
            };
            i.placeholder = get_str(t, "ph")?.filter(|s| !s.is_empty()).map(|s| cut(&s, w.max_text));
            e.input = Some(Box::new(i));
        }
        ElementKind::Slider => {
            let d = SliderProps::default();
            e.slider = Some(Box::new(SliderProps {
                value: get_num(t, "sval")?.unwrap_or(d.min),
                min: get_num(t, "smin")?.unwrap_or(d.min),
                max: get_num(t, "smax")?.unwrap_or(d.max),
                step: get_num(t, "sstep")?.unwrap_or(d.step),
                vertical: get_bool(t, "svert")?.unwrap_or(false),
                jump: get_bool(t, "sjump")?.unwrap_or(true),
                value_seq: get_num(t, "sseq")?.unwrap_or(0.0) as u64,
                on_change: handler(t, "h_change")?,
            }));
        }
        _ => {}
    }
    Ok(())
}

/// Handlers, focus and interaction props, and the animation.
#[inline(never)]
fn interaction_fields(t: &Table, kind: ElementKind, e: &mut Element) -> mlua::Result<()> {
    e.handlers.click = handler(t, "h_click")?;
    e.handlers.right_click = handler(t, "h_right_click")?;
    e.handlers.middle_click = handler(t, "h_middle_click")?;
    e.handlers.wheel = handler(t, "h_wheel")?;
    e.handlers.enter = handler(t, "h_enter")?;
    e.handlers.leave = handler(t, "h_leave")?;
    e.handlers.key = handler(t, "h_key")?;
    e.handlers.scroll = handler(t, "h_scroll")?;
    e.handlers.focus = handler(t, "h_focus")?;
    e.handlers.blur = handler(t, "h_blur")?;
    e.interaction.focusable = get_bool(t, "focusable")?.unwrap_or(false);
    e.interaction.autofocus = get_bool(t, "autofocus")?.unwrap_or(false);
    e.interaction.disabled = get_bool(t, "dis")?.unwrap_or(false);
    e.interaction.checked = get_bool(t, "chk")?;
    e.interaction.reveal_seq = get_num(t, "rseq")?.unwrap_or(0.0) as u64;
    e.interaction.nav = match get_str(t, "nav")?.as_deref() {
        Some("vertical") => NavAxis::Vertical,
        Some("horizontal") => NavAxis::Horizontal,
        Some("grid") => NavAxis::Grid,
        _ if kind == ElementKind::Menu => NavAxis::Vertical,
        _ => NavAxis::None,
    };
    if let Lv::Table(a) = t.raw_get::<Lv>("anim")? {
        let opt =
            |key: &str| -> mlua::Result<Option<Arc<str>>> { Ok(get_str(&a, key)?.map(|v| Arc::from(v.as_str()))) };
        e.animate = Some(Box::new(AnimateSpec {
            name: Arc::from(get_str(&a, "name")?.unwrap_or_default().as_str()),
            duration: opt("duration")?,
            easing: opt("easing")?,
            delay: opt("delay")?,
            iterations: opt("iterations")?,
            direction: opt("direction")?,
            fill: opt("fill")?,
            restart_seq: get_num(t, "aseq")?.unwrap_or(0.0) as u64,
        }));
    }
    Ok(())
}

fn node(w: &mut Walk, t: &Table, depth: usize) -> mlua::Result<Element> {
    w.count += 1;
    if w.count > w.max_nodes {
        return Err(w.sh.kill(Stop::Nodes(w.max_nodes)));
    }
    if depth > w.max_depth {
        return Err(w.sh.kill(Stop::Depth(w.max_depth)));
    }
    let (kind, mut e) = base(t)?;
    kind_fields(w, t, kind, &mut e)?;
    interaction_fields(t, kind, &mut e)?;
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
    // A tooltip and a menu opened by the `menu` prop sit after the element's own children.
    if let Lv::Table(aux) = t.raw_get::<Lv>("aux")? {
        for j in 1..=aux.raw_len() {
            if let Lv::Table(child) = aux.raw_get::<Lv>(j)? {
                e.children.push(node(w, &child, depth + 1)?);
            }
        }
    }
    Ok(e)
}
