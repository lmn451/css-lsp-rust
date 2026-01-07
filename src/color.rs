use csscolorparser::Color as CssColor;
use tower_lsp::lsp_types::{Color, ColorPresentation, Range, TextEdit};

/// Parse a CSS color value and return an LSP Color (simplified version)
pub fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    let lower = value.to_lowercase();

    // Try hex color
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex);
    }

    // Try rgb/rgba
    if lower.starts_with("rgb") {
        return parse_rgb(&lower);
    }

    // Try named colors (basic set)
    parse_named_color(&lower).or_else(|| parse_csscolorparser(value))
}

fn parse_csscolorparser(value: &str) -> Option<Color> {
    let parsed: CssColor = value.parse().ok()?;
    Some(Color {
        red: parsed.r as f32,
        green: parsed.g as f32,
        blue: parsed.b as f32,
        alpha: parsed.a as f32,
    })
}

fn parse_hex(hex: &str) -> Option<Color> {
    let hex = hex.trim();
    let len = hex.len();

    if len == 3 {
        // #RGB
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
        Some(Color {
            red: r as f32 / 255.0,
            green: g as f32 / 255.0,
            blue: b as f32 / 255.0,
            alpha: 1.0,
        })
    } else if len == 4 {
        // #RGBA
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
        let a = u8::from_str_radix(&hex[3..4].repeat(2), 16).ok()?;
        Some(Color {
            red: r as f32 / 255.0,
            green: g as f32 / 255.0,
            blue: b as f32 / 255.0,
            alpha: a as f32 / 255.0,
        })
    } else if len == 6 {
        // #RRGGBB
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Color {
            red: r as f32 / 255.0,
            green: g as f32 / 255.0,
            blue: b as f32 / 255.0,
            alpha: 1.0,
        })
    } else if len == 8 {
        // #RRGGBBAA
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
        Some(Color {
            red: r as f32 / 255.0,
            green: g as f32 / 255.0,
            blue: b as f32 / 255.0,
            alpha: a as f32 / 255.0,
        })
    } else {
        None
    }
}

fn parse_rgb(value: &str) -> Option<Color> {
    let inner = if let Some(rest) = value.strip_prefix("rgba") {
        rest
    } else if let Some(rest) = value.strip_prefix("rgb") {
        rest
    } else {
        return None;
    };

    let inner = inner.trim_start().strip_prefix('(')?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();

    if parts.len() < 3 {
        return None;
    }

    let parse_channel = |part: &str| -> Option<f32> {
        if let Some(pct) = part.strip_suffix('%') {
            let value = pct.trim().parse::<f32>().ok()?;
            return Some((value / 100.0).clamp(0.0, 1.0));
        }
        let value = part.parse::<f32>().ok()?;
        if value > 1.0 {
            Some((value / 255.0).clamp(0.0, 1.0))
        } else {
            Some(value.clamp(0.0, 1.0))
        }
    };

    let parse_alpha = |part: &str| -> Option<f32> {
        if let Some(pct) = part.strip_suffix('%') {
            let value = pct.trim().parse::<f32>().ok()?;
            return Some((value / 100.0).clamp(0.0, 1.0));
        }
        let value = part.parse::<f32>().ok()?;
        if value > 1.0 {
            Some((value / 255.0).clamp(0.0, 1.0))
        } else {
            Some(value.clamp(0.0, 1.0))
        }
    };

    let r = parse_channel(parts[0])?;
    let g = parse_channel(parts[1])?;
    let b = parse_channel(parts[2])?;
    let a = if parts.len() > 3 {
        parse_alpha(parts[3])?
    } else {
        1.0
    };

    Some(Color {
        red: r,
        green: g,
        blue: b,
        alpha: a,
    })
}

fn parse_named_color(name: &str) -> Option<Color> {
    // Basic named colors
    match name.to_lowercase().as_str() {
        "red" => Some(Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        }),
        "green" => Some(Color {
            red: 0.0,
            green: 0.5,
            blue: 0.0,
            alpha: 1.0,
        }),
        "blue" => Some(Color {
            red: 0.0,
            green: 0.0,
            blue: 1.0,
            alpha: 1.0,
        }),
        "white" => Some(Color {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        }),
        "black" => Some(Color {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        }),
        "yellow" => Some(Color {
            red: 1.0,
            green: 1.0,
            blue: 0.0,
            alpha: 1.0,
        }),
        "cyan" => Some(Color {
            red: 0.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        }),
        "magenta" => Some(Color {
            red: 1.0,
            green: 0.0,
            blue: 1.0,
            alpha: 1.0,
        }),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

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
    fn parse_color_csscolorparser_fallback() {
        let color = parse_color("hsl(0, 100%, 50%)").expect("csscolorparser");
        assert!(approx_eq(color.red, 1.0));
        assert!(approx_eq(color.green, 0.0));
        assert!(approx_eq(color.blue, 0.0));
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
        
        // Transparent keyword
        let color = parse_color("transparent").expect("transparent");
        assert!(approx_eq(color.alpha, 0.0));
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
