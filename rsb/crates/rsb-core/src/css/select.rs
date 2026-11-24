//! Selectors: parsing, specificity and matching.

use crate::element::ElementKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Combinator {
    Descendant,
    Child,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Compound {
    pub kind: Option<ElementKind>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub hover: bool,
    pub active: bool,
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
}

impl Selector {
    /// (ids, classes and pseudo-classes, types)
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut s = (0, 0, 0);
        for (_, c) in &self.parts {
            s.0 += u32::from(c.id.is_some());
            s.1 += c.classes.len() as u32 + u32::from(c.hover) + u32::from(c.active);
            s.2 += u32::from(c.kind.is_some());
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
}

fn compound_matches(tree: &dyn MatchTree, node: usize, c: &Compound) -> bool {
    c.kind.is_none_or(|k| tree.kind(node) == k)
        && c.id.as_deref().is_none_or(|id| tree.id(node) == Some(id))
        && c.classes.iter().all(|cl| tree.has_class(node, cl))
        && (!c.hover || tree.hover(node))
        && (!c.active || tree.active(node))
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
];

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
                    if b.get(i + 1) == Some(&b':') {
                        return Err((i, "pseudo-elements are not supported".into()));
                    }
                    i += 1;
                    let s = i;
                    while i < b.len() && is_ident_byte(b[i]) {
                        i += 1;
                    }
                    let name = text[s..i].to_ascii_lowercase();
                    match name.as_str() {
                        "hover" => c.hover = true,
                        "active" => c.active = true,
                        "root" => {
                            if c.kind.is_some() {
                                return Err((s, ":root cannot follow another type".into()));
                            }
                            c.kind = Some(ElementKind::Bar);
                        }
                        _ => {
                            return Err((s - 1, format!(":{name} is not supported; supported: :hover, :active")));
                        }
                    }
                    if b.get(i) == Some(&b'(') {
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
    Ok(Selector { parts })
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
            (".a:focus", "supported: :hover, :active"),
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
