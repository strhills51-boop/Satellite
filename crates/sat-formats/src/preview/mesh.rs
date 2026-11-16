//! Mesh geometry for the 3D view: shapes of a NIF placed in the model's space, with UVs and the diffuse texture. @sat:formats.preview.mesh
//!
//! Owns: walking the scene graph from the root (`NiNode` transforms), shapes of Skyrim LE / SE and Fallout 4 —
//! `BSTriShape`, `BSSubIndexTriShape`, `BSDynamicTriShape`, `BSMeshLODTriShape`, `NiTriShape` + `NiTriShapeData`,
//! `NiTriStrips` + `NiTriStripsData`; the diffuse texture of `BSLightingShaderProperty` → `BSShaderTextureSet`
//! (or the source texture of `BSEffectShaderProperty`); vertices, UVs and triangles packed as base64 little-endian
//! arrays. Skinned shapes are shown in their bind pose.
//! Not here: the header and block table (`@sat:formats.preview.nif`), drawing (the viewer of the explorer).
//!
//! Every block is read inside its declared size: a block this reader does not know is skipped, a block that does
//! not parse is dropped with a note, never a panic.

use serde::{Deserialize, Serialize};

use super::Cur;
use super::nif::Layout;

/// Vertices at most over all shapes (a bigger mesh is cut, with a note).
pub const MESH_VERTICES: usize = 1 << 20;

/// One drawable shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    pub name: String,
    pub vertices: u32,
    pub triangles: u32,
    /// Base64 of `f32` x, y, z per vertex, in the model's space.
    pub positions: String,
    /// Base64 of `f32` u, v per vertex; empty when the shape has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uvs: String,
    /// Base64 of `u32` vertex indices, three per triangle.
    pub indices: String,
    /// The diffuse texture, a data path (`textures/…`), when the shader names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// A material file (Fallout 4 `.bgsm`) instead of a texture set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    #[serde(default)]
    pub skinned: bool,
    /// Uses alpha (has an `NiAlphaProperty`).
    #[serde(default)]
    pub alpha: bool,
}

/// The shapes of a mesh and their bounds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub shapes: Vec<Shape>,
    /// min x, y, z, max x, y, z.
    pub bounds: [f32; 6],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A rigid transform: rotation (row-major 3×3), translation, uniform scale.
#[derive(Clone, Copy, Debug)]
struct Xf {
    r: [f32; 9],
    t: [f32; 3],
    s: f32,
}

impl Xf {
    const ID: Xf = Xf {
        r: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        t: [0.; 3],
        s: 1.,
    };
    fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.r;
        [
            self.s * (r[0] * v[0] + r[1] * v[1] + r[2] * v[2]) + self.t[0],
            self.s * (r[3] * v[0] + r[4] * v[1] + r[5] * v[2]) + self.t[1],
            self.s * (r[6] * v[0] + r[7] * v[1] + r[8] * v[2]) + self.t[2],
        ]
    }
    /// `self` (parent) then `c` (child).
    fn then(&self, c: &Xf) -> Xf {
        let (a, b) = (&self.r, &c.r);
        let mut r = [0f32; 9];
        for i in 0..3 {
            for j in 0..3 {
                r[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
            }
        }
        Xf {
            r,
            t: self.apply(c.t),
            s: self.s * c.s,
        }
    }
}

