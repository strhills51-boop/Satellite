//! Havok skeletons and animations for playback: packfile objects, spline-compressed tracks decoded to frames. @sat:formats.preview.motion
//!
//! Owns: binary packfiles of Skyrim LE / SE (hk_2010, 32 / 64-bit) and Fallout 4 (hk_2014) — sections, local and
//! virtual fixups, objects by class; `hkaSkeleton` (bones, parents, reference pose), `hkaSplineCompressedAnimation`
//! (blocks, track masks, B-splines of degree ≤ 3, positions and scales quantized to 8 / 16 bits, rotations as
//! POLAR32 / THREECOMP40 / THREECOMP48 / uncompressed), `hkaInterleavedUncompressedAnimation`,
//! `hkaAnimationBinding` (track → bone, additive), annotations (events with times); frames packed as base64.
//! Not here: what the file is (`@sat:formats.preview.hkx`), finding the skeleton of an animation
//! (`@sat:app.preview.skeleton`), drawing (the viewer of the explorer).
//!
//! The format is read from its public descriptions (Havok packfile layout, the spline compression scheme); no
//! Havok code. Every read is bounded: a malformed file gives no motion and a note.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Samples (frames × tracks) at most; longer animations are thinned to fit.
pub const MOTION_SAMPLES: usize = 400_000;

/// A skeleton or an animation ready to play.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Motion {
    /// `skeleton` or `animation`.
    pub kind: String,
    /// Bone names (a skeleton) or track names (an animation: its annotation tracks, usually bone names).
    pub names: Vec<String>,
    /// A skeleton: the parent of each bone (−1 for a root).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<i16>,
    /// A skeleton: base64 `f32` × 10 per bone — translation xyz, rotation xyzw, scale xyz — in the parent's space.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pose: String,
    /// An animation: track → bone index of its skeleton (empty: track i is bone i).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub track_bones: Vec<i16>,
    /// The skeleton the animation was made for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skeleton: String,
    #[serde(default)]
    pub additive: bool,
    pub duration: f32,
    /// Frames in `samples`.
    pub frames: u32,
    /// An animation: base64 `f32` × 7 per frame per track — translation xyz, rotation xyzw.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub samples: String,
    /// Annotations: (time, text).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<(f32, String)>,
    /// How the tracks are stored (`spline`, `interleaved`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compression: String,
}

/// The objects of a binary packfile.
struct Pack<'a> {
    b: &'a [u8],
    ptr: usize,
    data: usize,
    /// Local fixups of `__data__`: pointer field → target (both relative to the section).
    fix: HashMap<u32, u32>,
    /// (object, class).
    objects: Vec<(u32, String)>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    u32_at(b, at).map(f32::from_bits)
}
fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

