use csscolorparser::Color as CssColor;
use ls_types::{Color, ColorPresentation, Range, TextEdit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NormalizedColorKey {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

/// Parse a CSS color value and return an LSP Color
pub fn parse_color(value: &str) -> Option<Color> {
    parse_csscolorparser(value.trim())
}

pub fn normalized_color_key(value: &str) -> Option<NormalizedColorKey> {
    parse_color(value).map(normalize_color)
}

pub fn normalize_color(color: Color) -> NormalizedColorKey {
    NormalizedColorKey {
        red: color_channel_to_u8(color.red),
        green: color_channel_to_u8(color.green),
        blue: color_channel_to_u8(color.blue),
        alpha: color_channel_to_u8(color.alpha),
    }
}

pub fn color_from_key(key: NormalizedColorKey) -> Color {
    Color {
        red: key.red as f32 / 255.0,
        green: key.green as f32 / 255.0,
        blue: key.blue as f32 / 255.0,
        alpha: key.alpha as f32 / 255.0,
    }
}

fn strip_ascii_suffix_case_insensitive<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    let start = value.len().checked_sub(suffix.len())?;
    value
        .get(start..)
        .filter(|tail| tail.eq_ignore_ascii_case(suffix))
        .map(|_| &value[..start])
}

fn contains_nonfinite_numeric_token(value: &str) -> bool {
    for raw_token in value.split(|character: char| {
        character == '('
            || character == ')'
            || character == ','
            || character == '/'
            || character.is_ascii_whitespace()
    }) {
        let token = raw_token.trim();
        if token.is_empty() {
            continue;
        }

        // CSS color channels can carry a percentage or angle unit. Strip the
        // unit before checking the number so values such as `infdeg` and
        // `NaN%` cannot be accepted after the dependency clamps them.
        let numeric = strip_ascii_suffix_case_insensitive(token, "%")
            .or_else(|| strip_ascii_suffix_case_insensitive(token, "deg"))
            .or_else(|| strip_ascii_suffix_case_insensitive(token, "grad"))
            .or_else(|| strip_ascii_suffix_case_insensitive(token, "rad"))
            .or_else(|| strip_ascii_suffix_case_insensitive(token, "turn"))
            .unwrap_or(token);

        let unsigned = numeric
            .strip_prefix('+')
            .or_else(|| numeric.strip_prefix('-'))
            .unwrap_or(numeric);
        if unsigned.eq_ignore_ascii_case("nan")
            || unsigned.eq_ignore_ascii_case("inf")
            || unsigned.eq_ignore_ascii_case("infinity")
        {
            return true;
        }

        if let Ok(number) = numeric.parse::<f64>() {
            if !number.is_finite() {
                return true;
            }
        }
    }

    false
}

fn parse_csscolorparser(value: &str) -> Option<Color> {
    if contains_nonfinite_numeric_token(value) {
        return None;
    }

    let parsed: CssColor = value.parse().ok()?;

    // csscolorparser accepts any value that `f64::parse` accepts for numeric
    // channels, including `NaN` and infinities. It also leaves HWB alpha
    // outside the nominal range untouched. Never let either case cross the
    // LSP boundary: JSON has no representation for non-finite numbers and
    // LSP Color channels are defined in the inclusive [0, 1] range.
    let channels = [parsed.r, parsed.g, parsed.b, parsed.a];
    if channels.iter().any(|channel| !channel.is_finite()) {
        return None;
    }

    Some(Color {
        red: parsed.r.clamp(0.0, 1.0) as f32,
        green: parsed.g.clamp(0.0, 1.0) as f32,
        blue: parsed.b.clamp(0.0, 1.0) as f32,
        alpha: parsed.a.clamp(0.0, 1.0) as f32,
    })
}

/// Generate color presentations for color picker
pub fn generate_color_presentations(color: Color, range: Range) -> Vec<ColorPresentation> {
    let mut presentations = Vec::new();

    let hex_str = format_color_as_hex(color);
    presentations.push(ColorPresentation {
        label: hex_str.clone(),
        text_edit: Some(TextEdit {
            range,
            new_text: hex_str,
        }),
        additional_text_edits: None,
    });

    let rgb_str = format_color_as_rgb(color);
    presentations.push(ColorPresentation {
        label: rgb_str.clone(),
        text_edit: Some(TextEdit {
            range,
            new_text: rgb_str,
        }),
        additional_text_edits: None,
    });

    let hsl_str = format_color_as_hsl(color);
    presentations.push(ColorPresentation {
        label: hsl_str.clone(),
        text_edit: Some(TextEdit {
            range,
            new_text: hsl_str,
        }),
        additional_text_edits: None,
    });

    presentations
}

pub fn format_color_as_hex(color: Color) -> String {
    let r = (color.red.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (color.green.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (color.blue.clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = (color.alpha.clamp(0.0, 1.0) * 255.0).round() as u8;

    if a == 255 {
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a)
    }
}

pub fn format_color_as_rgb(color: Color) -> String {
    let r = (color.red.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (color.green.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (color.blue.clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = color.alpha.clamp(0.0, 1.0);

    if a >= 1.0 {
        format!("rgb({}, {}, {})", r, g, b)
    } else {
        format!("rgba({}, {}, {}, {:.2})", r, g, b, a)
    }
}

pub fn format_color_as_hsl(color: Color) -> String {
    let (h, s, l) = rgb_to_hsl(color.red, color.green, color.blue);
    let a = color.alpha.clamp(0.0, 1.0);

    let h_deg = (h * 360.0).round();
    let s_pct = (s * 100.0).round();
    let l_pct = (l * 100.0).round();

    if a >= 1.0 {
        format!("hsl({}, {}%, {}%)", h_deg, s_pct, l_pct)
    } else {
        format!("hsla({}, {}%, {}%, {:.2})", h_deg, s_pct, l_pct, a)
    }
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if max == min {
        return (0.0, 0.0, l);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if max == r {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };

    (h, s, l)
}

fn color_channel_to_u8(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use ls_types::Position;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn parse_color_hex_and_named() {
        let color = parse_color("#abc").expect("hex");
        assert!(approx_eq(color.red, 0xAA as f32 / 255.0));
        assert!(approx_eq(color.green, 0xBB as f32 / 255.0));
        assert!(approx_eq(color.blue, 0xCC as f32 / 255.0));
        assert!(approx_eq(color.alpha, 1.0));

        let color = parse_color("#abcd").expect("hex with alpha");
        assert!(approx_eq(color.red, 0xAA as f32 / 255.0));
        assert!(approx_eq(color.alpha, 0xDD as f32 / 255.0));

        let color = parse_color("blue").expect("named");
        assert!(approx_eq(color.blue, 1.0));
        assert!(approx_eq(color.red, 0.0));
    }

    #[test]
    fn parse_color_rgb_variants() {
        let color = parse_color("rgb(255, 0, 128)").expect("rgb");
        assert!(approx_eq(color.red, 1.0));
        assert!(approx_eq(color.green, 0.0));
        assert!(approx_eq(color.blue, 128.0 / 255.0));

        let color = parse_color("rgba(255, 0, 0, 0.5)").expect("rgba");
        assert!(approx_eq(color.red, 1.0));
        assert!(approx_eq(color.alpha, 0.5));

        let color = parse_color("rgb(100%, 0%, 50%)").expect("rgb percent");
        assert!(approx_eq(color.red, 1.0));
        assert!(approx_eq(color.blue, 0.5));

        let color = parse_color("rgba(255, 0, 0, 50%)").expect("rgba percent");
        assert!(approx_eq(color.alpha, 0.5));
    }

    #[test]
    fn normalized_color_key_matches_equivalent_inputs() {
        let white = normalized_color_key("white").expect("white");
        assert_eq!(white, normalized_color_key("#fff").expect("#fff"));
        assert_eq!(white, normalized_color_key("#ffffff").expect("#ffffff"));
        assert_eq!(
            white,
            normalized_color_key("rgb(255 255 255)").expect("rgb")
        );
        assert_eq!(white, normalized_color_key("hsl(0 0% 100%)").expect("hsl"));
    }

    #[test]
    fn normalized_color_key_preserves_alpha() {
        let translucent = normalized_color_key("rgba(255, 255, 255, 0.5)").expect("rgba");
        assert_eq!(
            translucent,
            normalized_color_key("#ffffff80").expect("hex alpha")
        );
        assert_ne!(translucent, normalized_color_key("white").expect("white"));
    }

    #[test]
    fn generate_color_presentations_formats_output() {
        let range = Range::new(Position::new(0, 0), Position::new(0, 4));
        let color = Color {
            red: 1.0,
            green: 0.0,
            blue: 0.5,
            alpha: 1.0,
        };
        let presentations = generate_color_presentations(color, range);
        assert_eq!(presentations.len(), 3);
        assert!(presentations[0].label.starts_with('#'));
        assert!(presentations[1].label.starts_with("rgb("));
        assert!(presentations[2].label.starts_with("hsl("));

        let color = Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.5,
        };
        let presentations = generate_color_presentations(color, range);
        assert_eq!(presentations.len(), 3);
        assert!(presentations[0].label.starts_with('#'));
        assert!(presentations[1].label.starts_with("rgba("));
        assert!(presentations[2].label.starts_with("hsla("));
    }

    #[test]
    fn format_color_hex_opaque_and_transparent() {
        // Opaque color (alpha = 255)
        let color = Color {
            red: 0.0,
            green: 0.5,
            blue: 1.0,
            alpha: 1.0,
        };
        let hex = format_color_as_hex(color);
        assert_eq!(hex, "#0080ff");

        // Transparent color (alpha < 255)
        let color = Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.5,
        };
        let hex = format_color_as_hex(color);
        assert_eq!(hex, "#ff000080");
    }

    #[test]
    fn format_color_rgb_with_alpha() {
        let color = Color {
            red: 0.5,
            green: 0.5,
            blue: 0.5,
            alpha: 1.0,
        };
        let rgb = format_color_as_rgb(color);
        assert_eq!(rgb, "rgb(128, 128, 128)");

        let color = Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.75,
        };
        let rgba = format_color_as_rgb(color);
        assert_eq!(rgba, "rgba(255, 0, 0, 0.75)");
    }

    #[test]
    fn format_color_hsl_with_alpha() {
        let color = Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        };
        let hsl = format_color_as_hsl(color);
        assert!(hsl.starts_with("hsl("));
        assert!(hsl.contains("0,") || hsl.contains("360,")); // Red hue

        let color = Color {
            red: 0.0,
            green: 0.5,
            blue: 1.0,
            alpha: 0.5,
        };
        let hsla = format_color_as_hsl(color);
        assert!(hsla.starts_with("hsla("));
        assert!(hsla.contains("0.50"));
    }

    #[test]
    fn rgb_to_hsl_conversion() {
        // Pure red
        let (h, s, l) = rgb_to_hsl(1.0, 0.0, 0.0);
        assert!(approx_eq(h, 0.0));
        assert!(approx_eq(s, 1.0));
        assert!(approx_eq(l, 0.5));

        // Pure green
        let (h, s, l) = rgb_to_hsl(0.0, 1.0, 0.0);
        assert!(approx_eq(h, 1.0 / 3.0));
        assert!(approx_eq(s, 1.0));
        assert!(approx_eq(l, 0.5));

        // Gray (no saturation)
        let (_h, s, l) = rgb_to_hsl(0.5, 0.5, 0.5);
        assert!(approx_eq(s, 0.0));
        assert!(approx_eq(l, 0.5));
    }

    #[test]
    fn parse_color_edge_cases() {
        // Invalid colors should return None
        assert!(parse_color("not-a-color").is_none());
        assert!(parse_color("").is_none());
        assert!(parse_color("rgb(999, 999, 999)").is_some()); // Clamped by parser

        // Named colors
        assert!(parse_color("rebeccapurple").is_some());
        assert!(parse_color("aliceblue").is_some());

        // Transparent keyword
        let color = parse_color("transparent").expect("transparent");
        assert!(approx_eq(color.alpha, 0.0));
    }

    #[test]
    fn parse_color_rejects_nonfinite_channels_and_clamps_alpha() {
        for value in [
            "#gggggg",
            "rgb(NaN, 0, 0)",
            "rgb(inf, 0, 0)",
            "rgb(infinity, 0, 0)",
            "rgb(1e999, 0, 0)",
            "hsl(0, NaN%, 50%)",
            "hsl(infdeg, 50%, 50%)",
            "hsl(infDEG, 50%, 50%)",
            "hsl(1e999TURN, 50%, 50%)",
            "hwb(0 0% 0% / NaN)",
            "hwb(0 0% 0% / InFiNiTy)",
        ] {
            assert!(parse_color(value).is_none(), "{value}");
        }

        let color = parse_color("hwb(0 0% 0% / 2)").expect("finite HWB color");
        assert!(approx_eq(color.alpha, 1.0));
        for channel in [color.red, color.green, color.blue, color.alpha] {
            assert!(channel.is_finite());
            assert!((0.0..=1.0).contains(&channel));
        }
    }

    #[test]
    fn color_clamping() {
        // Test that colors are properly clamped to [0, 1]
        let color = Color {
            red: 1.5,
            green: -0.5,
            blue: 0.5,
            alpha: 2.0,
        };

        let hex = format_color_as_hex(color);
        assert!(hex.starts_with('#'));

        let rgb = format_color_as_rgb(color);
        assert!(rgb.contains("255")); // Red clamped to max
        assert!(rgb.contains("0")); // Green clamped to min
    }
}
