//! Opening AVIF files (B4): the container (HEIF boxes: the primary item, its alpha item, colour, ICC,
//! rotation / mirroring), AV1 decoding with rav1d (the Rust port of dav1d, the decoder Pillow's libavif
//! uses) and YUV → RGB as libavif's own conversion (float, bilinear chroma). Grids, image sequences
//! without a primary item and premultiplied alpha answer Unknown (the shell opens those itself).

use crate::decode::Image;
use crate::{Error, ErrorKind};
use std::collections::HashMap;

fn bad(msg: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Failed, format!("avif: {msg}"))
}

fn unsupported(msg: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Unknown, format!("avif: {msg}"))
}

// ---- boxes ----------------------------------------------------------------------------------------------

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn new(d: &'a [u8]) -> Rd<'a> {
        Rd { d, p: 0 }
    }
    fn left(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let s = self.d.get(self.p..self.p + n).ok_or_else(|| bad("truncated box"))?;
        self.p += n;
        Ok(s)
    }
    fn uint(&mut self, n: usize) -> Result<u64, Error> {
        Ok(self.bytes(n)?.iter().fold(0u64, |v, &b| (v << 8) | b as u64))
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.uint(1)? as u8)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(self.uint(2)? as u16)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(self.uint(4)? as u32)
    }
    /// version, flags of a full box
    fn full(&mut self) -> Result<(u8, u32), Error> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0xFF_FFFF))
    }
    fn cstr(&mut self) -> Result<&'a [u8], Error> {
        let rest = &self.d[self.p.min(self.d.len())..];
        let n = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        self.p += (n + 1).min(rest.len());
        Ok(&rest[..n])
    }
}

/// The boxes in `d`: (type, payload).
fn boxes(d: &[u8]) -> Result<Vec<([u8; 4], &[u8])>, Error> {
    let mut out = vec![];
    let mut r = Rd::new(d);
    while r.left() >= 8 {
        let start = r.p;
        let size = r.u32()? as u64;
        let typ: [u8; 4] = r.bytes(4)?.try_into().unwrap();
        let (hdr, size) = match size {
            1 => (16, r.uint(8)?),
            0 => (8, (d.len() - start) as u64),
            s => (8, s),
        };
        if size < hdr as u64 || start as u64 + size > d.len() as u64 {
            return Err(bad(format!("box {} past the end", String::from_utf8_lossy(&typ))));
        }
        out.push((typ, &d[start + hdr..start + size as usize]));
        r.p = start + size as usize;
    }
    Ok(out)
}

#[derive(Default, Clone)]
struct Item {
    typ: [u8; 4],
    method: u8,
    base: u64,
    extents: Vec<(u64, u64)>,
    props: Vec<u16>,
}

#[derive(Clone)]
enum Prop {
    Av1c,
    Nclx { cp: u16, tc: u16, mc: u16, full: bool },
    Icc(Vec<u8>),
    Irot(u8),
    Imir(u8),
    AuxC(Vec<u8>),
    Other,
}

/// What the container says about the image.
pub struct Container {
    color: Vec<u8>,
    alpha: Option<Vec<u8>>,
    nclx: Option<(u16, u16, u16, bool)>,
    icc: Option<Vec<u8>>,
    /// Pillow's EXIF orientation from irot / imir
    orientation: u32,
}

