//! The per-pixel edit chain of `apply_edit` in fastedit/edit.py, fused into one loop.
//!
//! Stage order (same as edit.py): point correction -> Temp / Tint -> Contrast / Pivot ->
//! Shadows, Highlights -> Color Boost -> Saturation -> Filter Saturation / Warmth -> Film ->
//! per-channel LUTs. Every stage ends like NumPy's `np.clip(x, 0, 255).astype(np.uint8)`:
//! clamp, then truncate. Expressions keep NumPy's evaluation order (left to right), because
//! f64 addition is not associative and the result must be bit-identical.
//!
//! Perceptual Saturation (`bw.ps`, ps.rs) and Super Contrast (`sc`, sc.rs) need whole-image
//! statistics, so `apply` runs extra passes for them. Mid/Detail comes first (apply_mid_detail in
//! edit.py); its smoothed luma (guided filter) is computed outside and passed in.

/// A tone table on luma (Shadows / Highlights), indexed by `floor(y * 16 + 0.5)`, 4096 entries.
pub struct Tone<'a> {
    pub table: &'a [f64],
    /// lift (`rgb *= y'/y`) instead of darken (`rgb += y' - y`)
    pub ratio: bool,
}

/// The Filter: Saturation / Warmth, optionally Perceptual Saturation.
pub struct Bw<'a> {
    pub s: f64,
    pub op: f64,
    pub w: Option<[f64; 3]>,
    /// Perceptual Saturation: amount (bw.ps) and the Gaussian kernel over the grid (bw.psk)
    pub ps: Option<(f64, &'a [f64])>,
}

/// Super Contrast (sc_finish in edit.py).
pub struct Sc<'a> {
    pub cast: f64,
    pub cc: f64,
    pub dyn_: f64,
    pub op: f64,
    /// Gaussian kernel over the Dynamic Contrast grid (needed when dyn_ > 0)
    pub dk: &'a [f64],
    /// shared Shadows / Highlights table, 256 entries
    pub shl: Option<&'a [f64]>,
}

pub struct Film<'a> {
    /// grid size of the 3D table (33)
    pub n: usize,
    /// n^3 x 3 deltas (r major, then g, then b), or none (tone curve only)
    pub d: Option<&'a [f64]>,
    /// Brightness / Contrast tone table, 256 entries
    pub t1: Option<&'a [f64]>,
    pub op: f64,
}

pub struct Edit<'a> {
    /// Mid/Detail: amount (-1..1) and the smoothed luma, one f32 per pixel (md_base in edit.py)
    pub md: Option<(f64, &'a [f32])>,
    pub point: bool,
    pub k: f64,
    pub o: f64,
    pub gains: Option<[f64; 3]>,
    /// rows = output channel: out_c = wb[c][0] r + wb[c][1] g + wb[c][2] b
    pub wb: Option<[[f64; 3]; 3]>,
    /// contrast table, 4096 entries (rgb * 16 and luma * 16 index into it)
    pub con: Option<&'a [f64]>,
    pub tones: Vec<Tone<'a>>,
    /// gain by saturation (max - min), 256 entries
    pub boost: Option<&'a [f64]>,
    pub sat: Option<f64>,
    pub bw: Option<Bw<'a>>,
    pub film: Option<Film<'a>>,
    pub sc: Option<Sc<'a>>,
    pub luts: [[u8; 256]; 3],
    /// Sharpen tool, after everything else (sharp.rs); off when not active
    pub sharp: crate::sharp::Sharp,
}