impl<'a> Pack<'a> {
    fn open(b: &'a [u8]) -> Option<Self> {
        if u32_at(b, 0)? != 0x57E0_E057 || u32_at(b, 4)? != 0x10C0_C010 {
            return None;
        }
        let ver = u32_at(b, 0x0C)?;
        let ptr = *b.get(0x10)? as usize;
        if ptr != 4 && ptr != 8 || *b.get(0x11)? != 1 {
            return None;
        }
        let n = u32_at(b, 0x14)? as usize;
        // hk_2014 (file version 11) has 16 more bytes after the header and bigger section headers
        let (start, size) = if ver >= 11 {
            let pad = u16_at(b, 0x42).filter(|&x| x <= 64).unwrap_or(0) as usize;
            (0x40 + 0x10 + pad.saturating_sub(0x10), 0x40)
        } else {
            (0x40, 0x30)
        };
        let mut classnames = None;
        let mut data = None;
        for i in 0..n.min(8) {
            let h = start + i * size;
            let name = b.get(h..h + 16)?;
            let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(16)];
            let abs = u32_at(b, h + 0x14)? as usize;
            let off = |k: usize| u32_at(b, h + 0x18 + 4 * k).map(|x| abs + x as usize);
            match name {
                b"__classnames__" => classnames = Some(abs),
                b"__data__" => data = Some((abs, off(0)?, off(1)?, off(2)?, off(3)?)),
                _ => {}
            }
        }
        let cn = classnames?;
        let (abs, local, global, virt, exports) = data?;
        let mut fix = HashMap::new();
        let mut at = local;
        while at + 8 <= global {
            let (s, d) = (u32_at(b, at)?, u32_at(b, at + 4)?);
            if s == u32::MAX {
                break;
            }
            fix.insert(s, d);
            at += 8;
        }
        let mut objects = Vec::new();
        let mut at = virt;
        while at + 12 <= exports {
            let (o, name) = (u32_at(b, at)?, u32_at(b, at + 8)?);
            if o == u32::MAX {
                break;
            }
            let s = cn + name as usize;
            let end = b.get(s..)?.iter().take(256).position(|&c| c == 0)?;
            objects.push((o, String::from_utf8_lossy(&b[s..s + end]).into_owned()));
            at += 12;
        }
        Some(Pack {
            b,
            ptr,
            data: abs,
            fix,
            objects,
        })
    }
    fn find(&self, class: &str) -> Option<u32> {
        self.objects.iter().find(|(_, c)| c == class).map(|(o, _)| *o)
    }
    fn u32(&self, rel: u32) -> Option<u32> {
        u32_at(self.b, self.data + rel as usize)
    }
    fn f32(&self, rel: u32) -> Option<f32> {
        f32_at(self.b, self.data + rel as usize)
    }
    /// An `hkArray` field: (content, count).
    fn array(&self, field: u32) -> Option<(u32, usize)> {
        let n = self.u32(field + self.ptr as u32)? as usize;
        if n == 0 {
            return Some((0, 0));
        }
        Some((*self.fix.get(&field)?, n.min(1 << 24)))
    }
    fn bytes(&self, rel: u32, n: usize) -> Option<&'a [u8]> {
        let s = self.data + rel as usize;
        self.b.get(s..s.checked_add(n)?)
    }
    /// A string pointer field.
    fn string(&self, field: u32) -> Option<String> {
        let t = self.data + *self.fix.get(&field)? as usize;
        let end = self.b.get(t..)?.iter().take(1024).position(|&c| c == 0)?;
        Some(String::from_utf8_lossy(&self.b[t..t + end]).into_owned())
    }
    fn arr_size(&self) -> u32 {
        self.ptr as u32 + 8
    }
    fn base(&self) -> u32 {
        2 * self.ptr as u32
    }
}

fn b64(v: &[f32]) -> String {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    super::base64(&bytes)
}

/// The motion of a packfile: its animation when it has one, else its skeleton.
pub(super) fn read(b: &[u8]) -> Result<Option<Motion>, String> {
    let Some(p) = Pack::open(b) else {
        return Ok(None);
    };
    if let Some((o, class)) = p
        .objects
        .iter()
        .find(|(_, c)| c == "hkaSplineCompressedAnimation" || c == "hkaInterleavedUncompressedAnimation")
    {
        return animation(&p, *o, class)
            .map(Some)
            .ok_or_else(|| "анимация не распаковалась".to_string());
    }
    if let Some(o) = p.find("hkaSkeleton") {
        return skeleton(&p, o)
            .map(Some)
            .ok_or_else(|| "скелет не прочитался".to_string());
    }
    Ok(None)
}

fn skeleton(p: &Pack<'_>, o: u32) -> Option<Motion> {
    let a = p.arr_size();
    let name = p.base();
    let parents_f = o + name + p.ptr as u32;
    let bones_f = parents_f + a;
    let pose_f = bones_f + a;
    let (pc, pn) = p.array(parents_f)?;
    let parents: Vec<i16> = p
        .bytes(pc, pn * 2)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|x| i16::from_le_bytes(*x))
        .collect();
    let (bc, bn) = p.array(bones_f)?;
    let stride = if p.ptr == 8 { 16 } else { 8 };
    let names: Vec<String> = (0..bn)
        .map(|i| p.string(bc + (i * stride) as u32).unwrap_or_default())
        .collect();
    let (qc, qn) = p.array(pose_f)?;
    let mut pose = Vec::with_capacity(qn * 10);
    for i in 0..qn {
        let at = qc + (i * 48) as u32;
        for k in [0, 1, 2, 4, 5, 6, 7, 8, 9, 10] {
            pose.push(p.f32(at + k * 4)?);
        }
    }
    if parents.len() != names.len() || qn != names.len() {
        return None;
    }
    Some(Motion {
        kind: "skeleton".into(),
        skeleton: p.string(o + name).unwrap_or_default(),
        names,
        parents,
        pose: b64(&pose),
        ..Default::default()
    })
}

/// Per track and frame: translation, rotation (xyzw).
type Track = Vec<([f32; 3], [f32; 4])>;

