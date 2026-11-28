//! The `[about]` table of each shipped preset: it reads, every name in it is
//! defined by the preset's own sheet in each scheme it lists, the measured
//! contrast of the shipped colours holds, and any accent a person sets keeps
//! every pair readable.

use rsb_core::color::Color;
use rsb_core::contrast::{self, AccentSurfaces, Layer, PairKind, Vars};
use rsb_core::css::Stylesheet;
use rsb_core::preset::{About, Scheme, parse_about};

const BASE: &str = include_str!("../../../assets/themes/base.css");

const PRESETS: [(&str, &str, &str); 4] = [
    (
        "default",
        include_str!("../../../assets/presets/default.toml"),
        include_str!("../../../assets/presets/default.css"),
    ),
    ("fluent", include_str!("../../../assets/presets/fluent.toml"), include_str!("../../../assets/presets/fluent.css")),
    ("glass", include_str!("../../../assets/presets/glass.toml"), include_str!("../../../assets/presets/glass.css")),
    (
        "astur-classic",
        include_str!("../../../assets/presets/astur-classic.toml"),
        include_str!("../../../assets/presets/astur-classic.css"),
    ),
];

fn chain(preset_css: &str, user_css: &str) -> Stylesheet {
    let mut sheet = Stylesheet::empty();
    sheet.append(Stylesheet::parse(BASE, "base.css").0);
    sheet.append(Stylesheet::parse(preset_css, "preset.css").0);
    if !user_css.is_empty() {
        sheet.append(Stylesheet::parse(user_css, "style.css").0);
    }
    sheet
}

fn about(name: &str, toml: &str) -> About {
    parse_about(toml, &format!("presets/{name}.toml")).unwrap_or_else(|d| panic!("{name}: {d}"))
}

/// The colours block `rsb settings` writes for a derived accent (both schemes).
fn colours_block(set: &rsb_core::contract::AccentSet) -> String {
    let one = |t: &rsb_core::contract::AccentTokens| {
        format!(
            ":root {{ --accent: {}; --accent-fg: {}; --accent-hover: {}; --accent-press: {}; --accent-text: {}; --accent-line: {}; }}",
            set.value, t.fg, t.hover, t.press, t.text, t.line
        )
    };
    format!(
        "@media (prefers-color-scheme: dark) {{\n{}\n}}\n@media (prefers-color-scheme: light) {{\n{}\n}}\n",
        one(&set.schemes.dark),
        one(&set.schemes.light)
    )
}

fn surfaces_of_all_presets() -> AccentSurfaces {
    let mut surfaces = AccentSurfaces::default();
    for (name, toml, css) in PRESETS {
        let a = about(name, toml);
        let sheet = chain(css, "");
        for scheme in Scheme::ALL {
            contrast::collect_accent_surfaces(&a, &contrast::resolve_vars(&sheet, scheme), scheme, &mut surfaces);
        }
    }
    surfaces
}

#[test]
fn preset_about_parses() {
    let mut titles = Vec::new();
    for (name, toml, css) in PRESETS {
        let a = about(name, toml);
        assert!(!titles.contains(&a.title), "{name}: the title {:?} is used twice", a.title);
        titles.push(a.title.clone());
        assert_eq!(a.schemes, vec![Scheme::Dark, Scheme::Light], "{name}");
        assert_eq!(a.tune[0].var, "accent", "{name}: the accent comes first");
        let want: &[&str] =
            if name == "default" { &["accent", "fg", "seg1", "panel"] } else { &["accent", "fg", "bg"] };
        assert_eq!(a.tune.iter().map(|t| t.var.as_str()).collect::<Vec<_>>(), want, "{name}");
        let sheet = chain(css, "");
        for scheme in &a.schemes {
            let vars = contrast::resolve_vars(&sheet, *scheme);
            let missing = contrast::undefined_names(&a, &vars);
            assert!(missing.is_empty(), "{name} {}: not defined: {missing:?}", scheme.name());
        }
        let pairs = a.contrast.text.len() + a.contrast.icon.len() + a.contrast.line.len();
        assert!(pairs >= 4, "{name}: only {pairs} pair lines");
    }
    assert_eq!(titles.len(), 4);
}

#[test]
fn every_preset_defines_the_same_accent_tokens() {
    for (name, _, css) in PRESETS {
        let sheet = chain(css, "");
        for scheme in Scheme::ALL {
            let vars = contrast::resolve_vars(&sheet, scheme);
            for token in ["accent", "accent-fg", "accent-hover", "accent-press", "accent-text", "accent-line"] {
                assert!(vars.contains_key(token), "{name} {}: --{token} is not defined", scheme.name());
            }
        }
    }
}

