//! The colour pairs each preset lists in `[about.contrast]` cover the colours
//! its style sheet draws with: every custom property used as a text, icon,
//! border or outline colour is a foreground of some pair, and every one used
//! as a fill is a surface of some pair or listed as decorative. A colour added
//! to a sheet without a pair for it fails here, so contrast stays measured.

use std::collections::BTreeSet;

use rsb_app::assets;
use rsb_core::preset::{About, parse_about, split_pair_line, surface_layers};

const PRESETS: [&str; 4] = ["default", "fluent", "glass", "astur-classic"];

/// Properties whose colour is read against a fill: text and icons, and the rings
/// and outlines of `line` pairs. A plain `border` shorthand is decoration.
const FOREGROUND: [&str; 4] = ["color", "border-color", "outline", "outline-color"];
/// Properties that paint a surface.
const SURFACE: [&str; 3] = ["background", "background-color", "accent-color"];

struct Decl {
    /// The innermost selector is `:root`.
    on_root: bool,
    prop: String,
    value: String,
}

fn strip_comments(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn declarations(css: &str) -> Vec<Decl> {
    let css = strip_comments(css);
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut finish = |text: &mut String, stack: &[String]| {
        if let Some((prop, value)) = text.split_once(':') {
            let on_root = stack.last().is_some_and(|s| s == ":root");
            out.push(Decl { on_root, prop: prop.trim().to_string(), value: value.trim().to_string() });
        }
        text.clear();
    };
    let mut parens = 0;
    for c in css.chars() {
        match c {
            '(' => {
                parens += 1;
                current.push(c);
            }
            ')' => {
                parens -= 1;
                current.push(c);
            }
            '{' if parens == 0 => {
                stack.push(current.trim().to_string());
                current.clear();
            }
            '}' if parens == 0 => {
                finish(&mut current, &stack);
                stack.pop();
            }
            ';' if parens == 0 => finish(&mut current, &stack),
            _ => current.push(c),
        }
    }
    out
}

/// The `--name`s a value takes through `var()`, without the dashes.
fn vars_in(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(at) = rest.find("var(--") {
        rest = &rest[at + 6..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        out.push(name);
    }
    out
}

fn about(name: &str) -> About {
    parse_about(assets::preset_toml(name).unwrap(), &format!("{name}.toml")).unwrap_or_else(|d| panic!("{name}: {d}"))
}

#[test]
fn preset_contrast_table_covers_the_css() {
    for name in PRESETS {
        let a = about(name);
        let c = &a.contrast;
        let mut foregrounds = BTreeSet::new();
        let mut surfaces = BTreeSet::new();
        for line in c.text.iter().chain(&c.icon).chain(&c.line) {
            let (fg, on) = split_pair_line(line).unwrap();
            foregrounds.insert(fg.to_string());
            for s in on {
                surfaces.extend(surface_layers(s).unwrap().into_iter().map(String::from));
            }
        }
        let decorative: BTreeSet<String> = c.decorative.iter().cloned().collect();

        let css = [assets::base_css(), assets::preset_css(name).unwrap()].join("\n");
        let decls = declarations(&css);
        // A custom property set inside a rule that is not `:root` hands another one on
        // (`--hover: var(--seg1-hover)`): it is not a colour of its own, its source is.
        let aliases: BTreeSet<String> =
            decls.iter().filter(|d| !d.on_root && d.prop.starts_with("--")).map(|d| d.prop[2..].to_string()).collect();
        let resolve = |names: Vec<String>, decls: &[Decl]| -> Vec<String> {
            let mut out = Vec::new();
            for n in names {
                if aliases.contains(&n) {
                    for d in decls.iter().filter(|d| !d.on_root && d.prop == format!("--{n}")) {
                        out.extend(vars_in(&d.value));
                    }
                } else {
                    out.push(n);
                }
            }
            out
        };

        let (mut used_fg, mut used_surface) = (BTreeSet::new(), BTreeSet::new());
        for d in &decls {
            if d.prop.starts_with("--") {
                continue;
            }
            let names = resolve(vars_in(&d.value), &decls);
            if FOREGROUND.contains(&d.prop.as_str()) {
                used_fg.extend(names);
            } else if SURFACE.contains(&d.prop.as_str()) {
                used_surface.extend(names);
            }
        }
        // A colour the sheet does not set falls back (`var(--pill, transparent)`): nothing to measure.
        let defined: BTreeSet<String> =
            decls.iter().filter(|d| d.on_root && d.prop.starts_with("--")).map(|d| d.prop[2..].to_string()).collect();
        used_fg.retain(|n| defined.contains(n));
        used_surface.retain(|n| defined.contains(n));
        assert!(
            used_fg.len() >= 3 && used_surface.len() >= 2,
            "{name}: the scan found {used_fg:?} and {used_surface:?}"
        );

        let no_fg_pair: Vec<&String> =
            used_fg.iter().filter(|n| !foregrounds.contains(*n) && !decorative.contains(*n)).collect();
        assert!(no_fg_pair.is_empty(), "{name}: drawn as a foreground, no pair for it: {no_fg_pair:?}");
        let no_surface_pair: Vec<&String> = used_surface
            .iter()
            .filter(|n| !surfaces.contains(*n) && !foregrounds.contains(*n) && !decorative.contains(*n))
            .collect();
        assert!(no_surface_pair.is_empty(), "{name}: drawn as a fill, on no pair's surface: {no_surface_pair:?}");

        // And the other way: every name a pair uses is a colour the sheet sets (the table is not stale).
        for n in foregrounds.iter().chain(&surfaces).chain(&decorative) {
            assert!(defined.contains(n), "{name}: a pair names --{n}, which no :root rule sets");
        }
    }
}

#[test]
fn the_scan_reads_nested_rules_and_aliases() {
    let css = "/* c */ @media (x) { :root { --a: #fff; --b: var(--a); } }\n.i { background: var(--s); color: var(--f); --hover: var(--h); border-color: var(--hover) }";
    let d = declarations(css);
    assert_eq!(d.len(), 6);
    assert!(d[0].on_root && d[0].prop == "--a" && d[1].on_root);
    assert!(!d[2].on_root && d[2].prop == "background" && d[2].value == "var(--s)");
    assert_eq!(vars_in("1px solid var(--accent-line)"), ["accent-line"]);
    assert_eq!(vars_in("var(--a) var(--b, red)"), ["a", "b"]);
    assert!(vars_in("#fff").is_empty());
}

/// Windows high contrast is read and reported, never acted on: nothing that
/// builds or draws the bar looks at the flag, so the bar's pixels cannot depend on it.
#[test]
fn high_contrast_flag_never_reaches_drawing() {
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for dir in ["rsb-core/src", "rsb-render/src", "rsb-script/src"] {
        sources(&crates.join(dir), &mut files);
    }
    for file in ["app", "engine", "render_cmd", "winlayer", "scene", "timeline_run", "headless"] {
        let path = crates.join("rsb/src").join(format!("{file}.rs"));
        if path.exists() {
            files.push(path);
        }
    }
    assert!(files.len() > 40, "only {} source files scanned", files.len());
    let users: Vec<String> = files
        .iter()
        .filter(|f| std::fs::read_to_string(f).unwrap().contains("hicontrast"))
        .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(users.is_empty(), "these files look at the high contrast flag: {users:?}");
}

/// The error plate is drawn with literal colours, so no token pair covers it:
/// its text must pass on its fill here instead.
#[test]
fn the_error_plate_text_passes_on_its_fill() {
    let css = assets::base_css();
    let start = css
        .find(
            "
plate {",
        )
        .expect("base.css styles the plate")
        + 1;
    let block = &css[start..start + css[start..].find('}').expect("the rule ends")];
    let value = |prop: &str| -> rsb_core::color::Color {
        let line = block.lines().find_map(|l| l.trim().strip_prefix(prop)?.strip_prefix(':')).expect(prop);
        rsb_core::color::Color::parse(line.trim().trim_end_matches(';')).expect("a literal colour")
    };
    let ratio = rsb_core::color::Color::contrast_ratio(value("color"), value("background"));
    assert!(ratio >= 4.5, "plate text on its fill is {ratio:.2}");
}
