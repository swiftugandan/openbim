//! Typed measurements (spec/sketch.md "## Measurements"): what users type while a tool runs.
//!
//! The grammar (surrounding whitespace ignored):
//!
//! | Input | Result |
//! |---|---|
//! | `2500`, `2500mm`, `250cm`, `2.5m` | [`Measurement::Length`]; a bare number is in the default unit |
//! | `8'6"`, `8'`, `6"` | [`Measurement::Length`] in feet and inches |
//! | `4000,3000` | [`Measurement::Lengths`]: two lengths, each as above |
//! | `x5`, `5x` | [`Measurement::Array`]: 5 copies at the same spacing |
//! | `/5` | [`Measurement::Divide`]: 5 equal spaces between the original and the copy |
//! | `45` with an angle tool | [`Measurement::Angle`]: degrees, returned in radians |
//!
//! Numbers are unsigned decimals (`12`, `2.5`, `.5`, `5.`); units are `mm`, `cm` and `m`, optionally
//! separated from the number by spaces. Counts are positive integers. Anything else is an error.

use crate::SketchError;

/// A parsed measurement: lengths in metres, angles in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measurement {
    /// One length (metres).
    Length(f64),
    /// Two lengths (metres), e.g. a rectangle's sides.
    Lengths(f64, f64),
    /// An angle (radians), typed in degrees with an angle tool.
    Angle(f64),
    /// After a copy: this many copies at the same spacing.
    Array(u32),
    /// After a copy: this many equal spaces between the original and the copy.
    Divide(u32),
}

/// Parses `text` as typed while a tool runs. Bare numbers are lengths in `default_unit` (`mm`, `cm`
/// or `m`; empty means `mm`), or degrees when `angle` is true (an angle tool).
pub fn parse_measurement(text: &str, default_unit: &str, angle: bool) -> Result<Measurement, SketchError> {
    let err = |why: &str| SketchError(format!("measurement {text:?}: {why}"));
    let unit_scale = match default_unit.trim() {
        "" | "mm" => 1000.0,
        "cm" => 100.0,
        "m" => 1.0,
        other => return Err(SketchError(format!("unknown default unit {other:?} (expected mm, cm or m)"))),
    };
    let t = text.trim();
    if t.is_empty() {
        return Err(err("nothing typed"));
    }
    if let Some(n) = t.strip_prefix('x') {
        return count(n).map(Measurement::Array).ok_or_else(|| err("expected a positive whole number of copies"));
    }
    if let Some(n) = t.strip_suffix('x') {
        return count(n).map(Measurement::Array).ok_or_else(|| err("expected a positive whole number of copies"));
    }
    if let Some(n) = t.strip_prefix('/') {
        return count(n).map(Measurement::Divide).ok_or_else(|| err("expected a positive whole number of spaces"));
    }
    if angle {
        let d = t.strip_suffix('°').unwrap_or(t).trim_end();
        return match number(d) {
            Some((x, "")) => Ok(Measurement::Angle(x * std::f64::consts::PI / 180.0)),
            _ => Err(err("expected an angle in degrees")),
        };
    }
    if let Some((a, b)) = t.split_once(',') {
        let a = length(a.trim(), unit_scale).ok_or_else(|| err("expected two lengths separated by a comma"))?;
        let b = length(b.trim(), unit_scale).ok_or_else(|| err("expected two lengths separated by a comma"))?;
        return Ok(Measurement::Lengths(a, b));
    }
    length(t, unit_scale).map(Measurement::Length).ok_or_else(|| err("expected a length such as 2500, 2.5m or 8'6\""))
}

/// A positive whole number (digits only).
fn count(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok().filter(|&n| n > 0)
}

/// An unsigned decimal at the start of `s`, and the rest of `s`.
fn number(s: &str) -> Option<(f64, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    let x: f64 = s[..i].parse().ok()?;
    x.is_finite().then_some((x, &s[i..]))
}

/// A length in metres: a number with an optional unit (`mm`, `cm`, `m`; else the default unit,
/// `per_metre` of which make a metre), or feet and inches (`8'6"`, `8'`, `6"`).
fn length(s: &str, per_metre: f64) -> Option<f64> {
    let (x, rest) = number(s)?;
    let rest = rest.trim_start();
    let m = match rest {
        "" => x / per_metre,
        "mm" => x / 1000.0,
        "cm" => x / 100.0,
        "m" => x,
        "\"" => x * 0.0254,
        _ => {
            let after = rest.strip_prefix('\'')?.trim_start();
            if after.is_empty() {
                x * 0.3048
            } else {
                let (inches, tail) = number(after)?;
                if tail.trim_start() != "\"" {
                    return None;
                }
                x * 0.3048 + inches * 0.0254
            }
        }
    };
    m.is_finite().then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar() {
        let l = |t: &str| match parse_measurement(t, "mm", false) {
            Ok(Measurement::Length(x)) => x,
            other => panic!("{t:?}: {other:?}"),
        };
        assert_eq!(l(".5m"), 0.5);
        assert_eq!(l(" 2500 mm "), 2.5);
        assert!((l("8' 6\"") - 2.5908).abs() < 1e-12);
        assert!((l("8'6.5\"") - (8.0 * 0.3048 + 6.5 * 0.0254)).abs() < 1e-12);
        assert_eq!(parse_measurement("8',6\"", "mm", false), Ok(Measurement::Lengths(8.0 * 0.3048, 6.0 * 0.0254)));
        assert_eq!(parse_measurement("x3", "mm", true), Ok(Measurement::Array(3)));
        assert_eq!(parse_measurement("90°", "mm", true), Ok(Measurement::Angle(std::f64::consts::FRAC_PI_2)));
        for bad in ["x0", "/0", "-5", "+5", "1e3", "45mm", "5 x 2", "4000,", ",3000", "2,5,", "'", "\"", "8'6", "6\"'"] {
            assert!(parse_measurement(bad, "mm", bad == "45mm").is_err(), "{bad:?}");
        }
        assert!(parse_measurement("5", "parsecs", false).is_err());
        assert_eq!(parse_measurement("5", "", false), Ok(Measurement::Length(0.005)));
    }
}