const ALPHA_URNS: [&[u8]; 2] = [b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha", b"urn:mpeg:hevc:2015:auxid:1"];

pub fn parse(data: &[u8]) -> Result<Container, Error> {
    let top = boxes(data)?;
    let meta = top.iter().find(|(t, _)| t == b"meta").map(|b| b.1).ok_or_else(|| unsupported("no meta box"))?;
    let mut m = Rd::new(meta);
    m.full()?;
    let mut primary = None;
    let mut items: HashMap<u32, Item> = HashMap::new();
    let mut props: Vec<Prop> = vec![];
    let mut auxl: Vec<(u32, u32)> = vec![]; // (from = aux item, to = master)
    let mut prem: Vec<u32> = vec![];
    let mut idat: &[u8] = &[];
    for (t, b) in boxes(&meta[4..])? {
        let mut r = Rd::new(b);
        match &t {
            b"pitm" => {
                let (v, _) = r.full()?;
                primary = Some(if v == 0 { r.u16()? as u32 } else { r.u32()? });
            }
            b"idat" => idat = b,
            b"iinf" => {
                let (v, _) = r.full()?;
                if v == 0 {
                    r.u16()?;
                } else {
                    r.u32()?;
                }
                for (t2, e) in boxes(&b[r.p..])? {
                    if &t2 != b"infe" {
                        continue;
                    }
                    let mut e = Rd::new(e);
                    let (v, _) = e.full()?;
                    if v < 2 {
                        continue;
                    }
                    let id = if v == 2 { e.u16()? as u32 } else { e.u32()? };
                    e.u16()?;
                    let typ: [u8; 4] = e.bytes(4)?.try_into().unwrap();
                    items.entry(id).or_default().typ = typ;
                }
            }
            b"iloc" => {
                let (v, _) = r.full()?;
                let a = r.u8()?;
                let b2 = r.u8()?;
                let (off_sz, len_sz, base_sz) = ((a >> 4) as usize, (a & 15) as usize, (b2 >> 4) as usize);
                let idx_sz = if v == 1 || v == 2 { (b2 & 15) as usize } else { 0 };
                let n = if v < 2 { r.u16()? as u32 } else { r.u32()? };
                for _ in 0..n {
                    let id = if v < 2 { r.u16()? as u32 } else { r.u32()? };
                    let method = if v == 1 || v == 2 { (r.u16()? & 15) as u8 } else { 0 };
                    r.u16()?; // data_reference_index
                    let base = r.uint(base_sz)?;
                    let ne = r.u16()?;
                    let mut ext = vec![];
                    for _ in 0..ne {
                        r.uint(idx_sz)?;
                        let o = r.uint(off_sz)?;
                        let l = r.uint(len_sz)?;
                        ext.push((o, l));
                    }
                    let it = items.entry(id).or_default();
                    it.method = method;
                    it.base = base;
                    it.extents = ext;
                }
            }
            b"iref" => {
                let (v, _) = r.full()?;
                for (t2, rb) in boxes(&b[4..])? {
                    let mut rr = Rd::new(rb);
                    let id = |rr: &mut Rd| -> Result<u32, Error> { if v == 0 { Ok(rr.u16()? as u32) } else { rr.u32() } };
                    let from = id(&mut rr)?;
                    let n = rr.u16()?;
                    for _ in 0..n {
                        let to = id(&mut rr)?;
                        match &t2 {
                            b"auxl" => auxl.push((from, to)),
                            b"prem" => prem.push(from),
                            _ => {}
                        }
                    }
                }
            }
            b"iprp" => {
                for (t2, pb) in boxes(b)? {
                    if &t2 == b"ipco" {
                        for (pt, p) in boxes(pb)? {
                            props.push(property(&pt, p)?);
                        }
                    } else if &t2 == b"ipma" {
                        let mut rr = Rd::new(pb);
                        let (v, f) = rr.full()?;
                        let n = rr.u32()?;
                        for _ in 0..n {
                            let id = if v < 1 { rr.u16()? as u32 } else { rr.u32()? };
                            let k = rr.u8()?;
                            for _ in 0..k {
                                let idx = if f & 1 != 0 { rr.u16()? & 0x7FFF } else { (rr.u8()? & 0x7F) as u16 };
                                items.entry(id).or_default().props.push(idx);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let pid = primary.ok_or_else(|| unsupported("no primary item"))?;
    let p = items.get(&pid).ok_or_else(|| bad("primary item missing"))?;
    if &p.typ != b"av01" {
        return Err(unsupported(format!("primary item of type {}", String::from_utf8_lossy(&p.typ))));
    }
    if prem.contains(&pid) {
        return Err(unsupported("premultiplied alpha"));
    }
    let prop = |_: &Item, i: u16| -> Option<Prop> { if i == 0 { None } else { props.get(i as usize - 1).cloned() } };
    let read = |it: &Item| -> Result<Vec<u8>, Error> {
        let src = if it.method == 1 { idat } else { data };
        if it.method > 1 {
            return Err(unsupported("item data by reference"));
        }
        let mut v = vec![];
        for &(o, l) in &it.extents {
            let s = (it.base + o) as usize;
            let e = if l == 0 { src.len() } else { s + l as usize };
            v.extend_from_slice(src.get(s..e).ok_or_else(|| bad("item data past the end"))?);
        }
        Ok(v)
    };
    let (mut nclx, mut icc, mut irot, mut imir) = (None, None, None, None);
    for &i in &p.props {
        match prop(p, i) {
            Some(Prop::Nclx { cp, tc, mc, full }) if nclx.is_none() => nclx = Some((cp, tc, mc, full)),
            Some(Prop::Icc(d)) if icc.is_none() => icc = Some(d),
            Some(Prop::Irot(a)) => irot = Some(a),
            Some(Prop::Imir(a)) => imir = Some(a),
            _ => {}
        }
    }
    let _ = Prop::Av1c;
    let alpha_item = auxl.iter().filter(|(_, to)| *to == pid).map(|(from, _)| *from).find(|a| {
        items.get(a).is_some_and(|it| {
            &it.typ == b"av01"
                && it.props.iter().any(|&i| matches!(prop(it, i), Some(Prop::AuxC(u)) if ALPHA_URNS.contains(&u.as_slice())))
        })
    });
    if alpha_item.is_some_and(|a| prem.contains(&a)) {
        return Err(unsupported("premultiplied alpha"));
    }
    let alpha = match alpha_item {
        Some(a) => Some(read(&items[&a])?),
        None => None,
    };
    Ok(Container { color: read(p)?, alpha, nclx, icc, orientation: orientation(irot, imir) })
}

fn property(t: &[u8; 4], p: &[u8]) -> Result<Prop, Error> {
    let mut r = Rd::new(p);
    Ok(match t {
        b"av1C" => Prop::Av1c,
        b"colr" => match r.bytes(4)? {
            b"nclx" => Prop::Nclx { cp: r.u16()?, tc: r.u16()?, mc: r.u16()?, full: r.u8()? & 0x80 != 0 },
            b"prof" | b"rICC" => Prop::Icc(p[4..].to_vec()),
            _ => Prop::Other,
        },
        b"irot" => Prop::Irot(r.u8()? & 3),
        b"imir" => Prop::Imir(r.u8()? & 1),
        b"auxC" => {
            r.full()?;
            Prop::AuxC(r.cstr()?.to_vec())
        }
        _ => Prop::Other,
    })
}

/// Pillow's `irot_imir_to_exif_orientation` (_avif.c): the transforms as an EXIF orientation.
fn orientation(irot: Option<u8>, imir: Option<u8>) -> u32 {
    match (irot.unwrap_or(0), imir) {
        (1, Some(ax)) => if ax != 0 { 7 } else { 5 },
        (1, None) => 8,
        (2, Some(ax)) => if ax != 0 { 4 } else { 2 },
        (2, None) => 3,
        (3, Some(ax)) => if ax != 0 { 5 } else { 7 },
        (3, None) => 6,
        (_, Some(ax)) => if ax != 0 { 2 } else { 4 },
        _ => 1,
    }
}

// ---- AV1 (rav1d) --------------------------------------------------------------------------------------

/// A decoded AV1 frame: planes as u16 samples, its layout and colour signalling.
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub depth: u32,
    /// 400, 420, 422, 444
    pub layout: u32,
    pub planes: [Vec<u16>; 3],
    /// (primaries, transfer, matrix, full range) from the sequence header
    pub cicp: (u16, u16, u16, bool),
}

pub fn decode_av1(obus: &[u8]) -> Result<Frame, Error> {
    use rav1d::include::dav1d::data::Dav1dData;
    use rav1d::include::dav1d::dav1d::Dav1dSettings;
    use rav1d::include::dav1d::headers::*;
    use rav1d::include::dav1d::picture::Dav1dPicture;
    use rav1d::src::lib::*;
    use std::ptr::NonNull;
    unsafe {
        let mut s: Dav1dSettings = std::mem::zeroed();
        dav1d_default_settings(NonNull::from(&mut s));
        s.n_threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16)) as i32;
        s.max_frame_delay = 1;
        s.all_layers = 0;
        let mut ctx = None;
        let r = dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut s)));
        if r.0 < 0 || ctx.is_none() {
            return Err(bad(format!("decoder: open {}", r.0)));
        }
        let mut data = Dav1dData::default();
        let p = dav1d_data_create(Some(NonNull::from(&mut data)), obus.len());
        if p.is_null() {
            dav1d_close(Some(NonNull::from(&mut ctx)));
            return Err(bad("decoder: out of memory"));
        }
        std::ptr::copy_nonoverlapping(obus.as_ptr(), p, obus.len());
        let mut pic: Dav1dPicture = std::mem::zeroed();
        let mut got = false;
        let mut err = None;
        for _ in 0..10_000 {
            if data.sz > 0 {
                let r = dav1d_send_data(ctx.clone(), Some(NonNull::from(&mut data)));
                if r.0 < 0 && r.0 != -(libc_eagain()) {
                    err = Some(format!("send {}", r.0));
                    break;
                }
            }
            let r = dav1d_get_picture(ctx.clone(), Some(NonNull::from(&mut pic)));
            if r.0 == 0 {
                got = true;
                break;
            }
            if r.0 != -(libc_eagain()) {
                err = Some(format!("decode {}", r.0));
                break;
            }
            if data.sz == 0 {
                // flushed: one more round lets frame threads finish
                let r = dav1d_get_picture(ctx.clone(), Some(NonNull::from(&mut pic)));
                got = r.0 == 0;
                if !got {
                    err = Some("no picture".into());
                }
                break;
            }
        }
        if data.sz > 0 {
            rav1d::src::lib::dav1d_data_unref(Some(NonNull::from(&mut data)));
        }
        let out = if got {
            let (w, h, depth) = (pic.p.w as usize, pic.p.h as usize, pic.p.bpc as u32);
            let lay = pic.p.layout;
            let layout = match lay {
                l if l == DAV1D_PIXEL_LAYOUT_I400 => 400,
                l if l == DAV1D_PIXEL_LAYOUT_I420 => 420,
                l if l == DAV1D_PIXEL_LAYOUT_I422 => 422,
                _ => 444,
            };
            let (cw, ch) = match layout {
                420 => (w.div_ceil(2), h.div_ceil(2)),
                422 => (w.div_ceil(2), h),
                400 => (0, 0),
                _ => (w, h),
            };
            let plane = |i: usize, pw: usize, ph: usize| -> Vec<u16> {
                let Some(base) = pic.data[i] else { return vec![] };
                let stride = pic.stride[if i == 0 { 0 } else { 1 }];
                let mut v = Vec::with_capacity(pw * ph);
                for y in 0..ph {
                    let row = (base.as_ptr() as *const u8).offset(stride * y as isize);
                    if depth > 8 {
                        v.extend_from_slice(std::slice::from_raw_parts(row as *const u16, pw));
                    } else {
                        v.extend(std::slice::from_raw_parts(row, pw).iter().map(|&b| b as u16));
                    }
                }
                v
            };
            let planes = [plane(0, w, h), plane(1, cw, ch), plane(2, cw, ch)];
            let cicp = match pic.seq_hdr {
                Some(sh) => {
                    let sh = sh.as_ref();
                    (sh.pri as u16, sh.trc as u16, sh.mtrx as u16, sh.color_range != 0)
                }
                None => (2, 2, 2, false),
            };
            dav1d_picture_unref(Some(NonNull::from(&mut pic)));
            Ok(Frame { w, h, depth, layout, planes, cicp })
        } else {
            Err(bad(format!("decoder: {}", err.unwrap_or_default())))
        };
        dav1d_close(Some(NonNull::from(&mut ctx)));
        out
    }
}

fn libc_eagain() -> i32 {
    if cfg!(windows) {
        11
    } else if cfg!(any(target_os = "macos", target_os = "ios", target_os = "freebsd")) {
        35
    } else {
        11
    }
}

// ---- YUV -> RGB (libavif's built-in conversion) ---------------------------------------------------------

/// Kr, Kb of a matrix (libavif's table; unspecified / unknown: BT.601).
fn coefficients(mc: u16) -> (f32, f32) {
    match mc {
        1 => (0.2126, 0.0722),
        4 => (0.30, 0.11),
        5 | 6 => (0.299, 0.114),
        7 => (0.212, 0.087),
        9 | 10 => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    }
}

// ---- YUV -> RGB as libyuv does it (Pillow's libavif uses libyuv where libyuv has the conversion) --------

/// libyuv's YuvConstants (x86 layout): UB, UG, VG, VR, YG, YB.
#[derive(Clone, Copy)]
struct Yc {
    ub: i32,
    ug: i32,
    vg: i32,
    vr: i32,
    yg: i32,
    yb: i32,
}

const I601: Yc = Yc { ub: 128, ug: 25, vg: 52, vr: 102, yg: 18997, yb: -1160 };
const JPEG: Yc = Yc { ub: 113, ug: 22, vg: 46, vr: 90, yg: 16320, yb: 32 };
const H709: Yc = Yc { ub: 128, ug: 14, vg: 34, vr: 115, yg: 18997, yb: -1160 };
const F709: Yc = Yc { ub: 119, ug: 12, vg: 30, vr: 101, yg: 16320, yb: 32 };
const B2020: Yc = Yc { ub: 128, ug: 12, vg: 42, vr: 107, yg: 19003, yb: -1160 };
const V2020: Yc = Yc { ub: 120, ug: 11, vg: 37, vr: 94, yg: 16320, yb: 32 };

/// libavif's getLibYUVConstants: the constants for (primaries, matrix, range); None: libavif's own math.
fn libyuv_constants(cp: u16, mc: u16, full: bool) -> Option<Yc> {
    let (k709, k601, k2020) = if full { (F709, JPEG, V2020) } else { (H709, I601, B2020) };
    match mc {
        1 => Some(k709),
        5 | 6 | 2 => Some(k601),
        9 => Some(k2020),
        12 => match cp {
            1 | 2 => Some(k709),
            5 | 6 => Some(k601),
            9 => Some(k2020),
            _ => None,
        },
        _ => None,
    }
}

/// libyuv's YuvPixel (x86 C reference): y32 = Y scaled to 16 bits, 8-bit U / V → 8-bit R, G, B.
#[inline]
fn yuv_pixel(y32: u32, u: u8, v: u8, k: &Yc) -> [u8; 3] {
    let y1 = ((y32.wrapping_mul(k.yg as u32)) >> 16) as i32 + k.yb;
    let (ui, vi) = (u as i32 - 128, v as i32 - 128);
    let b = y1 + ui * k.ub;
    let g = y1 - (ui * k.ug + vi * k.vg);
    let r = y1 + vi * k.vr;
    let c = |x: i32| (x >> 6).clamp(0, 255) as u8;
    [c(r), c(g), c(b)]
}

/// libyuv's ScaleRowUp2_Linear_Any_C (also the 16-bit one): a chroma row to `w` samples.
fn up_linear(src: &[u16], w: usize, out: &mut [u16]) {
    out[0] = src[0];
    let work = (w - 1) & !1;
    for i in 0..work / 2 {
        let (a, b) = (src[i] as u32, src[i + 1] as u32);
        out[1 + 2 * i] = ((a * 3 + b + 2) >> 2) as u16;
        out[2 + 2 * i] = ((a + b * 3 + 2) >> 2) as u16;
    }
    out[w - 1] = src[(w - 1) / 2];
}

/// libyuv's ScaleRowUp2_Bilinear_Any_C: two chroma rows s, t → output rows d (nearer s) and e (nearer t).
fn up_bilinear(s: &[u16], t: &[u16], w: usize, d: &mut [u16], e: &mut [u16]) {
    let (s0, t0) = (s[0] as u32, t[0] as u32);
    d[0] = ((3 * s0 + t0 + 2) >> 2) as u16;
    e[0] = ((s0 + 3 * t0 + 2) >> 2) as u16;
    let work = (w - 1) & !1;
    for x in 0..work / 2 {
        let (a, b, c, dd) = (s[x] as u32, s[x + 1] as u32, t[x] as u32, t[x + 1] as u32);
        d[2 * x + 1] = ((a * 9 + b * 3 + c * 3 + dd + 8) >> 4) as u16;
        d[2 * x + 2] = ((a * 3 + b * 9 + c + dd * 3 + 8) >> 4) as u16;
        e[2 * x + 1] = ((a * 3 + b + c * 9 + dd * 3 + 8) >> 4) as u16;
        e[2 * x + 2] = ((a + b * 3 + c * 3 + dd * 9 + 8) >> 4) as u16;
    }
    let l = (w - 1) / 2;
    let (sl, tl) = (s[l] as u32, t[l] as u32);
    d[w - 1] = ((3 * sl + tl + 2) >> 2) as u16;
    e[w - 1] = ((sl + 3 * tl + 2) >> 2) as u16;
}

/// The chroma of output row `y`, upsampled to the full width, as libyuv's I420 / I422 "Filter"
/// functions build it (bilinear between rows for 4:2:0, linear along the row).
fn chroma_row(pl: &[u16], cw: usize, ch: usize, w: usize, h: usize, layout: u32, y: usize, out: &mut [u16]) {
    let row = |r: usize| &pl[r * cw..r * cw + cw];
    match layout {
        444 => out.copy_from_slice(row(y)),
        422 => up_linear(row(y), w, out),
        _ => {
            // row 0 and (even heights) the last row: one chroma row; between them pairs of rows
            if y == 0 || (h % 2 == 0 && y == h - 1) {
                up_linear(row(if y == 0 { 0 } else { ch - 1 }), w, out);
            } else {
                let k = (y - 1) / 2;
                let mut other = vec![0u16; w];
                if (y - 1) % 2 == 0 {
                    up_bilinear(row(k), row(k + 1), w, out, &mut other);
                } else {
                    up_bilinear(row(k), row(k + 1), w, &mut other, out);
                }
            }
        }
    }
}

/// libavif's libyuv path. `None` when libyuv has no conversion for this frame (libavif's own math then).
fn to_rgb_libyuv(f: &Frame, k: Yc, alpha: Option<&Frame>) -> Option<Vec<u8>> {
    use rayon::prelude::*;
    if f.layout == 400 || !matches!(f.depth, 8 | 10 | 12) {
        return None;
    }
    let (w, h) = (f.w, f.h);
    let c = if alpha.is_some() { 4 } else { 3 };
    let (cw, ch) = match f.layout {
        420 => (w.div_ceil(2), h.div_ceil(2)),
        422 => (w.div_ceil(2), h),
        _ => (w, h),
    };
    // 10-bit with alpha: libyuv's 10-bit functions (Y at full precision, U / V / A shifted to 8 bits);
    // otherwise everything goes down to 8 bits first (libavif's avifImageDownshiftTo8bpc)
    let hbd = alpha.is_some() && f.depth == 10;
    let down = |v: u16| -> u16 { if f.depth == 8 { v } else { (((v as u32) << (24 - f.depth)) >> 16).min(255) as u16 } };
    let low: [Vec<u16>; 2] = if hbd || f.depth == 8 {
        [vec![], vec![]]
    } else {
        [f.planes[1].iter().map(|&x| down(x)).collect(), f.planes[2].iter().map(|&x| down(x)).collect()]
    };
    let mut out = vec![0u8; w * h * c];
    out.par_chunks_mut(w * c).enumerate().for_each(|(y, row)| {
        let (mut u, mut v) = (vec![0u16; w], vec![0u16; w]);
        let (a, b) = if hbd || f.depth == 8 { (&f.planes[1][..], &f.planes[2][..]) } else { (&low[0][..], &low[1][..]) };
        chroma_row(a, cw, ch, w, h, f.layout, y, &mut u);
        chroma_row(b, cw, ch, w, h, f.layout, y, &mut v);
        let yr = &f.planes[0][y * w..y * w + w];
        for x in 0..w {
            let (y32, uu, vv) = if hbd {
                let yy = yr[x] as u32;
                ((yy << 6) | (yy >> 4), (u[x] >> 2).min(255) as u8, (v[x] >> 2).min(255) as u8)
            } else {
                let yy = down(yr[x]) as u32;
                (yy * 0x0101, u[x] as u8, v[x] as u8)
            };
            let p = yuv_pixel(y32, uu, vv, &k);
            row[x * c..x * c + 3].copy_from_slice(&p);
            if let Some(a) = alpha {
                let av = a.planes[0][y * w + x];
                row[x * c + 3] = if hbd { (av >> 2).min(255) as u8 } else { down(av) as u8 };
            }
        }
    });
    Some(out)
}

/// The frame as 8-bit RGB (and the alpha frame as the 4th channel): libyuv's conversion where Pillow's
/// libavif uses it, else libavif's own.
pub fn convert(f: &Frame, cp: u16, mc: u16, full: bool, alpha: Option<&Frame>) -> Result<Vec<u8>, Error> {
    if let Some(a) = alpha {
        if a.w != f.w || a.h != f.h || a.depth != f.depth {
            return Err(unsupported("alpha of another size or depth"));
        }
    }
    if let Some(k) = libyuv_constants(cp, mc, full) {
        if let Some(v) = to_rgb_libyuv(f, k, alpha) {
            return Ok(v);
        }
    }
    to_rgb(f, mc, full, alpha)
}

/// The frame as 8-bit RGB (and the alpha frame as the 4th channel): libavif's own conversion.
pub fn to_rgb(f: &Frame, mc: u16, full: bool, alpha: Option<&Frame>) -> Result<Vec<u8>, Error> {
    use rayon::prelude::*;
    let (w, h) = (f.w, f.h);
    let c = if alpha.is_some() { 4 } else { 3 };
    let max = ((1u32 << f.depth) - 1) as f32;
    let sh = f.depth - 8;
    let (bias_y, range_y) = if full { (0.0, max) } else { ((16u32 << sh) as f32, (219u32 << sh) as f32) };
    let (bias_uv, range_uv) = ((1u32 << (f.depth - 1)) as f32, if full { max } else { (224u32 << sh) as f32 });
    let ty: Vec<f32> = (0..=max as u32).map(|i| (i as f32 - bias_y) / range_y).collect();
    let tuv: Vec<f32> = (0..=max as u32).map(|i| (i as f32 - bias_uv) / range_uv).collect();
    let (kr, kb) = coefficients(mc);
    let kg = 1.0 - kr - kb;
    let identity = mc == 0;
    let (sx, sy) = match f.layout {
        420 => (1, 1),
        422 => (1, 0),
        _ => (0, 0),
    };
    let cw = if f.layout == 400 { 0 } else { w.div_ceil(1 << sx) };
    let chh = if f.layout == 400 { 0 } else { h.div_ceil(1 << sy) };
    let q = |v: f32| -> u8 { (0.5 + v.clamp(0.0, 1.0) * 255.0) as u8 };
    let mut out = vec![0u8; w * h * c];
    let alpha_px = match alpha {
        Some(a) if a.w == w && a.h == h => {
            let amax = ((1u32 << a.depth) - 1) as f32;
            let (ab, ar) = if a.cicp.3 { (0.0, amax) } else { ((16u32 << (a.depth - 8)) as f32, (219u32 << (a.depth - 8)) as f32) };
            Some(a.planes[0].iter().map(|&v| q((v as f32 - ab) / ar)).collect::<Vec<u8>>())
        }
        Some(_) => return Err(unsupported("alpha of another size")),
        None => None,
    };
    let cmax = max as u16;
    out.par_chunks_mut(w * c).enumerate().for_each(|(y, row)| {
        let uy = y >> sy;
        // bilinear chroma: the nearer neighbour gets 3/4 (libavif's built-in upsampling)
        let uy2 = if sy == 0 { uy } else if y & 1 == 1 { (uy + 1).min(chh.saturating_sub(1)) } else { uy.saturating_sub(1) };
        for x in 0..w {
            let yv = ty[f.planes[0][y * w + x].min(cmax) as usize];
            let (cb, cr) = if f.layout == 400 {
                (0.0, 0.0)
            } else {
                let ux = x >> sx;
                let ux2 = if sx == 0 { ux } else if x & 1 == 1 { (ux + 1).min(cw - 1) } else { ux.saturating_sub(1) };
                let s = |pl: &Vec<u16>| -> f32 {
                    let g = |yy: usize, xx: usize| tuv[pl[yy * cw + xx].min(cmax) as usize];
                    match (sx, sy) {
                        (0, 0) => g(uy, ux),
                        (1, 0) => g(uy, ux) * 0.75 + g(uy, ux2) * 0.25,
                        _ => (g(uy, ux) * 0.75 + g(uy, ux2) * 0.25) * 0.75 + (g(uy2, ux) * 0.75 + g(uy2, ux2) * 0.25) * 0.25,
                    }
                };
                (s(&f.planes[1]), s(&f.planes[2]))
            };
            let (r, g, b) = if identity {
                (cr + 0.5, yv, cb + 0.5)
            } else if f.layout == 400 {
                (yv, yv, yv)
            } else {
                let r = yv + (2.0 * (1.0 - kr)) * cr;
                let b = yv + (2.0 * (1.0 - kb)) * cb;
                let g = yv - ((2.0 * ((kr * (1.0 - kr) * cr) + (kb * (1.0 - kb) * cb))) / kg);
                (r, g, b)
            };
            let o = &mut row[x * c..x * c + c];
            o[0] = q(r);
            o[1] = q(g);
            o[2] = q(b);
            if let Some(a) = &alpha_px {
                o[3] = a[y * w + x];
            }
        }
    });
    Ok(out)
}

/// Open an AVIF file: upright RGB / RGBA (as Pillow's AvifImagePlugin + exif_transpose), its ICC profile.
pub fn decode(data: &[u8]) -> Result<(Image, u32), Error> {
    let ct = parse(data)?;
    let f = decode_av1(&ct.color)?;
    let a = match &ct.alpha {
        Some(d) => Some(decode_av1(d)?),
        None => None,
    };
    // the container's nclx wins over the sequence header's (libavif)
    let (cp, mc, full) = match ct.nclx {
        Some((cp, _, mc, full)) => (cp, mc, full),
        None => (f.cicp.0, f.cicp.2, f.cicp.3),
    };
    let px = convert(&f, cp, mc, full, a.as_ref())?;
    let img = Image { w: f.w as u32, h: f.h as u32, channels: if a.is_some() { 4 } else { 3 }, px, icc: ct.icc, format: "avif" };
    Ok((img, ct.orientation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientations() {
        assert_eq!(orientation(None, None), 1);
        assert_eq!(orientation(Some(1), None), 8);
        assert_eq!(orientation(Some(3), Some(1)), 5);
        assert_eq!(orientation(None, Some(0)), 4);
    }
}
