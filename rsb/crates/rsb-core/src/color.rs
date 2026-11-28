//! Colours: sRGB, straight (not premultiplied) alpha, every channel 0..=1.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }
    }

    /// CSS colour syntax: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`,
    /// `rgb()`/`rgba()`, `hsl()`/`hsla()` (comma or space separated, optional
    /// `/ alpha`), the 148 CSS named colours and `transparent`.
    /// Case-insensitive. `currentcolor` is not a colour value: the cascade
    /// resolves it before calling this. The error text is the short reason
    /// shown after the file position.
    pub fn parse(text: &str) -> Result<Color, String> {
        let t = text.trim();
        let lower = t.to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix('#') {
            return parse_hex(hex).ok_or_else(|| format!("bad colour '{t}': use #rgb, #rgba, #rrggbb or #rrggbbaa"));
        }
        if let Some(open) = lower.find('(') {
            let name = &lower[..open];
            let Some(inner) = lower[open + 1..].strip_suffix(')') else {
                return Err(format!("bad colour '{t}': missing ')'"));
            };
            return match name {
                "rgb" | "rgba" => parse_rgb(inner)
                    .ok_or_else(|| format!("bad colour '{t}': use rgb(255, 128, 0) or rgb(255 128 0 / 50%)")),
                "hsl" | "hsla" => parse_hsl(inner)
                    .ok_or_else(|| format!("bad colour '{t}': use hsl(210, 50%, 40%) or hsl(210 50% 40% / 0.5)")),
                _ => Err(format!("unknown colour function '{name}()'; use rgb(), rgba(), hsl() or hsla()")),
            };
        }
        if lower == "transparent" {
            return Ok(Color::TRANSPARENT);
        }
        if lower == "currentcolor" {
            return Err("currentcolor is only allowed as a property value".into());
        }
        match NAMED.binary_search_by(|(n, _)| (*n).cmp(lower.as_str())) {
            Ok(i) => {
                let v = NAMED[i].1;
                Ok(Color::rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, 255))
            }
            Err(_) => Err(format!("unknown colour '{t}'{}", crate::suggest::hint(&lower, named_colours()))),
        }
    }

    /// Straight-alpha interpolation in sRGB, `t` clamped to 0..=1. This is
    /// what CSS transitions of colours do.
    pub fn lerp(self, to: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let m = |a: f32, b: f32| a + (b - a) * t;
        Color { r: m(self.r, to.r), g: m(self.g, to.g), b: m(self.b, to.b), a: m(self.a, to.a) }
    }

    /// `self` composited source-over onto `bg`.
    pub fn over(self, bg: Color) -> Color {
        let a = self.a + bg.a * (1.0 - self.a);
        if a <= 0.0 {
            return Color::TRANSPARENT;
        }
        let ch = |s: f32, d: f32| (s * self.a + d * bg.a * (1.0 - self.a)) / a;
        Color { r: ch(self.r, bg.r), g: ch(self.g, bg.g), b: ch(self.b, bg.b), a }
    }

    /// WCAG 2 relative luminance of the opaque colour.
    pub fn relative_luminance(self) -> f32 {
        fn lin(c: f32) -> f32 {
            if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG 2 contrast ratio of two opaque colours, 1..=21.
    pub fn contrast_ratio(a: Color, b: Color) -> f32 {
        let (la, lb) = (a.relative_luminance(), b.relative_luminance());
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    let d: Vec<u8> = hex.chars().map(|c| c.to_digit(16).map(|v| v as u8)).collect::<Option<_>>()?;
    match d.len() {
        3 | 4 => {
            let a = if d.len() == 4 { d[3] * 17 } else { 255 };
            Some(Color::rgba8(d[0] * 17, d[1] * 17, d[2] * 17, a))
        }
        6 | 8 => {
            let byte = |i: usize| d[i] * 16 + d[i + 1];
            let a = if d.len() == 8 { byte(6) } else { 255 };
            Some(Color::rgba8(byte(0), byte(2), byte(4), a))
        }
        _ => None,
    }
}

/// Splits `a, b, c[, d]` or `a b c [/ d]` into three parts and an optional
/// alpha.
fn split_args(inner: &str) -> Option<([&str; 3], Option<&str>)> {
    let parts: Vec<&str> = if inner.contains(',') {
        let p: Vec<&str> = inner.split(',').map(str::trim).collect();
        if p.len() != 3 && p.len() != 4 {
            return None;
        }
        p
    } else {
        let (main, alpha) = match inner.split_once('/') {
            Some((m, a)) => (m, Some(a.trim())),
            None => (inner, None),
        };
        let mut p: Vec<&str> = main.split_whitespace().collect();
        if p.len() != 3 {
            return None;
        }
        if let Some(a) = alpha {
            p.push(a);
        }
        p
    };
    if parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    Some(([parts[0], parts[1], parts[2]], parts.get(3).copied()))
}

fn number(s: &str) -> Option<f32> {
    let v: f32 = s.parse().ok()?;
    v.is_finite().then_some(v)
}

/// `50%` -> half of `full`; a plain number as is.
fn number_or_percent(s: &str, full: f32) -> Option<f32> {
    match s.strip_suffix('%') {
        Some(p) => Some(number(p)? / 100.0 * full),
        None => number(s),
    }
}

fn alpha(s: Option<&str>) -> Option<f32> {
    match s {
        None => Some(1.0),
        Some(a) => Some(number_or_percent(a, 1.0)?.clamp(0.0, 1.0)),
    }
}

fn parse_rgb(inner: &str) -> Option<Color> {
    let (rgb, a) = split_args(inner)?;
    let ch = |s: &str| Some(number_or_percent(s, 255.0)?.clamp(0.0, 255.0) / 255.0);
    Some(Color { r: ch(rgb[0])?, g: ch(rgb[1])?, b: ch(rgb[2])?, a: alpha(a)? })
}

fn parse_hsl(inner: &str) -> Option<Color> {
    let (hsl, a) = split_args(inner)?;
    let h = if let Some(d) = hsl[0].strip_suffix("deg") {
        number(d)?
    } else if let Some(t) = hsl[0].strip_suffix("turn") {
        number(t)? * 360.0
    } else {
        number(hsl[0])?
    };
    let pct = |s: &str| Some((number(s.strip_suffix('%').unwrap_or(s))? / 100.0).clamp(0.0, 1.0));
    let (s, l) = (pct(hsl[1])?, pct(hsl[2])?);
    // CSS Color 4 hsl-to-rgb.
    let h = h.rem_euclid(360.0) / 30.0;
    let amp = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h).rem_euclid(12.0);
        l - amp * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    Some(Color { r: f(0.0), g: f(8.0), b: f(4.0), a: alpha(a)? })
}

/// Every named colour, for the docs and did-you-mean hints.
pub fn named_colours() -> impl Iterator<Item = &'static str> {
    NAMED.iter().map(|(n, _)| *n)
}

