//! A color's spoken name, from its hue, saturation, and brightness: "dark
//! red", "light pale blue", "grey" (milestone M4 item 7).
//!
//! Ported from NVDA's `source/colors.py` (`_calcColorName` and its tables;
//! copyright NV Access Limited and contributors, GPL version 2 or later,
//! used here under GPL-3.0-or-later). The names are English, as NVDA's
//! untranslated ones are; wording them by locale is a follow-up for the
//! presentation stage.

/// The grey shades by brightness in percent.
const SHADES: [(i32, &str); 7] = [
    (100, "white"),
    (84, "very light grey"),
    (66, "light grey"),
    (50, "grey"),
    (34, "dark grey"),
    (16, "very dark grey"),
    (0, "black"),
];

/// The named hues by angle in degrees.
const HUES: [(i32, &str); 16] = [
    (0, "red"),
    (15, "red-orange"),
    (30, "orange"),
    (45, "orange-yellow"),
    (60, "yellow"),
    (90, "yellow-green"),
    (120, "green"),
    (150, "green-aqua"),
    (180, "aqua"),
    (210, "aqua-blue"),
    (240, "blue"),
    (270, "blue-purple"),
    (300, "purple"),
    (312, "purple-pink"),
    (324, "pink"),
    (342, "pink-red"),
];

/// The hues that are brown when dark.
const BROWN_HUES: [(i32, &str); 3] = [(15, "red-brown"), (30, "brown"), (45, "brown-yellow")];

/// A brightness label: the words before and after the hue's name.
type Label = (&'static str, &'static str);

/// The brightness labels of one saturation, by brightness in percent.
type Labels = [(i32, Label); 4];

/// The brightness labels by saturation in percent.
const BRIGHTNESS: [(i32, Labels); 3] = [
    (
        100,
        [
            (100, ("bright ", "")),
            (72, ("", "")),
            (44, ("dark ", "")),
            (16, ("very dark ", "")),
        ],
    ),
    (
        60,
        [
            (100, ("light pale ", "")),
            (72, ("pale ", "")),
            (44, ("dark pale ", "")),
            (16, ("very dark pale ", "")),
        ],
    ),
    (
        10,
        [
            (100, ("", " white")),
            (72, ("", " grey")),
            (44, ("dark ", " grey")),
            (16, ("very dark ", " grey")),
        ],
    ),
];

/// The entry of `table` whose key is nearest `value` by `distance`, the
/// first of equals.
fn nearest<T: Copy>(
    table: &[(i32, T)],
    value: f64,
    distance: impl Fn(f64, f64) -> f64,
) -> (i32, T) {
    let mut best = table[0];
    for &entry in &table[1..] {
        if distance(f64::from(entry.0), value) < distance(f64::from(best.0), value) {
            best = entry;
        }
    }
    best
}

/// The spoken name of a Windows `COLORREF`, `0x00bbggrr`.
#[must_use]
pub fn color_name(colorref: u32) -> String {
    let [red, green, blue, _] = colorref.to_le_bytes();
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let value = f64::from(max) / 255.0;
    let saturation = if max > 0 {
        f64::from(max - min) / f64::from(max)
    } else {
        0.0
    };
    let linear = |a: f64, b: f64| (a - b).abs();
    if saturation * value < 0.02 {
        // Too little saturation to see a hue: a shade from black to white.
        return nearest(&SHADES, value * 100.0, linear).1.to_owned();
    }
    let hue = hue_degrees([red, green, blue], max, min);
    let circular = |a: f64, b: f64| 180.0 - ((a - b).abs() - 180.0).abs();
    let (hue_key, mut hue_name) = nearest(&HUES, hue, circular);
    let (_, labels) = nearest(&BRIGHTNESS, saturation * 100.0, linear);
    let (mut brightness, _) = nearest(&labels, value * 100.0, linear);
    if let Some(&(_, brown)) = BROWN_HUES.iter().find(|(key, _)| *key == hue_key) {
        // Dark oranges are browns, named a step brighter.
        let redirected = match brightness {
            44 => Some(72),
            16 => Some(44),
            _ => None,
        };
        if let Some(redirected) = redirected {
            brightness = redirected;
            hue_name = brown;
        }
    }
    let (before, after) = labels
        .iter()
        .find(|(key, _)| *key == brightness)
        .map_or(("", ""), |&(_, words)| words);
    format!("{before}{hue_name}{after}")
}

/// The hue of a color with some saturation, in degrees, as the HSV model
/// defines it.
fn hue_degrees([red, green, blue]: [u8; 3], max: u8, min: u8) -> f64 {
    let range = f64::from(max - min);
    let channel = f64::from;
    let sixths = if max == red {
        ((channel(green) - channel(blue)) / range).rem_euclid(6.0)
    } else if max == green {
        (channel(blue) - channel(red)) / range + 2.0
    } else {
        (channel(red) - channel(green)) / range + 4.0
    };
    sixths * 60.0
}

#[cfg(test)]
mod tests {
    use super::color_name;

    #[test]
    fn colors_are_named_as_nvda_names_them() {
        assert_eq!(color_name(0x0000_0000), "black");
        assert_eq!(color_name(0x00FF_FFFF), "white");
        assert_eq!(color_name(0x0080_8080), "grey");
        assert_eq!(color_name(0x0000_00FF), "bright red");
        assert_eq!(color_name(0x0000_0080), "dark red");
        assert_eq!(color_name(0x00FF_0000), "bright blue");
        assert_eq!(color_name(0x0000_80FF), "bright orange");
        assert_eq!(color_name(0x0000_4080), "brown");
    }
}
