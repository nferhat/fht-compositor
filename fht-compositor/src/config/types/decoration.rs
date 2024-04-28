use colors_transform::{AlphaColor, Color, Hsl, Rgb};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use self::border::BorderConfig;
pub use self::color::ColorConfig;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct DecorationConfig {
    /// The configuration for the border around the windows.
    pub border: BorderConfig,

    /// Should we allow clients to draw their own decorations.
    ///
    /// Basically allow what is called CSD, or client side decorations.
    ///
    /// NOTE: If you set this to no, fht-compositor does NOT draw a set of builtin decorations.
    ///
    /// NOTE: When changing this setting, only newly created windows will react to it.
    ///
    /// WARN: Gnome apps (in Gnome fashion) don't give a fuck about this setting, since they are
    /// hardstuck on the idea that CSD is the superior option. Don't send issues about this.
    #[serde(default)]
    pub allow_csd: bool,
}

mod border {
    use super::*;

    const fn default_thickness() -> u8 {
        2
    }

    const fn default_radius() -> f32 {
        10.0
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct BorderConfig {
        /// The border color for the focused window.
        pub focused_color: ColorConfig,

        /// The border color for the non-focused window(s).
        pub normal_color: ColorConfig,

        /// The thickness of the border.
        #[serde(default = "default_thickness")]
        pub thickness: u8,

        /// The radius of the border.
        #[serde(default = "default_radius")]
        pub radius: f32,
    }

    impl Default for BorderConfig {
        fn default() -> Self {
            Self {
                focused_color: ColorConfig::Solid([1.0, 0.0, 0.0, 1.0]),
                normal_color: ColorConfig::Solid([0.5, 0.5, 0.5, 0.5]),
                thickness: 2,
                radius: 10.0,
            }
        }
    }
}

mod color_parser {
    use super::*;

    pub fn serialize<S: Serializer>(color: &[f32; 4], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(color)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f32; 4], D::Error> {
        // We don't internally expose the BorderConfig type, but you can use a valid css color
        // string.
        let color = String::deserialize(deserializer)?;

        if let Ok(rgb) = Rgb::from_hex_str(&color) {
            return Ok([
                rgb.get_red() / 255.0,
                rgb.get_green() / 255.0,
                rgb.get_blue() / 255.0,
                rgb.get_alpha(), // alpha is already normalized
            ]);
        }

        if let Ok(rgb) = color.trim().parse::<Rgb>() {
            return Ok([
                rgb.get_red() / 255.0,
                rgb.get_green() / 255.0,
                rgb.get_blue() / 255.0,
                rgb.get_alpha(), // alpha is already normalized
            ]);
        }

        if let Ok(hsl) = color.trim().parse::<Hsl>() {
            let rgb = hsl.to_rgb(); // this is lossy but eh
            return Ok([
                rgb.get_red() / 255.0,
                rgb.get_green() / 255.0,
                rgb.get_blue() / 255.0,
                rgb.get_alpha(), // alpha is already normalized
            ]);
        }

        Err(<D::Error as serde::de::Error>::invalid_value(
            serde::de::Unexpected::Str(&color),
            &"Invalid color input!",
        ))
    }
}

mod color {
    use super::*;

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
    pub enum ColorConfig {
        Solid(#[serde(with = "super::color_parser")] [f32; 4]),
        Gradient {
            #[serde(with = "super::color_parser")]
            start: [f32; 4],
            #[serde(with = "super::color_parser")]
            end: [f32; 4],
            angle: f32,
        },
    }
}
