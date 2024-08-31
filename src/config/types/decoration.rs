use colors_transform::{AlphaColor, Color, Hsl, Rgb};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use self::border::BorderConfig;
pub use self::color::ColorConfig;

const fn default_window_opacity() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecorationConfig {
    pub border: BorderConfig,

    #[serde(default = "default_window_opacity")]
    pub focused_window_opacity: f32,

    #[serde(default = "default_window_opacity")]
    pub normal_window_opacity: f32,

    #[serde(default)]
    pub allow_csd: bool,
}

impl Default for DecorationConfig {
    fn default() -> Self {
        Self {
            border: Default::default(),
            focused_window_opacity: default_window_opacity(),
            normal_window_opacity: default_window_opacity(),
            allow_csd: false,
        }
    }
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
        pub focused_color: ColorConfig,

        pub normal_color: ColorConfig,

        #[serde(default = "default_thickness")]
        pub thickness: u8,

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

    impl BorderConfig {
        pub fn radius(&self) -> f32 {
            self.radius - self.half_thickness()
        }

        pub fn half_thickness(&self) -> f32 {
            self.thickness as f32 / 2.0
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

    impl ColorConfig {
        pub fn components(&self) -> [f32; 4] {
            match self {
                Self::Solid(color) => *color,
                Self::Gradient { start, .. } => *start,
            }
        }
    }
}
