//! Shared directions for independent blur and transparency falloff.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FadeDirection {
    #[default]
    None,
    Left,
    Right,
    Up,
    Down,
}

impl FadeDirection {
    pub(crate) const ALL: [Self; 5] = [Self::None, Self::Left, Self::Right, Self::Up, Self::Down];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "Uniform",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
        }
    }

    pub(crate) fn interpolate(self, start: f32, end: f32, x: f32, y: f32) -> f32 {
        if self == Self::None {
            return start;
        }
        let factor = self.factor_at(x, y);
        start * factor + end * (1.0 - factor)
    }

    /// Start-value contribution in normalized, top-left-origin coordinates.
    /// The chosen edge uses the end value; its opposite uses the start value.
    pub(crate) fn factor_at(self, x: f32, y: f32) -> f32 {
        match self {
            Self::None => 1.0,
            Self::Left => x.clamp(0.0, 1.0),
            Self::Right => 1.0 - x.clamp(0.0, 1.0),
            Self::Up => y.clamp(0.0, 1.0),
            Self::Down => 1.0 - y.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chosen_edge_has_zero_strength_with_top_left_coordinates() {
        for (direction, expected) in [
            (FadeDirection::None, [1.0, 1.0, 1.0, 1.0]),
            (FadeDirection::Left, [0.0, 1.0, 1.0, 0.0]),
            (FadeDirection::Right, [1.0, 0.0, 0.0, 1.0]),
            (FadeDirection::Up, [0.0, 0.0, 1.0, 1.0]),
            (FadeDirection::Down, [1.0, 1.0, 0.0, 0.0]),
        ] {
            let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                .map(|(x, y)| direction.factor_at(x, y));
            assert_eq!(corners, expected, "{direction:?}");
            assert_eq!(
                direction.factor_at(0.5, 0.5),
                if direction == FadeDirection::None {
                    1.0
                } else {
                    0.5
                }
            );
        }
    }

    #[test]
    fn strength_is_linear_clamped_and_independent_of_the_other_axis() {
        for step in 0..=10 {
            let point = step as f32 / 10.0;
            for other in [0.0, 0.3, 1.0] {
                assert_eq!(FadeDirection::Left.factor_at(point, other), point);
                assert_eq!(FadeDirection::Right.factor_at(point, other), 1.0 - point);
                assert_eq!(FadeDirection::Up.factor_at(other, point), point);
                assert_eq!(FadeDirection::Down.factor_at(other, point), 1.0 - point);
            }
        }
        assert_eq!(FadeDirection::Left.factor_at(-1.0, 0.0), 0.0);
        assert_eq!(FadeDirection::Right.factor_at(2.0, 0.0), 0.0);
        assert_eq!(FadeDirection::Up.factor_at(0.0, -1.0), 0.0);
        assert_eq!(FadeDirection::Down.factor_at(0.0, 2.0), 0.0);
    }

    #[test]
    fn interpolation_preserves_arbitrary_equal_and_reverse_endpoints() {
        for (start, end) in [
            (8.0, 0.0),
            (0.0, 30.0),
            (20.0, 20.0),
            (80.0, 40.0),
            (25.0, 100.0),
        ] {
            for direction in FadeDirection::ALL {
                for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                    let expected = if direction.factor_at(x, y) == 1.0 {
                        start
                    } else {
                        end
                    };
                    assert_eq!(direction.interpolate(start, end, x, y), expected);
                }
                assert_eq!(
                    direction.interpolate(start, end, 0.5, 0.5),
                    if direction == FadeDirection::None {
                        start
                    } else {
                        (start + end) / 2.0
                    }
                );
            }
        }
    }

    #[test]
    fn directions_round_trip_as_closed_snake_case_options() {
        assert_eq!(FadeDirection::default(), FadeDirection::None);
        for (direction, value) in FadeDirection::ALL
            .into_iter()
            .zip(["none", "left", "right", "up", "down"])
        {
            assert_eq!(serde_json::to_value(direction).unwrap(), value);
            assert_eq!(
                serde_json::from_value::<FadeDirection>(value.into()).unwrap(),
                direction
            );
        }
        for value in ["uniform", "diagonal", "Right", "", "top"] {
            assert!(serde_json::from_value::<FadeDirection>(value.into()).is_err());
        }
    }
}
