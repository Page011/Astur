//! The icon table against the real fonts: every code point must be in the
//! character map of both Segoe Fluent Icons (Windows 11) and Segoe MDL2
//! Assets (Windows 10), or an icon would draw as a missing-glyph box on one
//! of the two.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use rsb_core::icons::{ICON_NAMES, IconFont, icon_char};

fn be16(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?)))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// Code points that have a glyph, from the `cmap` table (formats 4 and 12).
fn cmap(font: &[u8]) -> Option<HashSet<u32>> {
    let tables = be16(font, 4)? as usize;
    let mut cmap_at = None;
    for i in 0..tables {
        let rec = 12 + 16 * i;
        if font.get(rec..rec + 4)? == b"cmap" {
            cmap_at = Some(be32(font, rec + 8)? as usize);
        }
    }
    let base = cmap_at?;
    let mut chars = HashSet::new();
    for i in 0..be16(font, base + 2)? as usize {
        let sub = base + be32(font, base + 8 + 8 * i)? as usize;
        match be16(font, sub)? {
            4 => {
                let segs = be16(font, sub + 6)? as usize / 2;
                let (ends, starts) = (sub + 14, sub + 16 + 2 * segs);
                let (deltas, offsets) = (starts + 2 * segs, starts + 4 * segs);
                for s in 0..segs {
                    let (end, start) = (be16(font, ends + 2 * s)?, be16(font, starts + 2 * s)?);
                    let delta = be16(font, deltas + 2 * s)?;
                    let range_offset = be16(font, offsets + 2 * s)?;
                    for ch in start..=end.min(0xFFFE) {
                        let glyph = if range_offset == 0 {
                            (ch + delta) & 0xFFFF
                        } else {
                            let at = offsets + 2 * s + range_offset as usize + 2 * (ch - start) as usize;
                            match be16(font, at)? {
                                0 => 0,
                                g => (g + delta) & 0xFFFF,
                            }
                        };
                        if glyph != 0 {
                            chars.insert(ch);
                        }
                    }
                }
            }
            12 => {
                for g in 0..be32(font, sub + 12)? as usize {
                    let at = sub + 16 + 12 * g;
                    for ch in be32(font, at)?..=be32(font, at + 4)? {
                        chars.insert(ch);
                    }
                }
            }
            _ => {}
        }
    }
    Some(chars)
}

fn system_font(file: &str) -> Option<Vec<u8>> {
    let dir = PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts");
    std::fs::read(dir.join(file)).ok()
}

#[test]
fn every_name_has_a_distinct_glyph_except_the_two_strongest_wifi_levels() {
    let mut by_cp: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    for name in ICON_NAMES {
        let c = icon_char(name).unwrap_or_else(|| panic!("{name} has no code point"));
        assert!((0xE000..=0xF8FF).contains(&(c as u32)), "{name}: {:X} is outside the private use area", c as u32);
        by_cp.entry(c as u32).or_default().push(name);
    }
    let shared: Vec<&Vec<&str>> = by_cp.values().filter(|v| v.len() > 1).collect();
    assert_eq!(shared, [&vec!["wifi_3", "wifi_4"]]);
    assert_eq!(icon_char("not_an_icon"), None);
    assert_eq!(IconFont::Fluent.family(), "Segoe Fluent Icons");
    assert_eq!(IconFont::Mdl2.family(), "Segoe MDL2 Assets");
}

#[test]
fn every_code_point_is_in_both_icon_fonts() {
    let fonts = [("Segoe Fluent Icons", "SegoeIcons.ttf"), ("Segoe MDL2 Assets", "segmdl2.ttf")];
    let mut checked = 0;
    for (family, file) in fonts {
        let Some(data) = system_font(file) else {
            eprintln!("icons: {family} is not installed here ({file}); its character map was not checked");
            continue;
        };
        let chars = cmap(&data).unwrap_or_else(|| panic!("{family}: the character map could not be read"));
        assert!(chars.len() > 500, "{family}: only {} characters read; the parser is wrong", chars.len());
        let missing: Vec<&str> =
            ICON_NAMES.iter().copied().filter(|n| !chars.contains(&(icon_char(n).unwrap() as u32))).collect();
        assert!(missing.is_empty(), "{family} has no glyph for {missing:?}");
        eprintln!(
            "icons: {family}: {} icons present of {} ({} private-use glyphs in the font)",
            ICON_NAMES.len(),
            ICON_NAMES.len(),
            chars.iter().filter(|c| (0xE000..=0xF8FF).contains(*c)).count()
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("icons: no icon font found, nothing checked");
    }
}
