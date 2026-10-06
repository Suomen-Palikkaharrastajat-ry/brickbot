//! The LDraw file AST.
//!
//! LDraw files are line-oriented; the first token of each line is its type.
//! We keep every line we can act on and discard nothing silently, so that
//! header metadata (notably `!LICENSE`) survives parsing.

#![allow(clippy::doc_markdown, clippy::many_single_char_names)]

use crate::math::{Mat4, Vec3};

/// The special LDraw colour code meaning "inherit from the caller".
pub const COLOR_INHERIT: u16 = 16;
/// The special LDraw colour code meaning "the complement/edge colour".
pub const COLOR_EDGE: u16 = 24;

/// A reference to another LDraw file, placed by a transform.
#[derive(Debug, Clone, PartialEq)]
pub struct SubfileRef {
    pub color: u16,
    pub transform: Mat4,
    /// The referenced file name, normalised to lowercase with `\` turned into `/`.
    pub file: String,
}

/// One parsed line of an LDraw file.
#[derive(Debug, Clone, PartialEq)]
pub enum LdrawLine {
    /// A type-0 line: a comment or a meta command. The payload is the text
    /// after the leading `0`, trimmed.
    Meta(String),
    /// A type-1 line: a sub-file reference.
    Subfile(SubfileRef),
    /// A type-2 line: an edge. Kept so callers can choose to draw outlines.
    Edge { color: u16, a: Vec3, b: Vec3 },
    /// A type-3 line: a triangle.
    Triangle {
        color: u16,
        a: Vec3,
        b: Vec3,
        c: Vec3,
    },
    /// A type-4 line: a quadrilateral, wound `a-b-c-d`.
    Quad {
        color: u16,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        d: Vec3,
    },
    /// A type-5 line: a conditional edge. Retained but not rendered.
    OptionalEdge { color: u16, a: Vec3, b: Vec3 },
}

impl LdrawLine {
    /// The sub-file this line references, if any.
    #[must_use]
    pub const fn subfile(&self) -> Option<&SubfileRef> {
        match self {
            Self::Subfile(r) => Some(r),
            _ => None,
        }
    }
}

/// Parsed LDraw files keyed by their normalised name (see
/// [`super::parser::normalize_name`]).
///
/// The cache is filled by [`super::resolve`] and consumed by
/// [`super::geometry`], which keeps flattening a pure, synchronous operation.
pub type PartCache = std::collections::HashMap<String, std::sync::Arc<Vec<LdrawLine>>>;

/// A flattened, transform-resolved mesh in a single coordinate space.
///
/// Vertices are stored in renderer space (+Y up), i.e. the LDraw Y axis has
/// already been negated. Colours are still LDraw colour codes; [`COLOR_INHERIT`]
/// means "use the colour of the placement that instantiated this mesh".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub triangles: Vec<MeshTriangle>,
}

/// A single flattened triangle with its resolved colour code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshTriangle {
    pub color: u16,
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
}

impl Mesh {
    #[must_use]
    pub const fn len(&self) -> usize {
        self.triangles.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// The bounding box of every vertex in the mesh.
    #[must_use]
    pub fn bounds(&self) -> crate::math::Aabb {
        self.triangles
            .iter()
            .fold(crate::math::Aabb::EMPTY, |b, t| {
                b.expand(t.a).expand(t.b).expand(t.c)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subfile_accessor_only_matches_type_one_lines() {
        let sub = LdrawLine::Subfile(SubfileRef {
            color: COLOR_INHERIT,
            transform: Mat4::IDENTITY,
            file: "3001.dat".to_string(),
        });
        assert_eq!(sub.subfile().map(|r| r.file.as_str()), Some("3001.dat"));
        assert!(
            LdrawLine::Meta("BFC CERTIFY CCW".to_string())
                .subfile()
                .is_none()
        );
    }

    #[test]
    fn mesh_bounds_cover_every_vertex() {
        let mesh = Mesh {
            triangles: vec![MeshTriangle {
                color: 4,
                a: Vec3::new(0.0, 0.0, 0.0),
                b: Vec3::new(2.0, 0.0, 0.0),
                c: Vec3::new(0.0, 3.0, 0.0),
            }],
        };
        assert_eq!(mesh.len(), 1);
        assert!(!mesh.is_empty());
        let b = mesh.bounds();
        assert_eq!(b.min, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(b.max, Vec3::new(2.0, 3.0, 0.0));
    }

    #[test]
    fn empty_mesh_has_empty_bounds() {
        assert!(Mesh::default().bounds().is_empty());
        assert!(Mesh::default().is_empty());
    }
}
