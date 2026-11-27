//! A colour picked through the edit engine keeps every pair of every preset
//! readable: the block the engine writes into `style.css`, laid over each
//! preset as the bar lays it, is measured against the preset's own pair lists.

use rsb_app::{assets, cfgload};
use rsb_core::cfgedit::{FileTexts, PlanContext, plan_change};
use rsb_core::color::Color;
use rsb_core::config::BUILTIN_PRESETS;
use rsb_core::contract::{ConfigFile, Op};
use rsb_core::contrast;
use rsb_core::preset::Scheme;

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
fn any_accent_set_through_the_engine_keeps_every_pair() {
    let dir = std::env::temp_dir().join(format!("rsb-accent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let preset_toml = |n: &str| assets::preset_toml(n).map(str::to_string);
    let exists = |_: &str| true;
    let opens = |_: &str| Vec::<String>::new();
    let template = |f: ConfigFile| {
        let rel = match f {
            ConfigFile::BarToml => "config/bar.toml",
            ConfigFile::StyleCss => "config/style.css",
        };
        assets::file(rel).unwrap_or("").to_string()
    };
    let ctx = PlanContext {
        reset: false,
        preset_toml: &preset_toml,
        widget_exists: &exists,
        widget_opens: &opens,
        template: &template,
        surfaces: assets::accent_surfaces(),
        style_chain: &|_| None,
    };
    let accents = sample_accents();
    assert_eq!(accents.len(), 75);
    let mut checked = 0;
    for accent in &accents {
        let value = contrast::hex(*accent);
        let ops = [Op::Set { key: "color.accent".into(), value: serde_json::json!(value) }];
        let plan = plan_change(&FileTexts { bar_toml: None, style_css: None }, &ops, &ctx).expect("the accent plans");
        let css = plan.file(ConfigFile::StyleCss).expect("the colours block is written to style.css");
        std::fs::write(dir.join("style.css"), css.after.as_bytes()).unwrap();
        for name in BUILTIN_PRESETS {
            let about = assets::preset_about(name).expect("the preset's about table parses");
            let built = cfgload::build_as(&dir, name);
            let sheet = built.sheet.expect("the preset and the written block build a style sheet");
            for scheme in Scheme::ALL {
                let vars = contrast::resolve_vars(&sheet, scheme);
                assert_eq!(contrast::hex(vars["accent"].0), value, "{name}: the accent is kept as picked");
                for p in contrast::pairs(&about, &vars) {
                    assert!(
                        p.passes(),
                        "accent {value} {name} {}: {} {:.3} < {}",
                        scheme.name(),
                        p.what(),
                        p.ratio,
                        p.need
                    );
                    checked += 1;
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(checked > 75 * 2 * 40, "only {checked} pair checks");
}
