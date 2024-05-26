use serde::de::Visitor;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize};

/// A single cubic control point.
pub type ControlPoint = (f64, f64);

/// How much points should we bake inside an animation?
pub const BAKED_POINTS: usize = 255;

/// Cubic bezier animation using two control points.
///
/// Adapted from Hyprland's cubic bezier curves:
/// - `src/helpers/BezierCurve.cpp`
/// - https://blog.maximeheckel.com/posts/cubic-bezier-from-math-to-motion/
#[derive(Debug, Clone, Copy)]
pub struct Animation {
    /// Our first control points for this curve.
    p1: ControlPoint,
    /// Our second control point for this curve.
    p2: ControlPoint,
    /// Baked animation points, basically precalculated values to speed up.
    baked_points: [ControlPoint; BAKED_POINTS],
    // In reality we don't have two control points but 4, one is (0,0) and the other is (1,1).
    // These are added implicitly to the curve to ensure that we go from these bounds.
}

impl Serialize for Animation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(4))?;

        seq.serialize_element(&self.p1.0)?;
        seq.serialize_element(&self.p2.0)?;
        seq.serialize_element(&self.p1.1)?;
        seq.serialize_element(&self.p2.1)?;

        seq.end()
    }
}

// Custom deserializer since we don't want to serialize baked points.
impl<'de> Deserialize<'de> for Animation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            P1,
            P2,
        }

        struct AnimationVisitor;
        impl<'de> Visitor<'de> for AnimationVisitor {
            type Value = Animation;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct Animation")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut p1 @ mut p2 = Option::<ControlPoint>::None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::P1 => {
                            if p1.is_some() {
                                return Err(serde::de::Error::duplicate_field("p1"));
                            }
                            p1 = Some(map.next_value()?);
                        }
                        Field::P2 => {
                            if p2.is_some() {
                                return Err(serde::de::Error::duplicate_field("p2"));
                            }
                            p2 = Some(map.next_value()?);
                        }
                    }
                }

                let p1 = p1.ok_or_else(|| serde::de::Error::missing_field("p1"))?;
                let p2 = p2.ok_or_else(|| serde::de::Error::missing_field("p2"))?;
                Ok(Animation::new(p1, p2))
            }
        }

        const FIELDS: &[&str] = &["p1", "p2"];
        deserializer.deserialize_struct("Animation", FIELDS, AnimationVisitor)
    }
}

impl Animation {
    pub fn new((x0, y0): ControlPoint, (x1, y1): ControlPoint) -> Self {
        let mut baked_points = [ControlPoint::default(); BAKED_POINTS];

        let get_x_for_t = |t: f64| {
            3.0 * t * (1.0 - t).powf(2.0) * x0 + 3.0 * t.powf(2.0) * (1.0 - t) * x1 + t.powf(3.0)
        };
        let get_y_for_t = |t: f64| {
            3.0 * t * (1.0 - t).powf(2.0) * y0 + 3.0 * t.powf(2.0) * (1.0 - t) * y1 + t.powf(3.0)
        };

        for i in 0..BAKED_POINTS {
            let t = (i + 1) as f64 / BAKED_POINTS as f64;
            baked_points[i] = (get_x_for_t(t), get_y_for_t(t));
        }

        Self {
            p1: (x0, y0),
            p2: (x1, y1),
            baked_points,
        }
    }

    /// Get the Y value at a given X coordinate, assuming that x is included in [0.0, 1.0]
    pub fn y(&self, x: f64) -> f64 {
        let mut index = 0;
        let mut below = true;

        let mut step = (BAKED_POINTS + 1) / 2;
        while step > 0 {
            if below {
                index += step;
            } else {
                index -= step;
            }

            below = self.baked_points[index].0 < x;
            step /= 2;
        }

        let lower_index = index.saturating_sub((!below || index == BAKED_POINTS - 1) as usize);
        let (x0, y0) = self.baked_points[lower_index];
        let (x1, y1) = self.baked_points[lower_index + 1];
        let delta = (x - x0) / (x1 - x0);

        if delta.is_nan() || delta.is_infinite() {
            0.0
        } else {
            y0 + (y1 - y0) * delta
        }
    }
}