/// IEEE half → f32.
pub(crate) fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = (h >> 10) & 0x1f;
    let m = (h & 0x3ff) as f32;
    match e {
        0 => s * m * 2f32.powi(-24),
        31 => s * f32::INFINITY,
        _ => s * (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

/// Positions, UVs, triangles.
type Geo = (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[u32; 3]>);

/// What a block is, read only as far as the 3D view needs.
#[derive(Default, Clone)]
struct Block {
    name: String,
    xf: Option<Xf>,
    children: Vec<i32>,
    /// A shape: its geometry (positions, uvs, triangles), data block, shader, alpha, skin.
    geo: Option<Geo>,
    data: i32,
    shader: i32,
    alpha: i32,
    skin: i32,
    /// A texture set: its first texture; an effect shader: its source texture; a lighting shader: its set.
    texture: Option<String>,
    texture_set: i32,
    material: Option<String>,
}

struct R<'a> {
    c: Cur<'a>,
    l: &'a Layout,
}

impl R<'_> {
    fn f32(&mut self) -> Option<f32> {
        self.c.u32().map(f32::from_bits)
    }
    fn i32(&mut self) -> Option<i32> {
        self.c.u32().map(|v| v as i32)
    }
    fn v3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
    fn string(&mut self) -> Option<String> {
        let i = self.c.u32()?;
        Some(if i == u32::MAX {
            String::new()
        } else {
            self.l.strings.get(i as usize).cloned().unwrap_or_default()
        })
    }
    fn sized(&mut self) -> Option<String> {
        let n = self.c.u32()? as usize;
        if n > 4096 {
            return None;
        }
        self.c.text(n)
    }
    fn refs(&mut self) -> Option<Vec<i32>> {
        let n = self.c.u32()? as usize;
        if n > 1 << 16 {
            return None;
        }
        (0..n).map(|_| self.i32()).collect()
    }
    /// NiObjectNET: the name (and the lighting shader's type before it).
    fn net(&mut self, lighting: bool) -> Option<String> {
        if lighting && self.l.bs < 155 {
            self.c.u32()?;
        }
        let name = self.string()?;
        self.refs()?;
        self.i32()?;
        Some(name)
    }
    /// NiAVObject after NiObjectNET.
    fn av(&mut self) -> Option<Xf> {
        if self.l.bs > 26 {
            self.c.u32()?;
        } else {
            self.c.u16()?;
        }
        let t = self.v3()?;
        let mut r = [0f32; 9];
        for x in &mut r {
            *x = self.f32()?;
        }
        let s = self.f32()?;
        if self.l.bs <= 34 {
            self.refs()?;
        }
        self.i32()?;
        Some(Xf { r, t, s })
    }
}

/// Packed vertices (`BSVertexData` / `BSVertexDataSSE`): positions and the first UVs.
#[allow(clippy::type_complexity)]
fn vertices(data: &[u8], desc: u64, bs: u32) -> Option<(Vec<[f32; 3]>, Vec<[f32; 2]>)> {
    let stride = (desc & 0xf) as usize * 4;
    if stride == 0 {
        return None;
    }
    let attrs = (desc >> 44) as u32;
    let uv_at = ((desc >> 8) & 0xf) as usize * 4;
    let full = bs == 100 || attrs & 0x400 != 0;
    let n = data.len() / stride;
    let mut pos = Vec::with_capacity(n);
    let mut uv = Vec::new();
    for v in data.chunks_exact(stride) {
        let mut c = Cur::new(v);
        if attrs & 1 != 0 {
            pos.push(if full {
                [
                    f32::from_bits(c.u32()?),
                    f32::from_bits(c.u32()?),
                    f32::from_bits(c.u32()?),
                ]
            } else {
                [half(c.u16()?), half(c.u16()?), half(c.u16()?)]
            });
        }
        if attrs & 2 != 0 {
            c.at = uv_at;
            uv.push([half(c.u16()?), half(c.u16()?)]);
        }
    }
    Some((pos, uv))
}