impl Edit<'_> {
    /// Table sizes, so the pixel loop can index without surprises. Returns a message on error.
    pub fn validate(&self) -> Result<(), String> {
        let need = |name: &str, t: &[f64], n: usize| {
            if t.len() < n {
                Err(format!("{name}: {} values, need {n}", t.len()))
            } else {
                Ok(())
            }
        };
        if let Some(c) = self.con {
            need("con", c, 4096)?;
        }
        for t in &self.tones {
            need("tones", t.table, 4096)?;
        }
        if let Some(b) = self.boost {
            need("boost", b, 256)?;
        }
        if let Some(f) = &self.film {
            if f.n < 2 {
                return Err("film: n must be at least 2".into());
            }
            if let Some(d) = f.d {
                let want = f.n * f.n * f.n * 3;
                if d.len() != want {
                    return Err(format!("film: d has {} values, need {want}", d.len()));
                }
            }
            if let Some(t) = f.t1 {
                need("film t1", t, 256)?;
            }
        }
        if let Some(Bw { ps: Some((_, k)), .. }) = &self.bw {
            if k.is_empty() {
                return Err("bw psk: empty kernel".into());
            }
        }
        if let Some(sc) = &self.sc {
            if sc.dyn_ > 0.0 && sc.dk.is_empty() {
                return Err("sc dk: empty kernel".into());
            }
            if let Some(t) = sc.shl {
                need("sc shl", t, 256)?;
            }
        }
        Ok(())
    }
}

pub const MD_GAIN: f64 = 0.952;
pub const MD_K: f64 = 0.706;
pub const MD_GAMMA: f64 = 0.662;

/// Mid/Detail for one pixel (apply_mid_detail in edit.py): detail = luma - smoothed luma,
/// added to all channels, faded out in the shadows. libm's pow, the same in every build.
#[inline(always)]
pub fn md_px(px: [u8; 3], amount: f64, base: f32) -> [u8; 3] {
    let f = [px[0] as f64, px[1] as f64, px[2] as f64];
    let y = (0.2126 * f[0] + 0.7152 * f[1] + 0.0722 * f[2]) / 255.0;
    let w = libm::pow(y.max(0.0) / MD_K, MD_GAMMA).min(1.0);
    let d = amount * MD_GAIN * w * (y - base as f64) * 255.0;
    [u8c(f[0] + d), u8c(f[1] + d), u8c(f[2] + d)]
}

/// rgb_luma01 in edit.py: luma 0..1 in float32 arithmetic (NumPy keeps float32 there)
pub fn luma01(rgb: &[u8]) -> Vec<f32> {
    rgb.chunks_exact(3)
        .map(|p| (0.2126f32 * p[0] as f32 + 0.7152f32 * p[1] as f32 + 0.0722f32 * p[2] as f32) / 255.0f32)
        .collect()
}