#[test]
fn an_undefined_name_fails_visibly_instead_of_vanishing() {
    let (name, toml, css) = PRESETS[1];
    let a = about(name, toml);
    let mut vars = contrast::resolve_vars(&chain(css, ""), Scheme::Dark);
    let all = contrast::pairs(&a, &vars);
    assert!(all.iter().all(|p| p.ratio > 0.0));
    vars.remove("muted");
    let broken = contrast::pairs(&a, &vars);
    assert_eq!(broken.len(), all.len(), "a missing colour does not drop pairs");
    assert!(broken.iter().filter(|p| p.fg == "muted").all(|p| p.ratio == 0.0 && !p.passes()));
    assert!(!contrast::summary(&broken).pass);
    assert_eq!(contrast::undefined_names(&a, &vars), vec!["muted".to_string()]);
}

/// The measured table of the shipped colours: lowest text and lowest icon or
/// line ratio of each preset and scheme, two places.
const MEASURED: [(&str, &str, f64, f64); 8] = [
    ("default", "dark", 4.67, 3.02),
    ("default", "light", 4.59, 3.46),
    ("fluent", "dark", 5.06, 4.32),
    ("fluent", "light", 5.35, 3.19),
    ("glass", "dark", 4.74, 3.57),
    ("glass", "light", 4.68, 3.93),
    ("astur-classic", "dark", 5.02, 7.13),
    ("astur-classic", "light", 5.09, 5.57),
];

#[test]
fn preset_token_contrast() {
    for (preset, scheme_name, text, graphic) in MEASURED {
        let (_, toml, css) = PRESETS.iter().find(|(n, _, _)| *n == preset).unwrap();
        let scheme = Scheme::from_name(scheme_name).unwrap();
        let a = about(preset, toml);
        let vars = contrast::resolve_vars(&chain(css, ""), scheme);
        let results = contrast::pairs(&a, &vars);
        let under: Vec<String> =
            results.iter().filter(|p| !p.passes()).map(|p| format!("{} {:.2}", p.what(), p.ratio)).collect();
        assert!(under.is_empty(), "{preset} {scheme_name}: under the need: {under:?}");
        let s = contrast::summary(&results);
        assert!(s.pass, "{preset} {scheme_name}");
        assert!(s.text.lowest >= 4.59, "{preset} {scheme_name}: lowest text {} ({})", s.text.lowest, s.text.what);
        assert!(
            (s.text.lowest - text).abs() <= 0.011,
            "{preset} {scheme_name}: text {} ({}), measured {text}",
            s.text.lowest,
            s.text.what
        );
        assert!(
            (s.graphic.lowest - graphic).abs() <= 0.011,
            "{preset} {scheme_name}: graphic {} ({}), measured {graphic}",
            s.graphic.lowest,
            s.graphic.what
        );
        assert_eq!((s.text.need, s.graphic.need), (4.5, 3.0));
    }
}

#[test]
fn the_lowest_pairs_are_named() {
    let (name, toml, css) = PRESETS[0];
    let a = about(name, toml);
    let dark = contrast::summary(&contrast::pairs(&a, &contrast::resolve_vars(&chain(css, ""), Scheme::Dark)));
    assert_eq!((dark.text.what.as_str(), dark.graphic.what.as_str()), ("placeholder on input", "dim on row"));
    let light = contrast::summary(&contrast::pairs(&a, &contrast::resolve_vars(&chain(css, ""), Scheme::Light)));
    assert_eq!(light.text.what, "mem on seg1-hover");
}

#[test]
fn a_pair_kind_sets_the_need_and_a_translucent_surface_takes_the_worse_base() {
    assert_eq!((PairKind::Text.need(), PairKind::Icon.need(), PairKind::Line.need()), (4.5, 3.0, 3.0));
    // Half-white over black is grey, over white it is white: the worse of the two is measured.
    let sheet = chain(":root { --fg: #ffffff; --sheer: rgba(255, 255, 255, 0.5); --solid: #000000; }", "");
    let a = about(
        "t",
        "[about]\ntitle = \"T\"\ndescription = \"d\"\nschemes = [\"dark\"]\ntune = [{ var = \"fg\", label = \"Text\" }, { var = \"accent\", label = \"A\" }]\n[about.contrast]\ntext = [\"fg on solid sheer\"]\n",
    );
    let vars = contrast::resolve_vars(&sheet, Scheme::Dark);
    let r = contrast::pairs(&a, &vars);
    assert_eq!(r.len(), 2);
    assert!((r[0].ratio - 21.0).abs() < 0.01, "white on black: {}", r[0].ratio);
    // White text on a half-white layer: over white it is white on white, ratio 1.
    assert!((r[1].ratio - 1.0).abs() < 0.01, "{}", r[1].ratio);
    assert_eq!(contrast::summary(&r).text.what, "fg on sheer");
}