fn animation(p: &Pack<'_>, o: u32, class: &str) -> Option<Motion> {
    let a = p.arr_size();
    let ptr = p.ptr as u32;
    let base = p.base();
    let duration = p.f32(o + base + 4)?;
    let tracks = p.u32(o + base + 8)? as usize;
    let ann_f = base + 16 + ptr;
    let after = ann_f + a;
    let mut m = Motion {
        kind: "animation".into(),
        duration,
        ..Default::default()
    };
    // annotation tracks: names (bones) and events
    let (ac, an) = p.array(o + ann_f)?;
    let stride = ptr + a;
    for i in 0..an as u32 {
        let t = ac + i * stride;
        m.names.push(p.string(t).unwrap_or_default());
        if let Some((ec, en)) = p.array(t + ptr) {
            for j in 0..en.min(1024) as u32 {
                let e = ec + j * 2 * ptr;
                if let (Some(time), Some(text)) = (p.f32(e), p.string(e + ptr)) {
                    m.events.push((time, text));
                }
            }
        }
    }
    m.events.sort_by(|x, y| x.0.total_cmp(&y.0));
    // the binding: which bone each track moves, additive or not
    if let Some(bo) = p.find("hkaAnimationBinding") {
        m.skeleton = p.string(bo + base).unwrap_or_default();
        let idx_f = bo + base + 2 * ptr;
        if let Some((ic, n)) = p.array(idx_f) {
            m.track_bones = p
                .bytes(ic, n * 2)
                .map(|x| {
                    x.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| i16::from_le_bytes(*c))
                        .collect()
                })
                .unwrap_or_default();
        }
        m.additive = p.u32(idx_f + 2 * a).is_some_and(|h| h & 0xff == 1);
    }
    let decoded: Vec<Track> = if class == "hkaInterleavedUncompressedAnimation" {
        m.compression = "interleaved".into();
        let (tc, tn) = p.array(o + after)?;
        if tracks == 0 || tn % tracks != 0 {
            return None;
        }
        let frames = tn / tracks;
        let mut out: Vec<Track> = vec![Vec::with_capacity(frames); tracks];
        for i in 0..tn {
            let at = tc + (i * 48) as u32;
            let t = [p.f32(at)?, p.f32(at + 4)?, p.f32(at + 8)?];
            let q = [p.f32(at + 16)?, p.f32(at + 20)?, p.f32(at + 24)?, p.f32(at + 28)?];
            out[i % tracks].push((t, q));
        }
        out
    } else {
        m.compression = "spline".into();
        let frames = p.u32(o + after)? as usize;
        let blocks = p.u32(o + after + 4)? as usize;
        let per_block = p.u32(o + after + 8)? as usize;
        let mask_size = p.u32(o + after + 12)? as usize;
        let offs_f = (after + 28).next_multiple_of(ptr);
        let (bc, bn) = p.array(o + offs_f)?;
        let block_offsets: Vec<usize> = (0..bn as u32)
            .map(|i| p.u32(bc + 4 * i).map(|x| x as usize))
            .collect::<Option<_>>()?;
        let (dc, dn) = p.array(o + offs_f + 4 * a)?;
        let data = p.bytes(dc, dn)?;
        if frames == 0 || per_block < 2 || blocks == 0 || blocks > bn || tracks > 4096 {
            return None;
        }
        spline(data, tracks, frames, blocks, per_block, &block_offsets, mask_size)?
    };
    let frames = decoded.first().map_or(0, Vec::len);
    // thin long animations to the sample budget
    let step = (frames * tracks.max(1)).div_ceil(MOTION_SAMPLES).max(1);
    let mut s = Vec::with_capacity(frames / step * tracks * 7);
    let mut kept = 0u32;
    for f in (0..frames).step_by(step) {
        kept += 1;
        for t in &decoded {
            let (tr, q) = t.get(f).copied().unwrap_or(([0.; 3], [0., 0., 0., 1.]));
            s.extend_from_slice(&tr);
            s.extend_from_slice(&q);
        }
    }
    m.frames = kept;
    m.samples = b64(&s);
    if m.names.len() != tracks {
        m.names.resize(tracks, String::new());
    }
    Some(m)
}

fn align(at: usize, a: usize) -> usize {
    if a <= 1 { at } else { at.next_multiple_of(a) }
}