#[inline(always)]
pub(crate) fn luma(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// np.clip(x, 0, 255).astype(np.uint8)  (clamp keeps NaN like np.clip; NaN as u8 = 0)
#[inline(always)]
pub(crate) fn u8c(x: f64) -> u8 {
    x.clamp(0.0, 255.0) as u8
}

/// np.minimum(np.floor(y * 16 + 0.5), 4095).astype(np.intp)   (y >= 0 here)
#[inline(always)]
pub(crate) fn t16(y: f64) -> usize {
    (y * 16.0 + 0.5).floor().min(4095.0) as usize
}

/// np.clip(np.floor(x + 0.5), 0, 255)   (still a float)
#[inline(always)]
pub(crate) fn round_clip(x: f64) -> f64 {
    (x + 0.5).floor().clamp(0.0, 255.0)
}

/// film_apply() for one pixel
#[inline(always)]
pub(crate) fn film_px(p: [u8; 3], fm: &Film) -> [u8; 3] {
    let f = [p[0] as f64, p[1] as f64, p[2] as f64];
    let mut q = f;
    if let Some(d) = fm.d {
        let n = fm.n;
        let nm1 = (n - 1) as f64;
        let top = (n - 2) as f64;
        let mut i = [0usize; 3];
        let mut fr = [0f64; 3];
        for c in 0..3 {
            let g = f[c] * nm1 / 255.0;
            let ic = g.floor().min(top);
            i[c] = ic as usize;
            fr[c] = g - ic;
        }
        let mut acc = [0f64; 3];
        for dr in 0..2 {
            let wr = if dr == 1 { fr[0] } else { 1.0 - fr[0] };
            for dg in 0..2 {
                let wg = if dg == 1 { fr[1] } else { 1.0 - fr[1] };
                for db in 0..2 {
                    let wb = if db == 1 { fr[2] } else { 1.0 - fr[2] };
                    let w = wr * wg * wb;
                    let k = (((i[0] + dr) * n + (i[1] + dg)) * n + (i[2] + db)) * 3;
                    acc[0] += w * d[k];
                    acc[1] += w * d[k + 1];
                    acc[2] += w * d[k + 2];
                }
            }
        }
        for c in 0..3 {
            q[c] = round_clip(f[c] + acc[c]);
        }
    }
    if let Some(t) = fm.t1 {
        for v in q.iter_mut() {
            *v = t[*v as usize];
        }
    }
    if fm.op != 1.0 {
        for c in 0..3 {
            q[c] = round_clip(f[c] + (q[c] - f[c]) * fm.op);
        }
    }
    [q[0] as u8, q[1] as u8, q[2] as u8]
}

/// Point correction for one pixel: luma levels (k, o), then the neutral-gray gains.
#[inline(always)]
pub fn point_px(px: [u8; 3], k: f64, o: f64, gains: Option<[f64; 3]>) -> [u8; 3] {
    let (mut fr, mut fg, mut fb) = (px[0] as f64, px[1] as f64, px[2] as f64);
    if k != 1.0 || o != 0.0 {
        let lum = luma(fr, fg, fb);
        let bl = (lum * k + o).clamp(0.0, 255.0);
        let ra = bl / lum.max(1.0);
        fr *= ra;
        fg *= ra;
        fb *= ra;
    }
    if let Some(gn) = gains {
        fr *= gn[0];
        fg *= gn[1];
        fb *= gn[2];
    }
    [u8c(fr), u8c(fg), u8c(fb)]
}

/// The Adjustments tool (one step of the chain). Mid/Detail (`md`, -1..1) runs first, on a
/// smoothed luma the chain computes from the image as it arrives at this step.
pub struct Adjust<'a> {
    pub md: f64,
    /// rows = output channel: out_c = wb[c][0] r + wb[c][1] g + wb[c][2] b
    pub wb: Option<[[f64; 3]; 3]>,
    /// contrast table, 4096 entries (rgb * 16 and luma * 16 index into it)
    pub con: Option<&'a [f64]>,
    pub tones: Vec<Tone<'a>>,
    /// gain by saturation (max - min), 256 entries
    pub boost: Option<&'a [f64]>,
    pub sat: Option<f64>,
}

impl Adjust<'_> {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(c) = self.con {
            if c.len() < 4096 {
                return Err(format!("con: {} values, need 4096", c.len()));
            }
        }
        for t in &self.tones {
            if t.table.len() < 4096 {
                return Err(format!("tones: {} values, need 4096", t.table.len()));
            }
        }
        if let Some(b) = self.boost {
            if b.len() < 256 {
                return Err(format!("boost: {} values, need 256", b.len()));
            }
        }
        Ok(())
    }
}

/// Point correction ... Saturation: everything before the Filter (the fixed order of `Edit`).
#[inline(always)]
pub fn pre_filter(px: [u8; 3], e: &Edit) -> [u8; 3] {
    let px = if e.point { point_px(px, e.k, e.o, e.gains) } else { px };
    adjust_parts(px, &e.wb, e.con, &e.tones, e.boost, e.sat)
}

/// Adjustments for one pixel, without Mid/Detail (the chain applies md_px first when md != 0).
#[inline(always)]
pub fn adjust_px(px: [u8; 3], a: &Adjust) -> [u8; 3] {
    adjust_parts(px, &a.wb, a.con, &a.tones, a.boost, a.sat)
}