#[test]
fn resolve_vars_names_the_sheet_and_place_of_the_winning_declaration() {
    let preset = ":root { --fg: #111111; --accent: #222222; --accent-text: var(--accent); --d: 13px; }\n\
                  @media (prefers-color-scheme: dark) {\n    :root { --fg: #eeeeee; }\n}\n";
    let user = "bar { --accent: #336699; }\n";
    let vars = contrast::resolve_vars(&chain(preset, user), Scheme::Dark);
    let (fg, origin) = &vars["fg"];
    assert_eq!(*fg, Color::parse("#eeeeee").unwrap());
    assert_eq!((origin.layer, origin.pos.line), (Layer::Preset, 3), "the dark override wins in the dark scheme");
    let light = contrast::resolve_vars(&chain(preset, user), Scheme::Light);
    assert_eq!(light["fg"].0, Color::parse("#111111").unwrap());
    assert_eq!((light["fg"].1.layer, light["fg"].1.pos.line), (Layer::Preset, 1));
    // The user's `bar` rule is the same selector as `:root`, and it comes later.
    assert_eq!(vars["accent"].0, Color::parse("#336699").unwrap());
    assert_eq!(
        (vars["accent"].1.layer, &*vars["accent"].1.pos.file, vars["accent"].1.pos.line),
        (Layer::User, "style.css", 1)
    );
    // A hand-over reports the place the colour was set, and a length is not a colour.
    assert_eq!(vars["accent-text"].0, Color::parse("#336699").unwrap());
    assert_eq!((&*vars["accent-text"].1.pos.file, vars["accent-text"].1.layer), ("style.css", Layer::User));
    assert!(!vars.contains_key("d"));
}

#[test]
fn a_colour_set_in_a_scheme_block_only_counts_in_that_scheme() {
    let user = "@media (prefers-color-scheme: light) { :root { --accent: #ff0000; } }";
    let preset = ":root { --accent: #00ff00; }";
    let dark = contrast::resolve_vars(&chain(preset, user), Scheme::Dark);
    let light = contrast::resolve_vars(&chain(preset, user), Scheme::Light);
    assert_eq!(dark["accent"].0, Color::parse("#00ff00").unwrap());
    assert_eq!(light["accent"].0, Color::parse("#ff0000").unwrap());
}

/// 24 hues at three lightnesses, plus black, white and a mid grey: 75 accents.
fn sample_accents() -> Vec<Color> {
    fn hsl(h: f32, s: f32, l: f32) -> Color {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        Color { r: r + m, g: g + m, b: b + m, a: 1.0 }
    }
    let mut out = Vec::new();
    for hue in 0..24 {
        for l in [0.25, 0.5, 0.75] {
            out.push(hsl(hue as f32 * 15.0, 0.85, l));
        }
    }
    out.extend([Color::BLACK, Color::WHITE, Color::rgba8(128, 128, 128, 255)]);
    out
}

#[test]
fn any_accent_keeps_every_pair() {
    let surfaces = surfaces_of_all_presets();
    let accents = sample_accents();
    assert_eq!(accents.len(), 75);
    let mut checked = 0;
    let mut lowest = (f64::MAX, String::new());
    let mut most_moved = 0;
    for accent in &accents {
        let set = contrast::derive_accent(*accent, &surfaces);
        assert!(
            set.schemes.dark.fg_ratio >= 4.5 && set.schemes.light.fg_ratio >= 4.5,
            "{}: {:?}",
            set.value,
            set.schemes
        );
        let block = colours_block(&set);
        for (name, toml, css) in PRESETS {
            let a = about(name, toml);
            let sheet = chain(css, &block);
            for scheme in Scheme::ALL {
                let vars = contrast::resolve_vars(&sheet, scheme);
                assert_eq!(contrast::hex(vars["accent"].0), set.value, "{name}: the accent is kept as picked");
                let results = contrast::pairs(&a, &vars);
                for p in &results {
                    assert!(
                        p.passes(),
                        "accent {} {name} {}: {} {:.3} < {}",
                        set.value,
                        scheme.name(),
                        p.what(),
                        p.ratio,
                        p.need
                    );
                    checked += 1;
                }
                let s = contrast::summary(&results);
                if s.text.lowest < lowest.0 {
                    lowest = (s.text.lowest, format!("{} {name} {} {}", set.value, scheme.name(), s.text.what));
                }
            }
        }
        most_moved = most_moved.max(set.schemes.dark.moved).max(set.schemes.light.moved);
    }
    assert!(checked > 75 * 2 * 40, "only {checked} pair checks");
    assert!(lowest.0 >= 4.5, "{lowest:?}");
    assert!(most_moved <= 100);
}