fn norm(q: [f32; 4]) -> [f32; 4] {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l < 1e-10 {
        [0., 0., 0., 1.]
    } else {
        [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
    }
}

/// Puts the reconstructed component `r` at index `shift` among the three stored ones.
fn place(shift: u32, v: [f32; 3], r: f32) -> [f32; 4] {
    match shift {
        0 => [r, v[0], v[1], v[2]],
        1 => [v[0], r, v[1], v[2]],
        2 => [v[0], v[1], r, v[2]],
        _ => [v[0], v[1], v[2], r],
    }
}

/// One quantized rotation: (quaternion, bytes read).
fn quat(fmt: u8, d: &[u8], at: usize) -> Option<([f32; 4], usize)> {
    Some(match fmt {
        0 => {
            let c = u32_at(d, at)?;
            let rm = (1u32 << 10) - 1;
            let r = ((c >> 18) & rm) as f32 / rm as f32;
            let r = 1.0 - r * r;
            let pt = (c & 0x3FFFF) as f32;
            let mut phi = pt.sqrt().floor();
            let mut theta = 0.0;
            if phi > 0.0 {
                theta = std::f32::consts::FRAC_PI_4 * (pt - phi * phi) / phi;
                phi *= std::f32::consts::FRAC_PI_2 / 511.0;
            }
            let mag = (1.0 - r * r).max(0.0).sqrt();
            let mut q = [
                phi.sin() * theta.cos() * mag,
                phi.sin() * theta.sin() * mag,
                phi.cos() * mag,
                r,
            ];
            for (i, x) in q.iter_mut().enumerate() {
                if c & (0x1000_0000 << i) != 0 {
                    *x = -*x;
                }
            }
            (norm(q), 4)
        }
        1 => {
            let raw = d.get(at..at + 5)?;
            let v = raw.iter().rev().fold(0u64, |a, &x| (a << 8) | x as u64);
            let k = 0.000_345_436_f32;
            let f = |s: u32| (((v >> s) & 0xFFF) as f32 - 2049.0) * k;
            let s = [f(0), f(12), f(24)];
            let mut r = (1.0 - s.iter().map(|x| x * x).sum::<f32>()).max(0.0).sqrt();
            if (v >> 38) & 1 != 0 {
                r = -r;
            }
            (norm(place(((v >> 36) & 3) as u32, s, r)), 5)
        }
        2 => {
            let (x, y, z) = (u16_at(d, at)?, u16_at(d, at + 2)?, u16_at(d, at + 4)?);
            let shift = (((y >> 14) & 2) | ((x >> 15) & 1)) as u32;
            let k = 0.000_043_161_f32;
            let f = |v: u16| ((v & 0x7FFF) as f32 - 16383.0) * k;
            let s = [f(x), f(y), f(z)];
            let mut r = (1.0 - s.iter().map(|x| x * x).sum::<f32>()).max(0.0).sqrt();
            if z >> 15 != 0 {
                r = -r;
            }
            (norm(place(shift, s, r)), 6)
        }
        5 => (
            norm([
                f32_at(d, at)?,
                f32_at(d, at + 4)?,
                f32_at(d, at + 8)?,
                f32_at(d, at + 12)?,
            ]),
            16,
        ),
        _ => return None,
    })
}

fn quat_align(fmt: u8) -> usize {
    match fmt {
        0 | 5 => 4,
        2 | 4 => 2,
        _ => 1,
    }
}

/// The knot span of `u` (NURBS book A2.1); `n` — control points − 1.
fn span(n: usize, p: usize, u: f32, k: &[f32]) -> usize {
    if u >= k[n + 1] {
        return n;
    }
    if u <= k[p] {
        return p;
    }
    let (mut lo, mut hi) = (p, n + 1);
    let mut mid = (lo + hi) / 2;
    for _ in 0..64 {
        if u < k[mid] {
            hi = mid;
        } else if u >= k[mid + 1] {
            lo = mid;
        } else {
            break;
        }
        mid = (lo + hi) / 2;
    }
    mid
}

/// B-spline value at `u` (NURBS book A2.2 basis functions); `D` components per control point.
fn eval<const D: usize>(p: usize, u: f32, k: &[f32], cps: &[[f32; D]]) -> [f32; D] {
    if cps.len() == 1 || p == 0 {
        let i = if cps.len() == 1 {
            0
        } else {
            span(cps.len() - 1, 0, u, k).min(cps.len() - 1)
        };
        return cps[i];
    }
    let n = cps.len() - 1;
    let s = span(n, p, u, k);
    let mut nb = [0f32; 4];
    let (mut left, mut right) = ([0f32; 4], [0f32; 4]);
    nb[0] = 1.0;
    for j in 1..=p {
        left[j] = u - k[s + 1 - j];
        right[j] = k[s + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            let den = right[r + 1] + left[j - r];
            let t = if den.abs() < 1e-12 { 0.0 } else { nb[r] / den };
            nb[r] = saved + right[r + 1] * t;
            saved = left[j - r] * t;
        }
        nb[j] = saved;
    }
    let mut out = [0f32; D];
    for (i, w) in nb.iter().enumerate().take(p + 1) {
        let Some(c) = cps.get(s - p + i) else { continue };
        for d in 0..D {
            out[d] += c[d] * w;
        }
    }
    out
}

/// A spline header: (control points − 1, degree, knots), the cursor after the knots.
fn header(d: &[u8], at: usize) -> Option<(usize, usize, Vec<f32>, usize)> {
    let n = u16_at(d, at)? as usize;
    let p = *d.get(at + 2)? as usize;
    if p > 3 {
        return None;
    }
    let nk = n + p + 2;
    let k: Vec<f32> = d.get(at + 3..at + 3 + nk)?.iter().map(|&x| x as f32).collect();
    Some((n, p, k, at + 3 + nk))
}

/// Three channels (position or scale): identity value `id`, flags (static bits 0..3, spline bits 4..7), quantization.
fn vec3(
    d: &[u8],
    mut at: usize,
    flags: u8,
    quant: u8,
    id: f32,
    frames: usize,
) -> Option<(Vec<[f32; 3]>, usize)> {
    let spline_bits = (flags >> 4) & 7;
    if spline_bits == 0 {
        let mut v = [id; 3];
        for (axis, x) in v.iter_mut().enumerate() {
            if flags & (1 << axis) != 0 {
                *x = f32_at(d, at)?;
                at += 4;
            }
        }
        return Some((vec![v; frames], align(at, 4)));
    }
    let (n, p, k, mut at2) = header(d, at)?;
    at2 = align(at2, 4);
    let mut info = [(0u8, id, id); 3];
    for (axis, inf) in info.iter_mut().enumerate() {
        if spline_bits & (1 << axis) != 0 {
            *inf = (2, f32_at(d, at2)?, f32_at(d, at2 + 4)?);
            at2 += 8;
        } else if flags & (1 << axis) != 0 {
            let v = f32_at(d, at2)?;
            *inf = (1, v, v);
            at2 += 4;
        }
    }
    let mut cps = vec![[0f32; 3]; n + 1];
    for cp in cps.iter_mut() {
        for (axis, &(kind, lo, hi)) in info.iter().enumerate() {
            cp[axis] = match kind {
                2 => {
                    let t = if quant == 0 {
                        let v = *d.get(at2)? as f32 / 255.0;
                        at2 += 1;
                        v
                    } else {
                        let v = u16_at(d, at2)? as f32 / 65535.0;
                        at2 += 2;
                        v
                    };
                    lo + (hi - lo) * t
                }
                _ => lo,
            };
        }
    }
    let out = (0..frames).map(|f| eval(p, f as f32, &k, &cps)).collect();
    Some((out, align(at2, 4)))
}

fn rotation(d: &[u8], mut at: usize, flags: u8, fmt: u8, frames: usize) -> Option<(Vec<[f32; 4]>, usize)> {
    let qa = quat_align(fmt);
    if (flags >> 4) & 0xF != 0 {
        let (n, p, k, a2) = header(d, at)?;
        at = align(a2, qa);
        let mut cps: Vec<[f32; 4]> = Vec::with_capacity(n + 1);
        for _ in 0..=n {
            let (mut q, used) = quat(fmt, d, at)?;
            at += used;
            if let Some(prev) = cps.last()
                && q.iter().zip(prev).map(|(a, b)| a * b).sum::<f32>() < 0.0
            {
                q = q.map(|x| -x);
            }
            cps.push(q);
        }
        let out = (0..frames).map(|f| norm(eval(p, f as f32, &k, &cps))).collect();
        Some((out, align(at, 4)))
    } else if flags & 0xF != 0 {
        at = align(at, qa);
        let (q, used) = quat(fmt, d, at)?;
        Some((vec![q; frames], align(at + used, 4)))
    } else {
        Some((vec![[0., 0., 0., 1.]; frames], align(at, 4)))
    }
}

/// Spline-compressed tracks to frames: each block holds the masks of all tracks, then each track's position,
/// rotation and scale; blocks overlap by one frame.
fn spline(
    d: &[u8],
    tracks: usize,
    frames: usize,
    blocks: usize,
    per_block: usize,
    offsets: &[usize],
    mask_size: usize,
) -> Option<Vec<Track>> {
    let mut out: Vec<Track> = vec![Vec::with_capacity(frames); tracks];
    let mask_size = if mask_size == 0 { 4 * tracks } else { mask_size };
    for (b, &start) in offsets.iter().enumerate().take(blocks) {
        let first = b * (per_block - 1);
        if first >= frames {
            break;
        }
        let n = per_block.min(frames - first);
        let skip = usize::from(b > 0);
        let masks = d.get(start..start + 4 * tracks)?;
        let mut at = start + mask_size;
        for (t, track) in out.iter_mut().enumerate() {
            let m = &masks[t * 4..t * 4 + 4];
            let (pq, rq, sq) = (m[0] & 3, (m[0] >> 2) & 0xF, (m[0] >> 6) & 3);
            let (pos, a1) = vec3(d, at, m[1], pq, 0.0, n)?;
            let (rot, a2) = rotation(d, a1, m[2], rq, n)?;
            let (_scale, a3) = vec3(d, a2, m[3], sq, 1.0, n)?;
            at = a3;
            track.extend(pos.into_iter().zip(rot).skip(skip));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dumps decoded frames of the files of `SAT_MOTION_DIR` as JSON lines to `SAT_MOTION_OUT` (checks against
    /// other readers).
    #[test]
    #[ignore]
    fn dump_real_files() {
        let (Ok(dir), Ok(out)) = (std::env::var("SAT_MOTION_DIR"), std::env::var("SAT_MOTION_OUT")) else {
            return;
        };
        let mut lines = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let b = std::fs::read(e.path()).unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            let r = read(&b);
            let v = match r {
                Ok(Some(m)) => {
                    let raw = if m.kind == "skeleton" { &m.pose } else { &m.samples };
                    let bytes = crate::preview::tests_b64_decode(raw);
                    let f: Vec<f32> = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| f32::from_le_bytes(*c))
                        .collect();
                    serde_json::json!({"file": name, "kind": m.kind, "frames": m.frames, "names": m.names,
                        "bones": m.track_bones, "parents": m.parents, "values": f, "additive": m.additive})
                }
                Ok(None) => serde_json::json!({"file": name, "kind": "none"}),
                Err(e) => serde_json::json!({"file": name, "error": e}),
            };
            lines.push(v.to_string());
        }
        std::fs::write(out, lines.join("\n")).unwrap();
    }

    #[test]
    fn splines_of_degree_one_and_three_pass_through_their_ends() {
        // degree 1, clamped knots 0 0 1 2 2: control points 0, 10, 20 at frames 0, 1, 2
        let k = [0., 0., 1., 2., 2.];
        let cps = [[0f32], [10.], [20.]];
        assert_eq!(eval(1, 0.0, &k, &cps), [0.]);
        assert_eq!(eval(1, 0.5, &k, &cps), [5.]);
        assert_eq!(eval(1, 2.0, &k, &cps), [20.]);
        // degree 3, clamped: ends interpolated
        let k = [0., 0., 0., 0., 4., 4., 4., 4.];
        let cps = [[1f32], [2.], [3.], [4.]];
        assert!((eval(3, 0.0, &k, &cps)[0] - 1.0).abs() < 1e-5);
        assert!((eval(3, 4.0, &k, &cps)[0] - 4.0).abs() < 1e-5);
    }

    #[test]
    fn quantized_rotations_decode_to_unit_quaternions() {
        // THREECOMP48 of the identity: x, y, z at the middle, w reconstructed (shift 3), positive
        let mid = 16383u16;
        let x = mid | 0x8000; // shift bit 0
        let y = mid | 0x8000; // shift bit 1 (bit 15 of y, read as 2)
        let z = mid;
        let mut d = Vec::new();
        for v in [x, y, z] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let (q, used) = quat(2, &d, 0).unwrap();
        assert_eq!(used, 6);
        assert!((q[3] - 1.0).abs() < 1e-4, "{q:?}");
        let (q, _) = quat(5, &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63], 0).unwrap();
        assert_eq!(q, [0., 0., 0., 1.]);
    }
}