fn read_block(l: &Layout, ty: &str, bytes: &[u8]) -> Option<Block> {
    let mut r = R {
        c: Cur::new(bytes),
        l,
    };
    let mut b = Block {
        data: -1,
        shader: -1,
        alpha: -1,
        skin: -1,
        texture_set: -1,
        ..Default::default()
    };
    match ty {
        "NiNode"
        | "BSFadeNode"
        | "BSLeafAnimNode"
        | "BSTreeNode"
        | "BSMultiBoundNode"
        | "BSOrderedNode"
        | "BSValueNode"
        | "BSDamageStage"
        | "BSBlastNode"
        | "BSDebrisNode"
        | "NiBillboardNode"
        | "NiSwitchNode"
        | "NiLODNode"
        | "BSMasterParticleSystem" => {
            b.name = r.net(false)?;
            b.xf = Some(r.av()?);
            b.children = r.refs()?;
        }
        "BSTriShape" | "BSSubIndexTriShape" | "BSDynamicTriShape" | "BSMeshLODTriShape" => {
            b.name = r.net(false)?;
            b.xf = Some(r.av()?);
            r.v3()?;
            r.f32()?;
            if l.bs >= 155 {
                r.c.skip(24)?;
            }
            b.skin = r.i32()?;
            b.shader = r.i32()?;
            b.alpha = r.i32()?;
            let desc = r.c.u64()?;
            let tris = if l.bs >= 130 {
                r.c.u32()? as usize
            } else {
                r.c.u16()? as usize
            };
            let n = r.c.u16()? as usize;
            let size = r.c.u32()? as usize;
            let stride = (desc & 0xf) as usize * 4;
            let mut pos = Vec::new();
            let mut uv = Vec::new();
            let mut ix = Vec::with_capacity(tris);
            if size > 0 && stride > 0 {
                let data = r.c.take(stride.checked_mul(n)?)?;
                (pos, uv) = vertices(data, desc, l.bs)?;
                for _ in 0..tris {
                    ix.push([r.c.u16()? as u32, r.c.u16()? as u32, r.c.u16()? as u32]);
                }
            }
            if l.bs == 100 {
                // SSE: the particle data (meshes without vertex data keep their vertices there)
                let psize = r.c.u32()? as usize;
                if psize > 0 {
                    let mut pv = Vec::with_capacity(n);
                    for _ in 0..n {
                        pv.push([half(r.c.u16()?), half(r.c.u16()?), half(r.c.u16()?)]);
                    }
                    r.c.skip(n * 6)?;
                    let mut pi = Vec::with_capacity(tris);
                    for _ in 0..tris {
                        pi.push([r.c.u16()? as u32, r.c.u16()? as u32, r.c.u16()? as u32]);
                    }
                    if pos.is_empty() {
                        pos = pv;
                        ix = pi;
                    }
                }
            }
            if ty == "BSDynamicTriShape" && l.bs == 100 {
                // SSE: the dynamic vertices (facegen heads)
                let dn = r.c.u32()? as usize / 16;
                if dn == n {
                    pos.clear();
                    for _ in 0..dn {
                        let p = r.v3()?;
                        r.f32()?;
                        pos.push(p);
                    }
                }
            }
            b.geo = Some((pos, uv, ix));
        }
        "NiTriShape" | "NiTriStrips" | "BSLODTriShape" | "BSSegmentedTriShape" => {
            b.name = r.net(false)?;
            b.xf = Some(r.av()?);
            b.data = r.i32()?;
            b.skin = r.i32()?;
            let nm = r.c.u32()? as usize;
            r.c.skip(nm.checked_mul(8)?)?;
            r.i32()?;
            r.c.u8()?;
            if l.bs > 34 {
                b.shader = r.i32()?;
                b.alpha = r.i32()?;
            }
            b.geo = Some((Vec::new(), Vec::new(), Vec::new()));
        }
        "NiTriShapeData" | "NiTriStripsData" => {
            r.i32()?;
            let n = r.c.u16()? as usize;
            r.c.u8()?;
            r.c.u8()?;
            let has_v = r.c.u8()? != 0;
            let mut pos = Vec::new();
            if has_v {
                for _ in 0..n {
                    pos.push(r.v3()?);
                }
            }
            let flags = r.c.u16()? as u32;
            if l.bs > 34 {
                r.c.u32()?;
            }
            let has_n = r.c.u8()? != 0;
            if has_n {
                r.c.skip(n * 12)?;
                if flags & 4096 != 0 {
                    r.c.skip(n * 24)?;
                }
            }
            r.v3()?;
            r.f32()?;
            if r.c.u8()? != 0 {
                r.c.skip(n * 16)?;
            }
            let sets = if l.bs > 0 { flags & 1 } else { flags & 63 };
            let mut uv = Vec::new();
            for k in 0..sets {
                for _ in 0..n {
                    let t = [r.f32()?, r.f32()?];
                    if k == 0 {
                        uv.push(t);
                    }
                }
            }
            r.c.u16()?;
            r.i32()?;
            let tris = r.c.u16()? as usize;
            let mut ix = Vec::new();
            if ty == "NiTriShapeData" {
                r.c.u32()?;
                if r.c.u8()? != 0 {
                    for _ in 0..tris {
                        ix.push([r.c.u16()? as u32, r.c.u16()? as u32, r.c.u16()? as u32]);
                    }
                }
            } else {
                let ns = r.c.u16()? as usize;
                let lens: Vec<usize> = (0..ns)
                    .map(|_| r.c.u16().map(|x| x as usize))
                    .collect::<Option<_>>()?;
                if r.c.u8()? != 0 {
                    for len in lens {
                        let pts: Vec<u32> = (0..len)
                            .map(|_| r.c.u16().map(u32::from))
                            .collect::<Option<_>>()?;
                        for k in 2..pts.len() {
                            let (a, b2, c) = (pts[k - 2], pts[k - 1], pts[k]);
                            if a == b2 || b2 == c || a == c {
                                continue;
                            }
                            ix.push(if k % 2 == 0 { [a, b2, c] } else { [a, c, b2] });
                        }
                    }
                }
            }
            b.geo = Some((pos, uv, ix));
        }
        "BSLightingShaderProperty" => {
            b.name = r.net(true)?;
            if l.bs >= 130 && b.name.to_ascii_lowercase().ends_with(".bgsm") {
                b.material = Some(b.name.clone());
            }
            if l.bs >= 130 {
                r.c.skip(8)?;
                if l.bs >= 132 {
                    let n1 = r.c.u32()? as usize;
                    let n2 = if l.bs >= 152 { r.c.u32()? as usize } else { 0 };
                    r.c.skip((n1 + n2) * 4)?;
                }
            } else {
                r.c.skip(8)?;
            }
            r.c.skip(16)?;
            b.texture_set = r.i32()?;
        }
        "BSEffectShaderProperty" => {
            b.name = r.net(false)?;
            if l.bs >= 130 && b.name.to_ascii_lowercase().ends_with(".bgem") {
                b.material = Some(b.name.clone());
            }
            r.c.skip(8)?;
            if l.bs >= 132 {
                let n1 = r.c.u32()? as usize;
                let n2 = if l.bs >= 152 { r.c.u32()? as usize } else { 0 };
                r.c.skip((n1 + n2) * 4)?;
            }
            r.c.skip(16)?;
            let t = r.sized()?;
            if !t.is_empty() {
                b.texture = Some(t);
            }
        }
        "NiSkinInstance" | "BSDismemberSkinInstance" => {
            r.i32()?;
            b.data = r.i32()?;
        }
        "NiSkinPartition" if l.bs == 100 => {
            // SSE keeps a skinned shape's vertices here, with the triangles of each partition (global indices)
            let parts = r.c.u32()? as usize;
            let size = r.c.u32()? as usize;
            r.c.u32()?;
            let desc = r.c.u64()?;
            let (pos, uv) = if size > 0 {
                vertices(r.c.take(size)?, desc, l.bs)?
            } else {
                (Vec::new(), Vec::new())
            };
            let mut ix = Vec::new();
            for _ in 0..parts.min(4096) {
                let nv = r.c.u16()? as usize;
                let nt = r.c.u16()? as usize;
                let nb = r.c.u16()? as usize;
                let ns = r.c.u16()? as usize;
                let nw = r.c.u16()? as usize;
                r.c.skip(nb * 2)?;
                if r.c.u8()? != 0 {
                    r.c.skip(nv * 2)?;
                }
                if r.c.u8()? != 0 {
                    r.c.skip(nv * nw * 4)?;
                }
                let lens: Vec<usize> = (0..ns)
                    .map(|_| r.c.u16().map(usize::from))
                    .collect::<Option<_>>()?;
                if r.c.u8()? != 0 {
                    if ns > 0 {
                        r.c.skip(lens.iter().sum::<usize>() * 2)?;
                    } else {
                        r.c.skip(nt * 6)?;
                    }
                }
                if r.c.u8()? != 0 {
                    r.c.skip(nv * nw)?;
                }
                r.c.skip(2)?;
                r.c.u64()?;
                for _ in 0..nt {
                    ix.push([r.c.u16()? as u32, r.c.u16()? as u32, r.c.u16()? as u32]);
                }
            }
            b.geo = Some((pos, uv, ix));
        }
        "BSShaderTextureSet" => {
            let n = r.c.u32()?;
            if n > 0 {
                let t = r.sized()?;
                if !t.is_empty() {
                    b.texture = Some(t);
                }
            }
        }
        _ => return None,
    }
    Some(b)
}