#[inline(always)]
fn adjust_parts(px: [u8; 3], wb: &Option<[[f64; 3]; 3]>, con: Option<&[f64]>, tones: &[Tone], boost: Option<&[f64]>,
                sat: Option<f64>) -> [u8; 3] {
    let [mut r, mut g, mut b] = px;

    if let Some(w) = wb {
        let (fr, fg, fb) = (r as f64, g as f64, b as f64);
        let o0 = fr * w[0][0] + fg * w[0][1] + fb * w[0][2];
        let o1 = fr * w[1][0] + fg * w[1][1] + fb * w[1][2];
        let o2 = fr * w[2][0] + fg * w[2][1] + fb * w[2][2];
        (r, g, b) = (u8c(o0), u8c(o1), u8c(o2));
    }

    if let Some(con) = con {
        let c0 = con[r as usize * 16];
        let c1 = con[g as usize * 16];
        let c2 = con[b as usize * 16];
        let y = luma(r as f64, g as f64, b as f64);
        let d = con[t16(y)] - (0.2126 * c0 + 0.7152 * c1 + 0.0722 * c2);
        (r, g, b) = (u8c(c0 + d), u8c(c1 + d), u8c(c2 + d));
    }

    for t in tones {
        let (fr, fg, fb) = (r as f64, g as f64, b as f64);
        let y = luma(fr, fg, fb);
        let y2 = t.table[t16(y)];
        if t.ratio {
            let q = y2 / y.max(0.5);
            (r, g, b) = (u8c(fr * q), u8c(fg * q), u8c(fb * q));
        } else {
            let d = y2 - y;
            (r, g, b) = (u8c(fr + d), u8c(fg + d), u8c(fb + d));
        }
    }

    if let Some(bt) = boost {
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        let gn = bt[(mx - mn) as usize];
        let (fr, fg, fb) = (r as f64, g as f64, b as f64);
        let y = luma(fr, fg, fb);
        (r, g, b) = (u8c(y + (fr - y) * gn), u8c(y + (fg - y) * gn), u8c(y + (fb - y) * gn));
    }

    if let Some(s) = sat {
        let (fr, fg, fb) = (r as f64, g as f64, b as f64);
        let y = luma(fr, fg, fb);
        (r, g, b) = (u8c(y + (fr - y) * s), u8c(y + (fg - y) * s), u8c(y + (fb - y) * s));
    }

    [r, g, b]
}

/// The Filter's Warmth / Saturation result before opacity: `fl` per channel (not clipped).
#[inline(always)]
pub(crate) fn filter_fl(px: [u8; 3], bw: &Bw) -> [f64; 3] {
    let f = [px[0] as f64, px[1] as f64, px[2] as f64];
    let w = match bw.w {
        Some(wg) => [(f[0] * wg[0]).floor().min(255.0), (f[1] * wg[1]).floor().min(255.0), (f[2] * wg[2]).floor().min(255.0)],
        None => f,
    };
    let y = (w[0] + 2.0 * w[1] + w[2]) / 4.0;
    [y + (w[0] - y) * bw.s, y + (w[1] - y) * bw.s, y + (w[2] - y) * bw.s]
}

/// The Filter without Perceptual Saturation: Warmth / Saturation, then opacity.
#[inline(always)]
pub(crate) fn filter_px(px: [u8; 3], bw: &Bw) -> [u8; 3] {
    let fl = filter_fl(px, bw);
    let mut o = [0u8; 3];
    for c in 0..3 {
        let f = px[c] as f64;
        let l = fl[c].clamp(0.0, 255.0);
        o[c] = round_clip(f + (l - f) * bw.op) as u8;
    }
    o
}

/// One pixel through the whole chain (without the final LUTs); not for Perceptual Saturation.
#[inline(always)]
pub fn pixel(px: [u8; 3], e: &Edit) -> [u8; 3] {
    let mut p = pre_filter(px, e);
    if let Some(bw) = &e.bw {
        p = filter_px(p, bw);
    }
    if let Some(fm) = &e.film {
        p = film_px(p, fm);
    }
    p
}

pub(crate) fn identity_luts() -> [[u8; 256]; 3] {
    let id: [u8; 256] = core::array::from_fn(|i| i as u8);
    [id, id, id]
}

/// Whole image, RGB in and out (what Process writes).
pub fn apply(src: &[u8], dst: &mut [u8], width: usize, e: &Edit) {
    apply_ex(src, dst, width, 3, e, false);
}

