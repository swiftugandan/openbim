//! The BCF data model: topics with their comments and viewpoints.
//!
//! Strings are kept exactly as written in the file (never trimmed), except GUIDs, which are
//! lowercase. Dates are the lexical `xs:dateTime` text, so offsets survive a round trip.

/// A BCF topic (an issue): `<topic guid>/markup.bcf`.
#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    /// `Topic@Guid`, lowercase.
    pub guid: String,
    /// `Topic/Title`.
    pub title: String,
    /// `Topic@TopicType`.
    pub topic_type: String,
    /// `Topic@TopicStatus`.
    pub status: String,
    /// `Topic/CreationDate`, an `xs:dateTime` as written.
    pub creation_date: String,
    /// `Topic/CreationAuthor`.
    pub creation_author: String,
    /// `Topic/Description`; `None` when absent or blank.
    pub description: Option<String>,
    /// `Topic/Labels/Label`, in order.
    pub labels: Vec<String>,
    /// `Topic/Comments/Comment`, in order.
    pub comments: Vec<Comment>,
    /// `Topic/Viewpoints/ViewPoint`, in order, each with the contents of its `.bcfv` file.
    pub viewpoints: Vec<Viewpoint>,
}

/// A comment on a topic.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    /// `Comment@Guid`, lowercase.
    pub guid: String,
    /// `Comment/Date`, an `xs:dateTime` as written.
    pub date: String,
    /// `Comment/Author`.
    pub author: String,
    /// `Comment/Comment`; empty when the comment has no text.
    pub text: String,
    /// `Comment/Viewpoint@Guid`: the [`Viewpoint::guid`] of the viewpoint the comment is about.
    pub viewpoint: Option<String>,
}

/// A viewpoint: a `ViewPoint` of the markup and the `VisualizationInfo` file it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewpoint {
    /// `ViewPoint@Guid`, lowercase.
    pub guid: String,
    /// The `IfcGuid` of every selected component (`Components/Selection/Component`), in order.
    pub components: Vec<String>,
    /// The camera, if the file has a usable one. Writing a viewpoint without one uses
    /// [`Camera::default_view`], since the schema demands a camera.
    pub camera: Option<Camera>,
}

/// How a camera projects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// `PerspectiveCamera`: the vertical field of view in degrees, in (0, 180).
    Perspective {
        /// `FieldOfView`.
        field_of_view: f64,
    },
    /// `OrthogonalCamera`: the visible vertical size in metres.
    Orthogonal {
        /// `ViewToWorldScale`.
        view_to_world_scale: f64,
    },
}

/// A viewpoint camera (metres, the model's right-handed z-up frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Perspective or orthogonal, with the parameter each needs.
    pub projection: Projection,
    /// `CameraViewPoint`: the eye position.
    pub view_point: [f64; 3],
    /// `CameraDirection`: where the camera looks.
    pub direction: [f64; 3],
    /// `CameraUpVector`.
    pub up: [f64; 3],
    /// `AspectRatio`: width over height, positive.
    pub aspect_ratio: f64,
}

/// Vertical field of view of [`Camera::default_view`] and [`Camera::fit`], in degrees.
const DEFAULT_FIELD_OF_VIEW: f64 = 60.0;

fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 0.0 && length.is_finite() {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        v
    }
}

impl Camera {
    /// Looking at the origin from (10, -10, 10): a perspective camera with a 60 degree field of
    /// view and a 16:9 aspect ratio, z up.
    pub fn default_view() -> Camera {
        let direction = normalized([-1.0, 1.0, -1.0]);
        // The world's up (z) made perpendicular to the direction.
        let along = direction[2];
        let up = normalized([-along * direction[0], -along * direction[1], 1.0 - along * direction[2]]);
        Camera {
            projection: Projection::Perspective { field_of_view: DEFAULT_FIELD_OF_VIEW },
            view_point: [10.0, -10.0, 10.0],
            direction,
            up,
            aspect_ratio: 16.0 / 9.0,
        }
    }

    /// The default direction and field of view, backed off along the view direction until the box
    /// from `min` to `max` (at least a 1 m radius around its centre) fills the view. The default
    /// view for a box that is not finite.
    pub fn fit(min: [f64; 3], max: [f64; 3]) -> Camera {
        let finite = min.iter().chain(max.iter()).all(|v| v.is_finite());
        if !finite {
            return Camera::default_view();
        }
        let centre = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
        let extent = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let radius = (0.5 * (extent[0] * extent[0] + extent[1] * extent[1] + extent[2] * extent[2]).sqrt()).max(1.0);
        let base = Camera::default_view();
        let distance = radius / (DEFAULT_FIELD_OF_VIEW / 2.0).to_radians().sin() * 1.1;
        let view_point = [
            centre[0] - base.direction[0] * distance,
            centre[1] - base.direction[1] * distance,
            centre[2] - base.direction[2] * distance,
        ];
        let camera = Camera { view_point, ..base };
        if camera.is_valid() {
            camera
        } else {
            Camera::default_view()
        }
    }

