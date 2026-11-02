//! Plain value types: 2D points, 3D points and 3D vectors over `f64`.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A point in the plane.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

/// A point in space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A direction / displacement in space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Point2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl Point3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// The origin.
    pub const ORIGIN: Point3 = Point3::new(0.0, 0.0, 0.0);

    /// The position vector of this point.
    pub fn to_vec(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

impl Vec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    pub const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    pub const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn norm2(self) -> f64 {
        self.dot(self)
    }

    pub fn norm(self) -> f64 {
        self.norm2().sqrt()
    }

    /// Unit vector in the same direction, or `None` when the length is zero (or not finite).
    pub fn normalized(self) -> Option<Vec3> {
        let n = self.norm();
        if n > 0.0 && n.is_finite() {
            Some(self / n)
        } else {
            None
        }
    }

    /// Interpret this vector as a point.
    pub fn to_point(self) -> Point3 {
        Point3::new(self.x, self.y, self.z)
    }
}

impl Add<Vec3> for Point3 {
    type Output = Point3;
    fn add(self, v: Vec3) -> Point3 {
        Point3::new(self.x + v.x, self.y + v.y, self.z + v.z)
    }
}

impl AddAssign<Vec3> for Point3 {
    fn add_assign(&mut self, v: Vec3) {
        *self = *self + v;
    }
}

impl Sub<Vec3> for Point3 {
    type Output = Point3;
    fn sub(self, v: Vec3) -> Point3 {
        Point3::new(self.x - v.x, self.y - v.y, self.z - v.z)
    }
}

impl SubAssign<Vec3> for Point3 {
    fn sub_assign(&mut self, v: Vec3) {
        *self = *self - v;
    }
}

impl Sub<Point3> for Point3 {
    type Output = Vec3;
    fn sub(self, o: Point3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl SubAssign for Vec3 {
    fn sub_assign(&mut self, o: Vec3) {
        *self = *self - o;
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Mul<Vec3> for f64 {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        v * self
    }
}

impl Div<f64> for Vec3 {
    type Output = Vec3;
    fn div(self, s: f64) -> Vec3 {
        Vec3::new(self.x / s, self.y / s, self.z / s)
    }
}

impl Sub for Point2 {
    type Output = Point2;
    /// Component-wise difference (a displacement, kept as a `Point2` for simplicity).
    fn sub(self, o: Point2) -> Point2 {
        Point2::new(self.x - o.x, self.y - o.y)
    }
}
