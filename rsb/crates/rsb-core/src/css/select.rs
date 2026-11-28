//! Selectors: parsing, specificity and matching.

use crate::element::ElementKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Combinator {
    Descendant,
    Child,
}

/// Elements a `::placeholder` or `::selection` rule styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pseudo {
    Placeholder,
    Selection,
}

/// `an+b` of `:nth-child()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Nth {
    pub a: i32,
    pub b: i32,
}

impl Nth {
    /// True when the 1-based position `pos` is `a*n + b` for some n >= 0.
    pub fn matches(&self, pos: usize) -> bool {
        let p = pos as i64;
        let (a, b) = (i64::from(self.a), i64::from(self.b));
        if a == 0 { p == b } else { (p - b) % a == 0 && (p - b) / a >= 0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Compound {
    pub kind: Option<ElementKind>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub hover: bool,
    pub active: bool,
    pub focus: bool,
    pub focus_visible: bool,
    pub focus_within: bool,
    pub disabled: bool,
    pub checked: bool,
    pub open: bool,
    pub first_child: bool,
    pub last_child: bool,
    pub nth: Vec<Nth>,
    pub pseudo: Option<Pseudo>,
}

impl Compound {
    fn has_focus(&self) -> bool {
        self.focus || self.focus_visible || self.focus_within
    }

    fn is_structural(&self) -> bool {
        self.first_child || self.last_child || !self.nth.is_empty()
    }

    fn pseudo_class_count(&self) -> u32 {
        [self.hover, self.active, self.focus, self.focus_visible, self.focus_within]
            .iter()
            .chain(&[self.disabled, self.checked, self.open, self.first_child, self.last_child])
            .filter(|b| **b)
            .count() as u32
            + self.nth.len() as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selector {
    /// Left to right; the combinator is the one that joins a compound to the
    /// compound before it (ignored for the first).
    pub parts: Vec<(Combinator, Compound)>,
}

/// What the cascade needs to know about a node and its ancestors.
pub(crate) trait MatchTree {
    fn kind(&self, node: usize) -> ElementKind;
    fn id(&self, node: usize) -> Option<&str>;
    fn has_class(&self, node: usize, class: &str) -> bool;
    fn hover(&self, node: usize) -> bool;
    fn active(&self, node: usize) -> bool;
    fn parent(&self, node: usize) -> Option<usize>;

    /// Keyboard focus is on the node (the surface's window has it).
    fn focus(&self, _node: usize) -> bool {
        false
    }
    /// Focused and the focus came from the keyboard.
    fn focus_visible(&self, _node: usize) -> bool {
        false
    }
    /// The node or a descendant is focused.
    fn focus_within(&self, _node: usize) -> bool {
        false
    }
    fn disabled(&self, _node: usize) -> bool {
        false
    }
    fn checked(&self, _node: usize) -> bool {
        false
    }
    /// A popup root while its surface is open, or the anchor of an open popup.
    fn open(&self, _node: usize) -> bool {
        false
    }
    /// 1-based position among the parent's siblings that take part in
    /// layout (those with `display: none` and empty widgets do not count);
    /// 0 for a node that does not take part itself.
    fn index_in_parent(&self, _node: usize) -> usize {
        1
    }
    /// How many siblings (itself included) take part in layout.
    fn sibling_count(&self, _node: usize) -> usize {
        1
    }
}

impl Selector {
    /// (ids, classes and pseudo-classes, types)
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut s = (0, 0, 0);
        for (_, c) in &self.parts {
            s.0 += u32::from(c.id.is_some());
            s.1 += c.classes.len() as u32 + c.pseudo_class_count();
            s.2 += u32::from(c.kind.is_some()) + u32::from(c.pseudo.is_some());
        }
        s
    }

    /// The type of the element the selector picks, when it names one.
    pub fn subject_kind(&self) -> Option<ElementKind> {
        self.parts.last().and_then(|(_, c)| c.kind)
    }

    pub fn matches(&self, tree: &dyn MatchTree, node: usize) -> bool {
        matches_from(tree, node, &self.parts, self.parts.len() - 1)
    }

    /// The pseudo-element the selector styles, when it ends in one.
    pub fn pseudo(&self) -> Option<Pseudo> {
        self.parts.last().and_then(|(_, c)| c.pseudo)
    }

    /// Whether the selector reads a node's position among its siblings.
    pub fn is_structural(&self) -> bool {
        self.parts.iter().any(|(_, c)| c.is_structural())
    }

    /// A focus pseudo-class that can only match inside a bar, where nothing
    /// takes keyboard focus: on a zone or plate, on a bar, widget wrapper
    /// (never focusable), or in a selector that has a zone or plate in it
    /// (popups are never inside either).
    pub fn focus_only_in_bar(&self) -> bool {
        let focus = self.parts.iter().any(|(_, c)| c.has_focus());
        let bar_only_kind = |k: Option<ElementKind>| matches!(k, Some(ElementKind::Zone | ElementKind::Plate));
        let unfocusable = |k: Option<ElementKind>| {
            matches!(k, Some(ElementKind::Bar | ElementKind::Zone | ElementKind::Widget | ElementKind::Plate))
        };
        focus
            && (self.parts.iter().any(|(_, c)| bar_only_kind(c.kind))
                || self.parts.iter().any(|(_, c)| c.has_focus() && unfocusable(c.kind)))
    }
}

fn compound_matches(tree: &dyn MatchTree, node: usize, c: &Compound) -> bool {
    c.kind.is_none_or(|k| tree.kind(node) == k)
        && c.id.as_deref().is_none_or(|id| tree.id(node) == Some(id))
        && c.classes.iter().all(|cl| tree.has_class(node, cl))
        && (!c.hover || tree.hover(node))
        && (!c.active || tree.active(node))
        && (!c.focus || tree.focus(node))
        && (!c.focus_visible || tree.focus_visible(node))
        && (!c.focus_within || tree.focus_within(node))
        && (!c.disabled || tree.disabled(node))
        && (!c.checked || tree.checked(node))
        && (!c.open || tree.open(node))
        && structural_matches(tree, node, c)
}

fn structural_matches(tree: &dyn MatchTree, node: usize, c: &Compound) -> bool {
    if !c.is_structural() {
        return true;
    }
    let pos = tree.index_in_parent(node);
    pos != 0
        && (!c.first_child || pos == 1)
        && (!c.last_child || pos == tree.sibling_count(node))
        && c.nth.iter().all(|n| n.matches(pos))
}

fn matches_from(tree: &dyn MatchTree, node: usize, parts: &[(Combinator, Compound)], at: usize) -> bool {
    if !compound_matches(tree, node, &parts[at].1) {
        return false;
    }
    if at == 0 {
        return true;
    }
    match parts[at].0 {
        Combinator::Child => tree.parent(node).is_some_and(|p| matches_from(tree, p, parts, at - 1)),
        Combinator::Descendant => {
            let mut cur = tree.parent(node);
            while let Some(p) = cur {
                if matches_from(tree, p, parts, at - 1) {
                    return true;
                }
                cur = tree.parent(p);
            }
            false
        }
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

/// Type selector names in the order the docs list them.
pub(crate) const TYPE_NAMES: &[(&str, ElementKind)] = &[
    ("bar", ElementKind::Bar),
    ("zone", ElementKind::Zone),
    ("widget", ElementKind::Widget),
    ("box", ElementKind::Box),
    ("text", ElementKind::Text),
    ("icon", ElementKind::Icon),
    ("image", ElementKind::Image),
    ("plate", ElementKind::Plate),
    ("popup", ElementKind::Popup),
    ("menu", ElementKind::Menu),
    ("menuitem", ElementKind::MenuItem),
    ("tooltip", ElementKind::Tooltip),
    ("input", ElementKind::Input),
    ("slider", ElementKind::Slider),
    ("scroll", ElementKind::Scroll),
];

/// What the `:` error lists.
const PSEUDO_CLASSES: &str = ":hover, :active, :focus, :focus-visible, :focus-within, :disabled, :checked, :open, :first-child, :last-child, :nth-child()";

/// A selector problem: the byte offset in the selector text and the message.
pub(crate) type SelErr = (usize, String);

/// Parses one complex selector (no commas). Offsets in errors are relative
/// to `text`.
pub(crate) fn parse_selector(text: &str) -> Result<Selector, SelErr> {
    let b = text.as_bytes();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
        *i > start
    };
    skip_ws(&mut i);
    let mut parts: Vec<(Combinator, Compound)> = Vec::new();
    let mut comb = Combinator::Descendant;
    loop {
        let mut c = Compound::default();
        let mut any = false;
        let start = i;
        // Type, `*` or `:root` first.
        if b.get(i) == Some(&b'*') {
            i += 1;
            any = true;
        } else if b.get(i).is_some_and(|&x| is_ident_byte(x) && !x.is_ascii_digit()) {
            let s = i;
            while i < b.len() && is_ident_byte(b[i]) {
                i += 1;
            }
            let name = &text[s..i];
            match TYPE_NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                Some((_, k)) => c.kind = Some(*k),
                None => {
                    let names = TYPE_NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
                    return Err((
                        s,
                        format!("unknown element type '{name}'; types are {names}; use .{name} for a class"),
                    ));
                }
            }
            any = true;
        }
        loop {
            if c.pseudo.is_some() && matches!(b.get(i), Some(b'.' | b'#')) {
                return Err((i, "a pseudo-element must be last in the selector".into()));
            }
            match b.get(i) {
                Some(b'.') | Some(b'#') => {
                    let is_class = b[i] == b'.';
                    i += 1;
                    let s = i;
                    while i < b.len() && is_ident_byte(b[i]) {
                        i += 1;
                    }
                    if s == i {
                        return Err((s, format!("expected a name after '{}'", if is_class { '.' } else { '#' })));
                    }
                    let name = text[s..i].to_string();
                    if is_class {
                        c.classes.push(name);
                    } else if c.id.replace(name).is_some() {
                        return Err((s, "a selector can name one id".into()));
                    }
                    any = true;
                }
                Some(b':') => {
                    if c.pseudo.is_some() {
                        return Err((i, "a pseudo-element must be last in the selector".into()));
                    }
                    if b.get(i + 1) == Some(&b':') {
                        i += 2;
                        let s = i;
                        while i < b.len() && is_ident_byte(b[i]) {
                            i += 1;
                        }
                        let name = text[s..i].to_ascii_lowercase();
                        c.pseudo = Some(match name.as_str() {
                            "placeholder" => Pseudo::Placeholder,
                            "selection" => Pseudo::Selection,
                            _ => {
                                return Err((
                                    s - 2,
                                    format!(
                                        "::{name}: pseudo-elements are not supported; supported: ::placeholder, ::selection"
                                    ),
                                ));
                            }
                        });
                        if !matches!(c.kind, None | Some(ElementKind::Input)) {
                            return Err((s - 2, format!("::{name} only works on input")));
                        }
                        any = true;
                        continue;
                    }
                    i += 1;
                    let s = i;
                    while i < b.len() && is_ident_byte(b[i]) {
                        i += 1;
                    }
                    let name = text[s..i].to_ascii_lowercase();
                    let paren = b.get(i) == Some(&b'(');
                    match name.as_str() {
                        "hover" => c.hover = true,
                        "active" => c.active = true,
                        "focus" => c.focus = true,
                        "focus-visible" => c.focus_visible = true,
                        "focus-within" => c.focus_within = true,
                        "disabled" => c.disabled = true,
                        "checked" => c.checked = true,
                        "open" => c.open = true,
                        "first-child" => c.first_child = true,
                        "last-child" => c.last_child = true,
                        "nth-child" if paren => {
                            let close = text[i..].find(')').map(|o| i + o).ok_or((i, "missing ')'".to_string()))?;
                            let nth = parse_nth(&text[i + 1..close]).map_err(|m| (i + 1, m))?;
                            c.nth.push(nth);
                            i = close + 1;
                        }
                        "root" => {
                            if c.kind.is_some() {
                                return Err((s, ":root cannot follow another type".into()));
                            }
                            c.kind = Some(ElementKind::Bar);
                        }
                        _ => {
                            return Err((s - 1, format!(":{name} is not supported; supported: {PSEUDO_CLASSES}")));
                        }
                    }
                    if name != "nth-child" && b.get(i) == Some(&b'(') {
                        return Err((i, format!(":{name}() is not supported")));
                    }
                    any = true;
                }
                Some(b'[') => return Err((i, "attribute selectors are not supported; use a class".into())),
                _ => break,
            }
        }
        if !any {
            let at = if i < b.len() { i } else { start };
            return Err((
                at,
                match b.get(i) {
                    Some(&ch) => format!("expected a selector, found '{}'", ch as char),
                    None => "expected a selector".into(),
                },
            ));
        }
        if c.pseudo.is_some() && i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
            return Err((i, "a pseudo-element must be last in the selector".into()));
        }
        parts.push((comb, c));
        let ws = skip_ws(&mut i);
        match b.get(i) {
            None => break,
            Some(b'>') => {
                i += 1;
                skip_ws(&mut i);
                if i >= b.len() {
                    return Err((i, "expected a selector after '>'".into()));
                }
                comb = Combinator::Child;
            }
            Some(b'+') | Some(b'~') => {
                return Err((i, format!("the '{}' combinator is not supported; use ' ' or '>'", b[i] as char)));
            }
            Some(_) if ws => comb = Combinator::Descendant,
            Some(&ch) => return Err((i, format!("unexpected '{}' in selector", ch as char))),
        }
    }
    if parts.iter().rev().skip(1).any(|(_, c)| c.pseudo.is_some()) {
        return Err((0, "a pseudo-element must be last in the selector".into()));
    }
    Ok(Selector { parts })
}

/// The inside of `:nth-child(...)`: `odd`, `even`, `3`, `n`, `2n`, `-n+3`, `2n + 1`.
fn parse_nth(inner: &str) -> Result<Nth, String> {
    const EXPECTED: &str = "expected odd, even, a number or an+b like 2n+1 inside :nth-child()";
    let t: String = inner.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    match t.as_str() {
        "odd" => return Ok(Nth { a: 2, b: 1 }),
        "even" => return Ok(Nth { a: 2, b: 0 }),
        "" => return Err(EXPECTED.into()),
        _ => {}
    }
    let int = |x: &str| -> Result<i32, String> {
        if x.is_empty() || x.len() > 6 || !x.bytes().all(|c| c.is_ascii_digit()) {
            return Err(EXPECTED.to_string());
        }
        x.parse::<i32>().map_err(|_| EXPECTED.to_string())
    };
    let Some(npos) = t.find('n') else {
        let v = t.strip_prefix('+').unwrap_or(&t);
        return match v.strip_prefix('-') {
            Some(neg) => Ok(Nth { a: 0, b: -int(neg)? }),
            None => Ok(Nth { a: 0, b: int(v)? }),
        };
    };
    let (acoef, rest) = t.split_at(npos);
    let rest = &rest[1..];
    let a = match acoef {
        "" | "+" => 1,
        "-" => -1,
        x => match x.strip_prefix('-') {
            Some(neg) => -int(neg)?,
            None => int(x.strip_prefix('+').unwrap_or(x))?,
        },
    };
    let b = match rest {
        "" => 0,
        r => match (r.strip_prefix('+'), r.strip_prefix('-')) {
            (Some(p), _) => int(p)?,
            (_, Some(m)) => -int(m)?,
            _ => return Err(EXPECTED.into()),
        },
    };
    Ok(Nth { a, b })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specificity_counts() {
        let s = parse_selector("bar.main zone#left .a:hover").unwrap();
        assert_eq!(s.specificity(), (1, 3, 2));
        assert_eq!(parse_selector(":root").unwrap().specificity(), (0, 0, 1));
        assert_eq!(parse_selector("*").unwrap().specificity(), (0, 0, 0));
    }

    #[test]
    fn refusals_name_the_alternative() {
        for (text, want) in [
            (".a:visited", "supported: :hover, :active, :focus"),
            ("[x]", "attribute"),
            (".a + .b", "'+' combinator"),
            ("foo", "unknown element type 'foo'"),
            ("bar >", "after '>'"),
            (".a::before", "pseudo-elements"),
            ("", "expected a selector"),
        ] {
            let e = parse_selector(text).unwrap_err();
            assert!(e.1.contains(want), "{text:?}: {}", e.1);
        }
    }
}
