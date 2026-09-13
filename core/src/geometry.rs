//! Unit-bearing geometry at the egui/native child-view boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EguiRect {
    min: [f32; 2],
    max: [f32; 2],
}
impl EguiRect {
    pub fn new(min: [f32; 2], max: [f32; 2]) -> Option<Self> {
        (min.into_iter().chain(max).all(f32::is_finite) && min[0] <= max[0] && min[1] <= max[1])
            .then_some(Self { min, max })
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeRect {
    min: [f32; 2],
    max: [f32; 2],
}
impl NativeRect {
    pub fn left(self) -> f32 {
        self.min[0]
    }
    pub fn top(self) -> f32 {
        self.min[1]
    }
    pub fn right(self) -> f32 {
        self.max[0]
    }
    pub fn bottom(self) -> f32 {
        self.max[1]
    }
    pub fn width(self) -> f32 {
        self.max[0] - self.min[0]
    }
    pub fn height(self) -> f32 {
        self.max[1] - self.min[1]
    }
}
pub struct ViewportTransform {
    origin: [f32; 2],
    scale: f32,
}
impl ViewportTransform {
    pub fn new(origin: [f32; 2], scale: f32) -> Option<Self> {
        (origin.into_iter().all(f32::is_finite) && scale.is_finite() && scale > 0.0)
            .then_some(Self { origin, scale })
    }
    /// A native rectangle cannot accidentally be converted a second time.
    /// ```compile_fail
    /// use tiptoptyp_core::geometry::{EguiRect, ViewportTransform};
    /// let transform = ViewportTransform::new([0.0; 2], 2.0).unwrap();
    /// let native = transform.to_native(EguiRect::new([0.0; 2], [10.0; 2]).unwrap()).unwrap();
    /// transform.to_native(native);
    /// ```
    pub fn to_native(&self, rect: EguiRect) -> Option<NativeRect> {
        let convert = |point: [f32; 2]| {
            std::array::from_fn(|i| self.origin[i] + (point[i] - self.origin[i]) * self.scale)
        };
        let min = convert(rect.min);
        let max = convert(rect.max);
        EguiRect::new(min, max).map(|rect| NativeRect {
            min: rect.min,
            max: rect.max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_positive_transforms_preserve_origin_and_containment() {
        for origin in [[0.0, 0.0], [-100.0, 20.0], [500.0, 400.0]] {
            for scale in [0.5, 1.0, 1.15, 2.0, 3.0] {
                let transform = ViewportTransform::new(origin, scale).unwrap();
                let rect = transform
                    .to_native(
                        EguiRect::new(origin, [origin[0] + 100.0, origin[1] + 50.0]).unwrap(),
                    )
                    .unwrap();
                assert_eq!([rect.left(), rect.top()], origin);
                assert!((rect.width() - 100.0 * scale).abs() < 0.001);
                assert!((rect.height() - 50.0 * scale).abs() < 0.001);
            }
        }
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(ViewportTransform::new([0.0; 2], scale).is_none());
        }
        assert!(EguiRect::new([10.0; 2], [0.0; 2]).is_none());
    }
}
