//! Transitions as pure functions: time in, values out. The scene keeps one
//! running transition per (node, property); while any runs it asks for the
//! next frame, and when none runs it asks for nothing.

use crate::color::Color;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Easing {
    Linear,
    #[default]
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// `cubic-bezier(x1, y1, x2, y2)`, x1 and x2 in 0..=1.
    CubicBezier(f32, f32, f32, f32),
    /// `steps(n, end)` (`start` when the flag is true): n equal jumps.
    Steps(u32, bool),
}

impl Easing {
    /// Progress 0..=1 to eased progress, CSS definitions (`ease` is
    /// cubic-bezier(0.25, 0.1, 0.25, 1)). `t` is clamped.
    pub fn apply(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let (x1, y1, x2, y2) = match self {
            Easing::Linear => return t,
            Easing::Steps(n, jump_start) => {
                let n = n.max(1) as f32;
                let k = if jump_start { (t * n).ceil() } else { (t * n).floor() };
                return (k / n).clamp(0.0, 1.0);
            }
            Easing::Ease => (0.25, 0.1, 0.25, 1.0),
            Easing::EaseIn => (0.42, 0.0, 1.0, 1.0),
            Easing::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Easing::EaseInOut => (0.42, 0.0, 0.58, 1.0),
            Easing::CubicBezier(a, b, c, d) => (a.clamp(0.0, 1.0), b, c.clamp(0.0, 1.0), d),
        };
        if t == 0.0 || t == 1.0 {
            return t;
        }
        let s = solve_bezier_x(x1 as f64, x2 as f64, t as f64);
        bezier(y1 as f64, y2 as f64, s) as f32
    }
}

/// One coordinate of a cubic Bezier from (0,0) to (1,1) at parameter `s`.
fn bezier(p1: f64, p2: f64, s: f64) -> f64 {
    let u = 1.0 - s;
    3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s
}

fn bezier_slope(p1: f64, p2: f64, s: f64) -> f64 {
    let u = 1.0 - s;
    3.0 * u * u * p1 + 6.0 * u * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

/// The parameter where the curve's x equals `x`: Newton steps, then
/// bisection when the slope is too flat to trust.
fn solve_bezier_x(x1: f64, x2: f64, x: f64) -> f64 {
    let mut s = x;
    for _ in 0..8 {
        let err = bezier(x1, x2, s) - x;
        if err.abs() < 1e-7 {
            return s;
        }
        let d = bezier_slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s -= err / d;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    s = x;
    for _ in 0..60 {
        let v = bezier(x1, x2, s);
        if (v - x).abs() < 1e-7 {
            break;
        }
        if v < x {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    s
}

/// A value that can be interpolated.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimValue {
    Num(f32),
    Color(Color),
    /// Box shadows, radii, edges: same length lists interpolate item-wise;
    /// different lengths jump at the end.
    List(Vec<AnimValue>),
}

/// `from` + (`to` - `from`) * `t`, `t` clamped to 0..=1. Mismatched kinds and
/// lists of different lengths keep `from` until `t` reaches 1, then `to`.
pub fn interpolate(from: &AnimValue, to: &AnimValue, t: f32) -> AnimValue {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    match (from, to) {
        (AnimValue::Num(a), AnimValue::Num(b)) => AnimValue::Num(a + (b - a) * t),
        (AnimValue::Color(a), AnimValue::Color(b)) => AnimValue::Color(a.lerp(*b, t)),
        (AnimValue::List(a), AnimValue::List(b)) if a.len() == b.len() => {
            AnimValue::List(a.iter().zip(b).map(|(x, y)| interpolate(x, y, t)).collect())
        }
        _ => {
            if t >= 1.0 {
                to.clone()
            } else {
                from.clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_reference_values() {
        // Reference points computed from the CSS definitions (cubic-bezier
        // solved to 1e-9).
        let cases = [
            (Easing::Ease, 0.25, 0.408_5),
            (Easing::Ease, 0.5, 0.802_4),
            (Easing::Ease, 0.75, 0.960_5),
            (Easing::EaseIn, 0.5, 0.315_4),
            (Easing::EaseIn, 0.25, 0.093_5),
            (Easing::EaseOut, 0.5, 0.684_6),
            (Easing::EaseInOut, 0.5, 0.5),
            (Easing::EaseInOut, 0.25, 0.129_2),
            (Easing::Linear, 0.3, 0.3),
        ];
        for (e, t, want) in cases {
            let got = e.apply(t);
            assert!((got - want).abs() < 0.001, "{e:?}({t}) = {got}, want {want}");
        }
    }

    #[test]
    fn steps_jump_at_the_end_or_the_start_of_each_step() {
        assert_eq!(Easing::Steps(4, false).apply(0.0), 0.0);
        assert_eq!(Easing::Steps(4, false).apply(0.24), 0.0);
        assert_eq!(Easing::Steps(4, false).apply(0.25), 0.25);
        assert_eq!(Easing::Steps(4, false).apply(0.99), 0.75);
        assert_eq!(Easing::Steps(4, false).apply(1.0), 1.0);
        assert_eq!(Easing::Steps(4, true).apply(0.01), 0.25);
        assert_eq!(Easing::Steps(4, true).apply(0.5), 0.5);
        assert_eq!(Easing::Steps(1, true).apply(0.5), 1.0);
    }

    #[test]
    fn ends_and_clamping() {
        for e in [Easing::Ease, Easing::EaseIn, Easing::CubicBezier(0.9, -0.5, 0.1, 1.5)] {
            assert_eq!(e.apply(0.0), 0.0);
            assert_eq!(e.apply(1.0), 1.0);
            assert_eq!(e.apply(-1.0), 0.0);
            assert_eq!(e.apply(2.0), 1.0);
            assert_eq!(e.apply(f32::NAN), 0.0);
        }
        // Overshooting curves may leave 0..1 in between.
        assert!(Easing::CubicBezier(0.3, 1.8, 0.7, 1.8).apply(0.5) > 1.0);
    }

    #[test]
    fn interpolation() {
        assert_eq!(interpolate(&AnimValue::Num(0.0), &AnimValue::Num(10.0), 0.25), AnimValue::Num(2.5));
        let c = interpolate(&AnimValue::Color(Color::BLACK), &AnimValue::Color(Color::WHITE), 0.5);
        assert_eq!(c, AnimValue::Color(Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }));
        let a = AnimValue::List(vec![AnimValue::Num(0.0), AnimValue::Num(2.0)]);
        let b = AnimValue::List(vec![AnimValue::Num(4.0), AnimValue::Num(2.0)]);
        assert_eq!(interpolate(&a, &b, 0.5), AnimValue::List(vec![AnimValue::Num(2.0), AnimValue::Num(2.0)]));
        let short = AnimValue::List(vec![AnimValue::Num(1.0)]);
        assert_eq!(interpolate(&a, &short, 0.99), a);
        assert_eq!(interpolate(&a, &short, 1.0), short);
        assert_eq!(interpolate(&AnimValue::Num(1.0), &AnimValue::Color(Color::WHITE), 0.5), AnimValue::Num(1.0));
    }
}