#[test]
fn the_derivation_follows_the_stated_rules() {
    let surfaces = surfaces_of_all_presets();
    let amber = contrast::derive_accent(Color::parse("#e5a50a").unwrap(), &surfaces);
    assert_eq!(amber.value, "#e5a50a");
    // Dark surfaces: the amber already reads; black is the better text on it.
    assert_eq!((amber.schemes.dark.fg.as_str(), amber.schemes.dark.text.as_str()), ("#000000", "#e5a50a"));
    assert_eq!(amber.schemes.dark.moved, 0);
    // Light surfaces: text and ring shades are darker than the amber, the ring less so.
    let light = &amber.schemes.light;
    assert!(light.moved > 0 && light.text != "#e5a50a" && light.line != "#e5a50a", "{light:?}");
    let lum = |h: &str| Color::parse(h).unwrap().relative_luminance();
    assert!(lum(&light.text) < lum(&light.line) && lum(&light.line) < lum("#e5a50a"), "{light:?}");
    // Hover and press move away from the foreground (toward white for black text), press further.
    assert!(
        lum(&amber.schemes.dark.hover) > lum("#e5a50a")
            && lum(&amber.schemes.dark.press) > lum(&amber.schemes.dark.hover)
    );
    // White text on a dark accent: hover and press move toward black.
    let navy = contrast::derive_accent(Color::parse("#112244").unwrap(), &surfaces);
    assert_eq!(navy.schemes.dark.fg, "#ffffff");
    assert!(
        lum(&navy.schemes.dark.hover) < lum("#112244") && lum(&navy.schemes.dark.press) < lum(&navy.schemes.dark.hover)
    );
    // Black on a dark scheme needs a lighter text shade; on a light scheme it is already dark.
    let black = contrast::derive_accent(Color::BLACK, &surfaces);
    assert!(black.schemes.dark.moved > 40 && black.schemes.light.text == "#000000", "{black:?}");
    // The alpha of a picked colour is dropped.
    let see_through = contrast::derive_accent(Color::rgba8(229, 165, 10, 40), &surfaces);
    assert_eq!(see_through.value, "#e5a50a");
}

#[test]
fn pure_white_and_black_pass_on_every_shipped_surface() {
    let surfaces = surfaces_of_all_presets();
    let on_all = |c: Color, list: &[Color], need: f32| list.iter().all(|s| Color::contrast_ratio(c, *s) >= need);
    assert!(on_all(Color::WHITE, &surfaces.dark.text, 4.5) && on_all(Color::WHITE, &surfaces.dark.line, 3.0));
    assert!(on_all(Color::BLACK, &surfaces.light.text, 4.5) && on_all(Color::BLACK, &surfaces.light.line, 3.0));
    assert!(surfaces.dark.text.len() >= 8 && surfaces.light.line.len() >= 5, "{surfaces:?}");
}

#[test]
fn the_surfaces_come_only_from_pairs_of_the_two_tokens() {
    let (name, toml, css) = PRESETS[0];
    let a = about(name, toml);
    let vars: Vars = contrast::resolve_vars(&chain(css, ""), Scheme::Dark);
    let mut s = AccentSurfaces::default();
    contrast::collect_accent_surfaces(&a, &vars, Scheme::Dark, &mut s);
    assert!(s.light.text.is_empty() && s.light.line.is_empty(), "only the dark scheme was collected");
    let seg0 = vars["seg0"].0;
    assert!(s.dark.line.contains(&seg0) && s.dark.text.contains(&vars["seg1"].0));
    // Only accent-text pairs count: panel is a surface of its icon pair.
    assert!(s.dark.text.contains(&vars["panel"].0));
    assert!(!s.dark.text.contains(&vars["row-hover"].0), "row-hover only holds fg");
}
