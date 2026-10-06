//! Serializable keyframed parameters (JSON / MCP safe — no closures).

use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};

/// Why a keyframe curve cannot be sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveFault {
    /// No keys were stored.
    Empty,
    /// Times do not increase. The curve is not sorted into place.
    NotStrictlyIncreasing,
}

impl std::fmt::Display for CurveFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("empty"),
            Self::NotStrictlyIncreasing => formatter.write_str("not strictly increasing"),
        }
    }
}

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

    /// Empty and out-of-order keys are refused. They are not sorted and not
    /// replaced with zero.
    #[must_use]
    pub fn curve_fault(&self) -> Option<CurveFault> {
        let Self::Keyframes { keys } = self else {
            return None;
        };
        if keys.is_empty() {
            return Some(CurveFault::Empty);
        }
        let mut previous = None;
        for key in keys {
            if let Some(earlier) = previous
                && !strictly_after(earlier, key.t)
            {
                return Some(CurveFault::NotStrictlyIncreasing);
            }
            previous = Some(key.t);
        }
        None
    }
}

/// `next` is later than `previous` on the rational media clock.
fn strictly_after(previous: MediaTime, next: MediaTime) -> bool {
    let left = i128::from(next.ticks) * i128::from(previous.timescale.max(1));
    let right = i128::from(previous.ticks) * i128::from(next.timescale.max(1));
    left > right
}

impl Animated<f32> {
    /// Sample a valid curve. An empty or unsorted curve returns the fault.
    ///
    /// # Errors
    ///
    /// [`CurveFault::Empty`] or [`CurveFault::NotStrictlyIncreasing`].
    pub fn try_sample_f32(&self, t: MediaTime) -> Result<f32, CurveFault> {
        if let Some(fault) = self.curve_fault() {
            return Err(fault);
        }
        Ok(self.sample_sorted_f32(t))
    }

    /// Evaluate at media time `t`.
    ///
    /// An empty or unsorted curve is [`f32::NAN`], not `0.0`.
    #[must_use]
    pub fn sample_f32(&self, t: MediaTime) -> f32 {
        self.try_sample_f32(t).unwrap_or(f32::NAN)
    }

    #[allow(clippy::cast_precision_loss)]
    fn sample_sorted_f32(&self, t: MediaTime) -> f32 {
        match self {
            Self::Constant { value } => *value,
            Self::Keyframes { keys } if keys.is_empty() => f32::NAN,
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
    /// Sample a valid position. An empty or unsorted curve returns the fault.
    ///
    /// # Errors
    ///
    /// [`CurveFault::Empty`] or [`CurveFault::NotStrictlyIncreasing`].
    pub fn try_sample_xy(&self, t: MediaTime) -> Result<(f32, f32), CurveFault> {
        if let Some(fault) = self.curve_fault() {
            return Err(fault);
        }
        Ok(self.sample_sorted_xy(t))
    }

    /// Evaluate a 2D position. An invalid curve is `(NaN, NaN)`, not the origin.
    #[must_use]
    pub fn sample_xy(&self, t: MediaTime) -> (f32, f32) {
        self.try_sample_xy(t).unwrap_or((f32::NAN, f32::NAN))
    }

    fn sample_sorted_xy(&self, t: MediaTime) -> (f32, f32) {
        match self {
            Self::Constant { value } => *value,
            Self::Keyframes { keys } if keys.is_empty() => (f32::NAN, f32::NAN),
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

    #[test]
    fn empty_and_unsorted_keys_are_not_zero() {
        let empty = Animated::<f32>::keyframes(vec![]);
        assert_eq!(empty.curve_fault(), Some(CurveFault::Empty));
        assert!(empty.sample_f32(MediaTime::new(0, 1).unwrap()).is_nan());
        let reversed = Animated::keyframes(vec![
            Keyframe::new(MediaTime::new(1, 1).unwrap(), 10.0),
            Keyframe::new(MediaTime::new(0, 1).unwrap(), 0.0),
        ]);
        assert_eq!(
            reversed.curve_fault(),
            Some(CurveFault::NotStrictlyIncreasing)
        );
        let sampled = reversed.sample_f32(MediaTime::new(0, 1).unwrap());
        assert!(sampled.is_nan());
        let xy = Animated::keyframes(vec![Keyframe::new(
            MediaTime::new(0, 1).unwrap(),
            (0.0, 0.0),
        )])
        .sample_xy(MediaTime::new(0, 1).unwrap());
        assert_eq!(xy, (0.0, 0.0));
    }
}
