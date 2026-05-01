/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 3 spatial tree — resolve transform chains for the GPU palette.
//!
//! Each spatial node carries a local transform relative to its parent.
//! `resolve_mat4` walks from the node to the root, accumulating
//! transforms into a single column-major `mat4x4<f32>` ready for GPU
//! upload. The root node (index 0) is always the identity transform.
//!
//! The tree is append-only; node indices are stable u32 IDs.

/// Stable index into a [`SpatialTree`].
pub type SpatialNodeId = u32;

/// The root spatial node. Always present, always identity.
pub const ROOT_SPATIAL_NODE: SpatialNodeId = 0;

/// Column-major 4×4 float matrix: `m[col][row]`.
type Mat4 = [[f32; 4]; 4];

fn identity() -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0], // col 0
        [0.0, 1.0, 0.0, 0.0], // col 1
        [0.0, 0.0, 1.0, 0.0], // col 2
        [0.0, 0.0, 0.0, 1.0], // col 3
    ]
}

fn mat4_translate(tx: f32, ty: f32) -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [tx,  ty,  0.0, 1.0],
    ]
}

fn mat4_rotate(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    [
        [ c,   s,  0.0, 0.0], // col 0: x-basis rotated
        [-s,   c,  0.0, 0.0], // col 1: y-basis rotated
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mat4_scale(sx: f32, sy: f32) -> Mat4 {
    [
        [sx,  0.0, 0.0, 0.0],
        [0.0, sy,  0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Column-major matrix multiplication: `a * b`.
/// Semantics: `b` is applied to a vector first, then `a`.
/// `result[col][row] = Σ_k a[k][row] * b[col][k]`
fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            for k in 0..4 {
                r[col][row] += a[k][row] * b[col][k];
            }
        }
    }
    r
}

/// Transform variant stored on a spatial node. Describes the node's
/// local-to-parent transform. All operations are in 2D device space
/// (the Z component passes through unchanged).
#[derive(Debug, Clone)]
pub enum SpatialTransform {
    Identity,
    Translate { x: f32, y: f32 },
    /// Counter-clockwise rotation, in radians. In screen space (y+ down)
    /// this appears as a clockwise rotation visually.
    Rotate { angle: f32 },
    Scale { x: f32, y: f32 },
}

impl SpatialTransform {
    fn to_mat4(&self) -> Mat4 {
        match self {
            SpatialTransform::Identity => identity(),
            SpatialTransform::Translate { x, y } => mat4_translate(*x, *y),
            SpatialTransform::Rotate { angle } => mat4_rotate(*angle),
            SpatialTransform::Scale { x, y } => mat4_scale(*x, *y),
        }
    }
}

struct SpatialNode {
    parent: SpatialNodeId,
    transform: SpatialTransform,
}

/// Append-only tree of spatial nodes. Each node holds a local transform
/// relative to its parent. [`resolve_mat4`][SpatialTree::resolve_mat4]
/// walks to the root and returns the accumulated local→world matrix.
pub struct SpatialTree {
    nodes: Vec<SpatialNode>,
}

impl SpatialTree {
    /// Create a tree with only the root node (identity, no parent).
    pub fn new() -> Self {
        Self {
            nodes: vec![SpatialNode {
                parent: ROOT_SPATIAL_NODE,
                transform: SpatialTransform::Identity,
            }],
        }
    }

    /// Append a child node. Returns its stable [`SpatialNodeId`].
    pub fn push_node(
        &mut self,
        parent: SpatialNodeId,
        transform: SpatialTransform,
    ) -> SpatialNodeId {
        let id = self.nodes.len() as SpatialNodeId;
        self.nodes.push(SpatialNode { parent, transform });
        id
    }

    /// Number of nodes (including the root at index 0).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Resolve node `id` to a column-major `mat4x4<f32>` representing
    /// the full local→world transform. The root node (id=0) returns
    /// identity. Walks from `id` to the root, multiplying transforms
    /// outermost-first: `root * ... * parent * local`.
    pub fn resolve_mat4(&self, id: SpatialNodeId) -> Mat4 {
        // Collect transforms from node→root.
        let mut stack = Vec::new();
        let mut cur = id;
        loop {
            let node = &self.nodes[cur as usize];
            stack.push(node.transform.to_mat4());
            if cur == ROOT_SPATIAL_NODE {
                break;
            }
            cur = node.parent;
        }
        // Multiply root→node so inner transforms apply first.
        // stack = [t_id, t_parent, …, t_root]; reverse = [t_root, …, t_id]
        let mut result = identity();
        for m in stack.iter().rev() {
            result = mat4_mul(&result, m);
        }
        result
    }
}

impl Default for SpatialTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn mat_approx_eq(a: &Mat4, b: &Mat4) -> bool {
        a.iter().zip(b.iter()).all(|(ca, cb)| {
            ca.iter().zip(cb.iter()).all(|(fa, fb)| approx_eq(*fa, *fb))
        })
    }

    #[test]
    fn root_is_identity() {
        let tree = SpatialTree::new();
        assert!(mat_approx_eq(&tree.resolve_mat4(ROOT_SPATIAL_NODE), &identity()));
    }

    #[test]
    fn single_translate() {
        let mut tree = SpatialTree::new();
        let n = tree.push_node(ROOT_SPATIAL_NODE, SpatialTransform::Translate { x: 10.0, y: 20.0 });
        let m = tree.resolve_mat4(n);
        // Translation is in the last column (col 3).
        assert!(approx_eq(m[3][0], 10.0));
        assert!(approx_eq(m[3][1], 20.0));
    }

    #[test]
    fn translate_chain() {
        let mut tree = SpatialTree::new();
        let n1 = tree.push_node(ROOT_SPATIAL_NODE, SpatialTransform::Translate { x: 10.0, y: 0.0 });
        let n2 = tree.push_node(n1, SpatialTransform::Translate { x: 5.0, y: 3.0 });
        let m = tree.resolve_mat4(n2);
        assert!(approx_eq(m[3][0], 15.0));
        assert!(approx_eq(m[3][1], 3.0));
    }

    #[test]
    fn rotate_90() {
        let mut tree = SpatialTree::new();
        let n = tree.push_node(ROOT_SPATIAL_NODE, SpatialTransform::Rotate { angle: PI / 2.0 });
        let m = tree.resolve_mat4(n);
        // (1, 0) → (0, 1); (0, 1) → (-1, 0)
        assert!(approx_eq(m[0][0],  0.0)); // x of x-basis
        assert!(approx_eq(m[0][1],  1.0)); // y of x-basis
        assert!(approx_eq(m[1][0], -1.0)); // x of y-basis
        assert!(approx_eq(m[1][1],  0.0)); // y of y-basis
    }
}