/// The CSS named colours, sorted by name for binary search.
static NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xF0F8FF),
    ("antiquewhite", 0xFAEBD7),
    ("aqua", 0x00FFFF),
    ("aquamarine", 0x7FFFD4),
    ("azure", 0xF0FFFF),
    ("beige", 0xF5F5DC),
    ("bisque", 0xFFE4C4),
    ("black", 0x000000),
    ("blanchedalmond", 0xFFEBCD),
    ("blue", 0x0000FF),
    ("blueviolet", 0x8A2BE2),
    ("brown", 0xA52A2A),
    ("burlywood", 0xDEB887),
    ("cadetblue", 0x5F9EA0),
    ("chartreuse", 0x7FFF00),
    ("chocolate", 0xD2691E),
    ("coral", 0xFF7F50),
    ("cornflowerblue", 0x6495ED),
    ("cornsilk", 0xFFF8DC),
    ("crimson", 0xDC143C),
    ("cyan", 0x00FFFF),
    ("darkblue", 0x00008B),
    ("darkcyan", 0x008B8B),
    ("darkgoldenrod", 0xB8860B),
    ("darkgray", 0xA9A9A9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xA9A9A9),
    ("darkkhaki", 0xBDB76B),
    ("darkmagenta", 0x8B008B),
    ("darkolivegreen", 0x556B2F),
    ("darkorange", 0xFF8C00),
    ("darkorchid", 0x9932CC),
    ("darkred", 0x8B0000),
    ("darksalmon", 0xE9967A),
    ("darkseagreen", 0x8FBC8F),
    ("darkslateblue", 0x483D8B),
    ("darkslategray", 0x2F4F4F),
    ("darkslategrey", 0x2F4F4F),
    ("darkturquoise", 0x00CED1),
    ("darkviolet", 0x9400D3),
    ("deeppink", 0xFF1493),
    ("deepskyblue", 0x00BFFF),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1E90FF),
    ("firebrick", 0xB22222),
    ("floralwhite", 0xFFFAF0),
    ("forestgreen", 0x228B22),
    ("fuchsia", 0xFF00FF),
    ("gainsboro", 0xDCDCDC),
    ("ghostwhite", 0xF8F8FF),
    ("gold", 0xFFD700),
    ("goldenrod", 0xDAA520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xADFF2F),
    ("grey", 0x808080),
    ("honeydew", 0xF0FFF0),
    ("hotpink", 0xFF69B4),
    ("indianred", 0xCD5C5C),
    ("indigo", 0x4B0082),
    ("ivory", 0xFFFFF0),
    ("khaki", 0xF0E68C),
    ("lavender", 0xE6E6FA),
    ("lavenderblush", 0xFFF0F5),
    ("lawngreen", 0x7CFC00),
    ("lemonchiffon", 0xFFFACD),
    ("lightblue", 0xADD8E6),
    ("lightcoral", 0xF08080),
    ("lightcyan", 0xE0FFFF),
    ("lightgoldenrodyellow", 0xFAFAD2),
    ("lightgray", 0xD3D3D3),
    ("lightgreen", 0x90EE90),
    ("lightgrey", 0xD3D3D3),
    ("lightpink", 0xFFB6C1),
    ("lightsalmon", 0xFFA07A),
    ("lightseagreen", 0x20B2AA),
    ("lightskyblue", 0x87CEFA),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xB0C4DE),
    ("lightyellow", 0xFFFFE0),
    ("lime", 0x00FF00),
    ("limegreen", 0x32CD32),
    ("linen", 0xFAF0E6),
    ("magenta", 0xFF00FF),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66CDAA),
    ("mediumblue", 0x0000CD),
    ("mediumorchid", 0xBA55D3),
    ("mediumpurple", 0x9370DB),
    ("mediumseagreen", 0x3CB371),
    ("mediumslateblue", 0x7B68EE),
    ("mediumspringgreen", 0x00FA9A),
    ("mediumturquoise", 0x48D1CC),
    ("mediumvioletred", 0xC71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xF5FFFA),
    ("mistyrose", 0xFFE4E1),
    ("moccasin", 0xFFE4B5),
    ("navajowhite", 0xFFDEAD),
    ("navy", 0x000080),
    ("oldlace", 0xFDF5E6),
    ("olive", 0x808000),
    ("olivedrab", 0x6B8E23),
    ("orange", 0xFFA500),
    ("orangered", 0xFF4500),
    ("orchid", 0xDA70D6),
    ("palegoldenrod", 0xEEE8AA),
    ("palegreen", 0x98FB98),
    ("paleturquoise", 0xAFEEEE),
    ("palevioletred", 0xDB7093),
    ("papayawhip", 0xFFEFD5),
    ("peachpuff", 0xFFDAB9),
    ("peru", 0xCD853F),
    ("pink", 0xFFC0CB),
    ("plum", 0xDDA0DD),
    ("powderblue", 0xB0E0E6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xFF0000),
    ("rosybrown", 0xBC8F8F),
    ("royalblue", 0x4169E1),
    ("saddlebrown", 0x8B4513),
    ("salmon", 0xFA8072),
    ("sandybrown", 0xF4A460),
    ("seagreen", 0x2E8B57),
    ("seashell", 0xFFF5EE),
    ("sienna", 0xA0522D),
    ("silver", 0xC0C0C0),
    ("skyblue", 0x87CEEB),
    ("slateblue", 0x6A5ACD),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xFFFAFA),
    ("springgreen", 0x00FF7F),
    ("steelblue", 0x4682B4),
    ("tan", 0xD2B48C),
    ("teal", 0x008080),
    ("thistle", 0xD8BFD8),
    ("tomato", 0xFF6347),
    ("turquoise", 0x40E0D0),
    ("violet", 0xEE82EE),
    ("wheat", 0xF5DEB3),
    ("white", 0xFFFFFF),
    ("whitesmoke", 0xF5F5F5),
    ("yellow", 0xFFFF00),
    ("yellowgreen", 0x9ACD32),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color) -> bool {
        (a.r - b.r).abs() < 0.003 && (a.g - b.g).abs() < 0.003 && (a.b - b.b).abs() < 0.003 && (a.a - b.a).abs() < 0.003
    }

    #[test]
    fn black_on_white_is_21() {
        let r = Color::contrast_ratio(Color::BLACK, Color::WHITE);
        assert!((r - 21.0).abs() < 0.01, "{r}");
    }

    #[test]
    fn over_opaque_background_is_opaque() {
        let c = Color { a: 0.5, ..Color::WHITE }.over(Color::BLACK);
        assert!((c.a - 1.0).abs() < 1e-6 && (c.r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hex_forms() {
        assert_eq!(Color::parse("#fff"), Ok(Color::WHITE));
        assert_eq!(Color::parse("#FFFFFF"), Ok(Color::WHITE));
        assert_eq!(Color::parse("#00000080"), Ok(Color::rgba8(0, 0, 0, 128)));
        assert_eq!(Color::parse("#1A1B26"), Ok(Color::rgba8(0x1A, 0x1B, 0x26, 255)));
        assert_eq!(Color::parse("#f008"), Ok(Color::rgba8(255, 0, 0, 0x88)));
        for bad in ["#", "#ff", "#fffff", "#ggg", "#1234567", "#\u{e9}e9"] {
            assert!(Color::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn functions() {
        assert_eq!(Color::parse("rgb(255, 0, 0)"), Ok(Color::rgba8(255, 0, 0, 255)));
        assert_eq!(Color::parse("rgba(255,0,0,0.5)").map(|c| c.a), Ok(0.5));
        assert_eq!(Color::parse("rgb(255 0 0 / 50%)").map(|c| c.a), Ok(0.5));
        assert!(close(Color::parse("rgb(100%, 50%, 0%)").unwrap(), Color { r: 1.0, g: 0.5, b: 0.0, a: 1.0 }));
        assert_eq!(Color::parse("RGB(300, -5, 0)"), Ok(Color::rgba8(255, 0, 0, 255)));
        assert!(close(Color::parse("hsl(0, 100%, 50%)").unwrap(), Color::rgba8(255, 0, 0, 255)));
        assert!(close(Color::parse("hsl(120 100% 25%)").unwrap(), Color { r: 0.0, g: 0.5, b: 0.0, a: 1.0 }));
        assert!(close(Color::parse("hsla(240, 100%, 50%, 0.25)").unwrap(), Color { r: 0.0, g: 0.0, b: 1.0, a: 0.25 }));
        assert!(close(Color::parse("hsl(0.5turn 100% 50%)").unwrap(), Color::rgba8(0, 255, 255, 255)));
        for bad in ["rgb(1, 2)", "rgb(1 2 3 4)", "rgb(1, 2, 3", "rgb(a, b, c)", "lab(1 2 3)", "rgb(1,,2,3)"] {
            assert!(Color::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(named_colours().count(), 148);
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0), "sorted");
        assert_eq!(Color::parse("RebeccaPurple"), Ok(Color::rgba8(0x66, 0x33, 0x99, 255)));
        assert_eq!(Color::parse("transparent"), Ok(Color::TRANSPARENT));
        assert!(Color::parse("currentcolor").is_err());
        assert_eq!(Color::parse("redd"), Err("unknown colour 'redd'; did you mean 'red'?".into()));
        assert_eq!(Color::parse("blu"), Err("unknown colour 'blu'; did you mean 'blue'?".into()));
        assert_eq!(Color::parse("zzzzzzzz"), Err("unknown colour 'zzzzzzzz'".into()));
    }
}