    /// Whether the camera is something the schema accepts and the document can store: every number
    /// finite, a positive aspect ratio and a field of view strictly between 0 and 180 degrees.
    pub fn is_valid(&self) -> bool {
        let numbers_ok = self.view_point.iter().chain(&self.direction).chain(&self.up).all(|v| v.is_finite());
        let projection_ok = match self.projection {
            Projection::Perspective { field_of_view } => field_of_view.is_finite() && field_of_view > 0.0 && field_of_view < 180.0,
            Projection::Orthogonal { view_to_world_scale } => view_to_world_scale.is_finite(),
        };
        numbers_ok && projection_ok && self.aspect_ratio.is_finite() && self.aspect_ratio > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length(v: [f64; 3]) -> f64 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    #[test]
    fn the_default_view_is_valid_with_orthonormal_vectors() {
        let c = Camera::default_view();
        assert!(c.is_valid());
        assert!((length(c.direction) - 1.0).abs() < 1e-12);
        assert!((length(c.up) - 1.0).abs() < 1e-12);
        let dot = c.direction[0] * c.up[0] + c.direction[1] * c.up[1] + c.direction[2] * c.up[2];
        assert!(dot.abs() < 1e-12, "{dot}");
        assert!(c.up[2] > 0.0, "z is up");
    }

    #[test]
    fn fit_looks_at_the_centre_from_a_distance_that_grows_with_the_box() {
        let small = Camera::fit([0.0; 3], [1.0, 1.0, 1.0]);
        let large = Camera::fit([0.0; 3], [100.0, 100.0, 10.0]);
        for c in [small, large] {
            assert!(c.is_valid());
        }
        let eye_to_centre = |c: Camera, centre: [f64; 3]| length([c.view_point[0] - centre[0], c.view_point[1] - centre[1], c.view_point[2] - centre[2]]);
        assert!(eye_to_centre(large, [50.0, 50.0, 5.0]) > 10.0 * eye_to_centre(small, [0.5, 0.5, 0.5]));
        // The centre lies on the viewing ray.
        let c = small;
        let to_centre = [0.5 - c.view_point[0], 0.5 - c.view_point[1], 0.5 - c.view_point[2]];
        let cos = (to_centre[0] * c.direction[0] + to_centre[1] * c.direction[1] + to_centre[2] * c.direction[2]) / length(to_centre);
        assert!((cos - 1.0).abs() < 1e-9, "{cos}");
    }

    #[test]
    fn fit_falls_back_to_the_default_for_a_box_that_is_not_finite() {
        for (min, max) in [([f64::NAN; 3], [1.0; 3]), ([0.0; 3], [f64::INFINITY; 3]), ([f64::NEG_INFINITY; 3], [0.0; 3])] {
            assert_eq!(Camera::fit(min, max), Camera::default_view());
        }
        assert!(Camera::fit([1e308; 3], [-1e308; 3]).is_valid(), "overflowing arithmetic still gives a usable camera");
    }

    #[test]
    fn validity_covers_every_number() {
        let good = Camera::default_view();
        assert!(good.is_valid());
        for bad in [
            Camera { aspect_ratio: 0.0, ..good },
            Camera { aspect_ratio: -1.0, ..good },
            Camera { aspect_ratio: f64::NAN, ..good },
            Camera { view_point: [0.0, f64::INFINITY, 0.0], ..good },
            Camera { direction: [f64::NAN, 0.0, 0.0], ..good },
            Camera { up: [0.0, 0.0, f64::NEG_INFINITY], ..good },
            Camera { projection: Projection::Perspective { field_of_view: 0.0 }, ..good },
            Camera { projection: Projection::Perspective { field_of_view: 180.0 }, ..good },
            Camera { projection: Projection::Perspective { field_of_view: f64::NAN }, ..good },
            Camera { projection: Projection::Orthogonal { view_to_world_scale: f64::INFINITY }, ..good },
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
        assert!(Camera { projection: Projection::Orthogonal { view_to_world_scale: 12.5 }, ..good }.is_valid());
    }
}
