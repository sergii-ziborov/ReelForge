//! Serializable keyframed parameters (JSON / MCP safe — no closures).

use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};

/// Interpolation curve between keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    /// Step hold until next key.
    Hold,
    /// Linear blend (default).
    #[default]
    Linear,
    /// Smoothstep ease in-out.
    Smooth,
}

/// One keyframe at exact media time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe<T> {
    /// Sample time.
    pub t: MediaTime,
    /// Value at `t`.
    pub value: T,
    /// Outgoing easing toward the next key (ignored for last).
    #[serde(default)]
    pub easing: Easing,
}

impl<T> Keyframe<T> {
    /// Construct a linear keyframe.
    #[must_use]
    pub fn new(t: MediaTime, value: T) -> Self {
        Self {
            t,
            value,
            easing: Easing::Linear,
        }
    }
}

/// Constant or keyframed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Animated<T> {
    /// Unchanging value.
    Constant {
        /// Value.
        value: T,
    },
    /// Time-varying keys (must be sorted by `t` for evaluation).
    Keyframes {
        /// Ordered samples.
        keys: Vec<Keyframe<T>>,
    },
}

impl<T: Clone> Animated<T> {
    /// Constant helper.
    #[must_use]
    pub fn constant(value: T) -> Self {
        Self::Constant { value }
    }

    /// Keyframed helper.
    #[must_use]
    pub fn keyframes(keys: Vec<Keyframe<T>>) -> Self {
        Self::Keyframes { keys }
    }
}

impl Animated<f32> {
    /// Evaluate at media time `t` (seconds path via `as_secs`).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn sample_f32(&self, t: MediaTime) -> f32 {
        match self {
            Self::Constant { value } => *value,
            Self::Keyframes { keys } if keys.is_empty() => 0.0,
            Self::Keyframes { keys } if keys.len() == 1 => keys[0].value,
            Self::Keyframes { keys } => {
                let ts = t.as_secs();
                if ts <= keys[0].t.as_secs() {
                    return keys[0].value;
                }
                let last = keys.last().unwrap();
                if ts >= last.t.as_secs() {
                    return last.value;
                }
                for w in keys.windows(2) {
                    let a = &w[0];
                    let b = &w[1];
                    let ta = a.t.as_secs();
                    let tb = b.t.as_secs();
                    if ts >= ta && ts <= tb {
                        let span = (tb - ta).max(1e-12);
                        #[allow(clippy::cast_possible_truncation)]
                        let mut u = ((ts - ta) / span) as f32;
                        u = match a.easing {
                            Easing::Hold => 0.0,
                            Easing::Linear => u,
                            Easing::Smooth => u * u * (3.0 - 2.0 * u),
                        };
                        return a.value + (b.value - a.value) * u;
                    }
                }
                last.value
            }
        }
    }
}

impl Animated<(f32, f32)> {
    /// Evaluate 2D position.
    #[must_use]
    pub fn sample_xy(&self, t: MediaTime) -> (f32, f32) {
        match self {
            Self::Constant { value } => *value,
            Self::Keyframes { keys } if keys.is_empty() => (0.0, 0.0),
            Self::Keyframes { keys } if keys.len() == 1 => keys[0].value,
            Self::Keyframes { keys } => {
                let x = Animated::Keyframes {
                    keys: keys
                        .iter()
                        .map(|k| Keyframe {
                            t: k.t,
                            value: k.value.0,
                            easing: k.easing,
                        })
                        .collect(),
                }
                .sample_f32(t);
                let y = Animated::Keyframes {
                    keys: keys
                        .iter()
                        .map(|k| Keyframe {
                            t: k.t,
                            value: k.value.1,
                            easing: k.easing,
                        })
                        .collect(),
                }
                .sample_f32(t);
                (x, y)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_midpoint() {
        let a = Animated::keyframes(vec![
            Keyframe::new(MediaTime::new(0, 10).unwrap(), 0.0),
            Keyframe::new(MediaTime::new(10, 10).unwrap(), 10.0),
        ]);
        let v = a.sample_f32(MediaTime::new(5, 10).unwrap());
        assert!((v - 5.0).abs() < 1e-4);
    }
}