/// Whole image: `src` is RGB, `dst` has `ob` bytes per pixel (3 = RGB, 4 = RGBA with alpha 255),
/// `width` pixels per row. Same order as apply_edit: the chain (Perceptual Saturation needs a
/// second pass), then Super Contrast on the result; the curves come last. With `want_hist` the
/// UI's histograms are counted in whichever pass writes the final pixels.
pub fn apply_ex(src: &[u8], dst: &mut [u8], width: usize, ob: usize, e: &Edit, want_hist: bool) -> Option<crate::Hist> {
    assert!(ob == 3 || ob == 4, "ob must be 3 or 4");
    let (row, orow) = (width * 3, width * ob);
    if row == 0 {
        return want_hist.then(crate::Hist::default);
    }
    assert!(src.len().is_multiple_of(row), "src is not whole rows");
    let height = src.len() / row;
    assert_eq!(dst.len(), height * orow, "dst size");
    if let Some((_, b)) = e.md {
        assert_eq!(b.len(), width * height, "md base size");
    }
    let ident = identity_luts();
    let sc_on = e.sc.is_some();
    let luts = if sc_on { &ident } else { &e.luts };
    let hist_here = want_hist && !sc_on;           // with Super Contrast its final pass counts
    let h = match &e.bw {
        Some(bw @ Bw { ps: Some(_), .. }) => crate::ps::apply(src, dst, width, height, ob, e, bw, luts, hist_here),
        _ => crate::Hist::merge(crate::map_bands(dst, orow, |band, first_row| {
            let s = &src[first_row * row..first_row * row + band.len() / ob * 3];
            let mut hist = hist_here.then(crate::Hist::default);
            let p0 = first_row * width;
            for (k, (o, i)) in band.chunks_exact_mut(ob).zip(s.chunks_exact(3)).enumerate() {
                let mut px = [i[0], i[1], i[2]];
                if let Some((a, b)) = e.md {
                    px = md_px(px, a, b[p0 + k]);
                }
                crate::write_px(o, pixel(px, e), luts, &mut hist);
            }
            hist
        })),
    };
    let h = match &e.sc {
        Some(sc) => crate::sc::finish(dst, width, height, ob, sc, &e.luts, want_hist).1,
        None => h,
    };
    if !e.sharp.active() {
        return h;
    }
    // Sharpen works on the finished pixels; the output histograms are counted again afterwards
    crate::sharp::apply(dst, width, height, ob, &e.sharp);
    h.map(|mut h| {
        h.hout.fill(0);
        h.hmx.fill(0);
        h.hmn.fill(0);
        for p in dst.chunks_exact(ob) {
            h.hout[p[0] as usize] += 1;
            h.hout[256 + p[1] as usize] += 1;
            h.hout[512 + p[2] as usize] += 1;
            h.hmx[p[0].max(p[1]).max(p[2]) as usize] += 1;
            h.hmn[p[0].min(p[1]).min(p[2]) as usize] += 1;
        }
        h
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> [[u8; 256]; 3] {
        let id: [u8; 256] = core::array::from_fn(|i| i as u8);
        [id, id, id]
    }

    fn empty<'a>() -> Edit<'a> {
        Edit { md: None, point: false, k: 1.0, o: 0.0, gains: None, wb: None, con: None, tones: vec![], boost: None,
               sat: None, bw: None, film: None, sc: None, luts: ident(), sharp: Default::default() }
    }

    #[test]
    fn empty_edit_is_identity() {
        let e = empty();
        for v in [0u8, 1, 77, 254, 255] {
            assert_eq!(pixel([v, 255 - v, v / 2], &e), [v, 255 - v, v / 2]);
        }
    }

    #[test]
    fn saturation_zero_is_grey_truncated() {
        let mut e = empty();
        e.sat = Some(0.0);
        // y = 0.2126*200 + 0.7152*100 + 0.0722*50 = 117.66 -> 117 on every channel
        assert_eq!(pixel([200, 100, 50], &e), [117, 117, 117]);
    }

    #[test]
    fn validate_catches_short_tables() {
        let short = [0.0; 10];
        let mut e = empty();
        e.con = Some(&short);
        assert!(e.validate().is_err());
    }
}
