/// A closed axis-aligned box `[min, max]`. Boxes that merely touch (share a face, an
/// edge or a corner) intersect. Zero-extent (degenerate) boxes are allowed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    /// The empty box (`min = +inf`, `max = -inf`); it intersects nothing and is the
    /// identity of [`Aabb::union`].
    pub const EMPTY: Aabb = Aabb { min: [f64::INFINITY; 3], max: [f64::NEG_INFINITY; 3] };

    #[inline]
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb { min, max }
    }

    /// The degenerate box containing exactly one point.
    #[inline]
    pub fn from_point(p: [f64; 3]) -> Aabb {
        Aabb { min: p, max: p }
    }

    /// Smallest box containing every point (empty box for no points).
    pub fn from_points(points: &[[f64; 3]]) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in points {
            b.add_point(*p);
        }
        b
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        !(self.min[0] <= self.max[0] && self.min[1] <= self.max[1] && self.min[2] <= self.max[2])
    }

    #[inline]
    pub fn add_point(&mut self, p: [f64; 3]) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(p[k]);
            self.max[k] = self.max[k].max(p[k]);
        }
    }

    /// Closed intersection test: touching counts.
    #[inline]
    pub fn intersects(&self, o: &Aabb) -> bool {
        self.min[0] <= o.max[0]
            && o.min[0] <= self.max[0]
            && self.min[1] <= o.max[1]
            && o.min[1] <= self.max[1]
            && self.min[2] <= o.max[2]
            && o.min[2] <= self.max[2]
    }

    /// Whether `o` lies entirely inside `self` (closed).
    #[inline]
    pub fn contains(&self, o: &Aabb) -> bool {
        self.min[0] <= o.min[0]
            && self.min[1] <= o.min[1]
            && self.min[2] <= o.min[2]
            && o.max[0] <= self.max[0]
            && o.max[1] <= self.max[1]
            && o.max[2] <= self.max[2]
    }

    #[inline]
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb {
            min: [self.min[0].min(o.min[0]), self.min[1].min(o.min[1]), self.min[2].min(o.min[2])],
            max: [self.max[0].max(o.max[0]), self.max[1].max(o.max[1]), self.max[2].max(o.max[2])],
        }
    }

    /// Volume (0 for empty or degenerate boxes).
    #[inline]
    pub fn volume(&self) -> f64 {
        let dx = self.max[0] - self.min[0];
        let dy = self.max[1] - self.min[1];
        let dz = self.max[2] - self.min[2];
        if dx < 0.0 || dy < 0.0 || dz < 0.0 {
            0.0
        } else {
            dx * dy * dz
        }
    }

    /// Sum of the edge lengths along the three axes (the R*-tree "margin", up to a
    /// constant factor of 4). 0 for empty boxes.
    #[inline]
    pub fn margin(&self) -> f64 {
        let dx = self.max[0] - self.min[0];
        let dy = self.max[1] - self.min[1];
        let dz = self.max[2] - self.min[2];
        if dx < 0.0 || dy < 0.0 || dz < 0.0 {
            0.0
        } else {
            dx + dy + dz
        }
    }

    /// Surface area (0 for empty boxes).
    #[inline]
    pub fn surface_area(&self) -> f64 {
        let dx = self.max[0] - self.min[0];
        let dy = self.max[1] - self.min[1];
        let dz = self.max[2] - self.min[2];
        if dx < 0.0 || dy < 0.0 || dz < 0.0 {
            0.0
        } else {
            2.0 * (dx * dy + dy * dz + dz * dx)
        }
    }

    /// Volume of the intersection (0 when disjoint or merely touching).
    #[inline]
    pub fn overlap_volume(&self, o: &Aabb) -> f64 {
        let mut v = 1.0;
        for k in 0..3 {
            let d = self.max[k].min(o.max[k]) - self.min[k].max(o.min[k]);
            if d <= 0.0 {
                return 0.0;
            }
            v *= d;
        }
        v
    }

    #[inline]
    pub fn center(&self) -> [f64; 3] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
            0.5 * (self.min[2] + self.max[2]),
        ]
    }

    /// Squared Euclidean distance from `p` to the closest point of the box (0 inside).
    #[inline]
    pub fn distance2_to_point(&self, p: [f64; 3]) -> f64 {
        let mut s = 0.0;
        for k in 0..3 {
            let d = (self.min[k] - p[k]).max(p[k] - self.max[k]).max(0.0);
            s += d * d;
        }
        s
    }

    /// Euclidean distance from `p` to the box (0 inside or on the boundary).
    #[inline]
    pub fn distance_to_point(&self, p: [f64; 3]) -> f64 {
        self.distance2_to_point(p).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_intersection_and_distance() {
        let a = Aabb::new([0.0; 3], [1.0; 3]);
        let b = Aabb::new([1.0; 3], [2.0; 3]);
        assert!(a.intersects(&b) && b.intersects(&a), "corner touch intersects");
        assert_eq!(a.overlap_volume(&b), 0.0);
        let p = Aabb::from_point([1.0, 0.5, 0.0]);
        assert!(a.intersects(&p) && a.contains(&p));
        assert!(!Aabb::EMPTY.intersects(&a) && Aabb::EMPTY.is_empty());
        assert_eq!(a.distance_to_point([0.5, 0.5, 0.5]), 0.0);
        assert_eq!(a.distance_to_point([4.0, 5.0, 1.0]), 5.0);
        assert_eq!(Aabb::EMPTY.union(&a), a);
    }
}