fn b64_f32(v: impl Iterator<Item = f32>) -> String {
    let bytes: Vec<u8> = v.flat_map(f32::to_le_bytes).collect();
    super::base64(&bytes)
}

/// The geometry of a NIF, when it has drawable shapes.
pub(super) fn read(b: &[u8], l: &Layout) -> Option<Mesh> {
    if l.sizes.len() != l.idx.len() || l.idx.is_empty() {
        return None;
    }
    let mut blocks: Vec<Option<Block>> = Vec::with_capacity(l.idx.len());
    let mut at = l.data_at;
    let mut failed = 0usize;
    for (k, &ti) in l.idx.iter().enumerate() {
        let size = l.sizes[k];
        let end = at.checked_add(size)?;
        let bytes = b.get(at..end)?;
        let ty = l.types.get(ti).map(String::as_str).unwrap_or("");
        let blk = read_block(l, ty, bytes);
        if blk.is_none()
            && matches!(
                ty,
                "BSTriShape" | "BSSubIndexTriShape" | "NiTriShape" | "NiTriShapeData" | "NiTriStripsData"
            )
        {
            failed += 1;
        }
        blocks.push(blk);
        at = end;
    }
    let get = |i: i32| -> Option<&Block> { blocks.get(usize::try_from(i).ok()?)?.as_ref() };
    let mut mesh = Mesh::default();
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut total = 0usize;
    // walk from the roots (blocks no node names as a child), parents first
    let mut is_child = vec![false; blocks.len()];
    for blk in blocks.iter().flatten() {
        for &c in &blk.children {
            if let Some(x) = usize::try_from(c).ok().and_then(|c| is_child.get_mut(c)) {
                *x = true;
            }
        }
    }
    let mut stack: Vec<(usize, Xf, u32)> = (0..blocks.len())
        .filter(|&i| !is_child[i] && blocks[i].as_ref().is_some_and(|b| b.xf.is_some()))
        .map(|i| (i, Xf::ID, 0))
        .collect();
    stack.reverse();
    while let Some((i, parent, depth)) = stack.pop() {
        let Some(blk) = blocks[i].as_ref() else { continue };
        let xf = parent.then(&blk.xf.unwrap_or(Xf::ID));
        if depth < 64 {
            for &c in blk.children.iter().rev() {
                if let Ok(c) = usize::try_from(c)
                    && c < blocks.len()
                {
                    stack.push((c, xf, depth + 1));
                }
            }
        }
        let Some((pos, uv, ix)) = &blk.geo else { continue };
        let (pos, uv, ix) = if blk.data >= 0 {
            match get(blk.data).and_then(|d| d.geo.as_ref()) {
                Some(g) => (&g.0, &g.1, &g.2),
                None => continue,
            }
        } else if (pos.is_empty() || ix.is_empty()) && blk.skin >= 0 {
            // SSE skinned: the skin instance's partition holds the vertices and triangles (a dynamic shape — a
            // face — keeps its own positions)
            match get(blk.skin)
                .and_then(|sk| get(sk.data))
                .and_then(|d| d.geo.as_ref())
            {
                Some(g) => (
                    if pos.is_empty() { &g.0 } else { pos },
                    if uv.is_empty() { &g.1 } else { uv },
                    if ix.is_empty() { &g.2 } else { ix },
                ),
                None => continue,
            }
        } else {
            (pos, uv, ix)
        };
        if pos.is_empty() || ix.is_empty() {
            continue;
        }
        if total + pos.len() > MESH_VERTICES {
            mesh.notes
                .push(format!("меш больше {MESH_VERTICES} вершин: показана часть"));
            break;
        }
        total += pos.len();
        let world: Vec<[f32; 3]> = pos.iter().map(|&v| xf.apply(v)).collect();
        for v in &world {
            for a in 0..3 {
                lo[a] = lo[a].min(v[a]);
                hi[a] = hi[a].max(v[a]);
            }
        }
        let n = pos.len() as u32;
        let ix: Vec<u32> = ix
            .iter()
            .filter(|t| t.iter().all(|&x| x < n))
            .flatten()
            .copied()
            .collect();
        let shader = get(blk.shader);
        let texture = shader.and_then(|s| {
            s.texture
                .clone()
                .or_else(|| get(s.texture_set).and_then(|t| t.texture.clone()))
        });
        mesh.shapes.push(Shape {
            name: blk.name.clone(),
            vertices: n,
            triangles: (ix.len() / 3) as u32,
            positions: b64_f32(world.iter().flatten().copied()),
            uvs: if uv.len() == pos.len() {
                b64_f32(uv.iter().flatten().copied())
            } else {
                String::new()
            },
            indices: super::base64(&ix.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()),
            texture: texture.map(|t| crate::normalize_path(&t)).map(|t| {
                if t.starts_with("textures/") || t.contains(':') {
                    t
                } else {
                    format!("textures/{t}")
                }
            }),
            material: shader.and_then(|s| s.material.clone()),
            skinned: blk.skin >= 0,
            alpha: blk.alpha >= 0,
        });
    }
    if failed > 0 {
        mesh.notes
            .push(format!("не разобрано блоков геометрии: {failed}"));
    }
    if mesh.shapes.is_empty() {
        return (failed > 0).then_some(mesh);
    }
    mesh.bounds = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::super::inspect;

    fn sized(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u32).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    /// An SSE NIF: a node with one BSTriShape (a triangle, UVs), a lighting shader and a texture set.
    pub(crate) fn tri_nif() -> Vec<u8> {
        let mut blocks: Vec<(u16, Vec<u8>)> = Vec::new();
        let f = |v: &mut Vec<u8>, x: f32| v.extend_from_slice(&x.to_le_bytes());
        let u = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        let av = |v: &mut Vec<u8>, t: [f32; 3], s: f32| {
            u(v, 0xE);
            for x in t {
                f(v, x);
            }
            for x in [1., 0., 0., 0., 1., 0., 0., 0., 1.] {
                f(v, x);
            }
            f(v, s);
            u(v, u32::MAX); // collision
        };
        // 0: NiNode "root" translated by 10 on x, scale 2, child 1
        let mut n = Vec::new();
        u(&mut n, 0);
        u(&mut n, 0);
        u(&mut n, u32::MAX);
        av(&mut n, [10., 0., 0.], 2.);
        u(&mut n, 1);
        u(&mut n, 1);
        u(&mut n, 0);
        blocks.push((0, n));
        // 1: BSTriShape
        let mut s = Vec::new();
        u(&mut s, 1);
        u(&mut s, 0);
        u(&mut s, u32::MAX);
        av(&mut s, [0., 0., 0.], 1.);
        for _ in 0..4 {
            f(&mut s, 0.);
        }
        u(&mut s, u32::MAX); // skin
        u(&mut s, 2); // shader
        u(&mut s, u32::MAX); // alpha
        // vertex desc: size 5 dwords (pos 12 + w 4 + uv 4), uv at 4 dwords, attrs VERTEX|UV
        let desc: u64 = 5 | (4 << 8) | (0x3u64 << 44);
        s.extend_from_slice(&desc.to_le_bytes());
        s.extend_from_slice(&1u16.to_le_bytes());
        s.extend_from_slice(&3u16.to_le_bytes());
        u(&mut s, 3 * 20 + 6);
        for (p, t) in [
            ([0., 0., 0.], [0u16, 0]),
            ([1., 0., 0.], [0x3C00, 0]),
            ([0., 1., 0.], [0, 0x3C00]),
        ] {
            for x in p {
                f(&mut s, x);
            }
            u(&mut s, 0);
            s.extend_from_slice(&t[0].to_le_bytes());
            s.extend_from_slice(&t[1].to_le_bytes());
        }
        for i in [0u16, 1, 2] {
            s.extend_from_slice(&i.to_le_bytes());
        }
        u(&mut s, 0); // particle data size
        blocks.push((1, s));
        // 2: BSLightingShaderProperty
        let mut l = Vec::new();
        u(&mut l, 1); // shader type
        u(&mut l, u32::MAX);
        u(&mut l, 0);
        u(&mut l, u32::MAX);
        u(&mut l, 0);
        u(&mut l, 0);
        for x in [0., 0., 1., 1.] {
            f(&mut l, x);
        }
        u(&mut l, 3);
        blocks.push((2, l));
        // 3: BSShaderTextureSet
        let mut t = Vec::new();
        u(&mut t, 2);
        sized(&mut t, "textures\\x\\a_d.dds");
        sized(&mut t, "textures\\x\\a_n.dds");
        blocks.push((3, t));

        let mut b = b"Gamebryo File Format, Version 20.2.0.7\n".to_vec();
        u(&mut b, 0x14020007);
        b.push(1);
        u(&mut b, 12);
        u(&mut b, blocks.len() as u32);
        u(&mut b, 100);
        b.extend_from_slice(&[0; 3]);
        let types = [
            "NiNode",
            "BSTriShape",
            "BSLightingShaderProperty",
            "BSShaderTextureSet",
        ];
        b.extend_from_slice(&(types.len() as u16).to_le_bytes());
        for t in types {
            sized(&mut b, t);
        }
        for (ti, _) in &blocks {
            b.extend_from_slice(&ti.to_le_bytes());
        }
        for (_, d) in &blocks {
            u(&mut b, d.len() as u32);
        }
        u(&mut b, 2);
        u(&mut b, 4);
        sized(&mut b, "root");
        sized(&mut b, "Tri");
        u(&mut b, 0);
        for (_, d) in &blocks {
            b.extend_from_slice(d);
        }
        b
    }

    #[test]
    fn a_shape_is_placed_by_its_nodes_and_names_its_diffuse() {
        let p = inspect("m.nif", &tri_nif());
        let m = p.mesh.expect("geometry");
        assert!(m.notes.is_empty(), "{:?}", m.notes);
        assert_eq!(m.shapes.len(), 1);
        let s = &m.shapes[0];
        assert_eq!((s.name.as_str(), s.vertices, s.triangles), ("Tri", 3, 1));
        assert_eq!(s.texture.as_deref(), Some("textures/x/a_d.dds"));
        // translated by 10 and scaled by 2: x from 10 to 12, y from 0 to 2
        assert_eq!(m.bounds, [10., 0., 0., 12., 2., 0.]);
        assert!(!s.uvs.is_empty());
    }
}
