//! Edit math shared by the Python extension (Process, lossy compare preview) and the
//! WebAssembly live preview. Ported line by line from fastedit/edit.py; output must be
//! bit-identical to it (checked by tests/parity).
//!
//! Numerics rules: f64 where edit.py uses float64, same operation order, no fast-math,
//! rounding `floor(x + 0.5)`, truncation `as u8` after clamping, `libm` for pow. Speed may cost up
//! to 1 level where it pays off (px32.rs: Mid/Detail's weight table); preview and Process share
//! this code, so they stay identical to each other.
//!
//! Images are interleaved 8-bit, 3 (RGB) or 4 (RGBA, alpha left alone) bytes per pixel,
//! processed in bands of whole rows so the work can be split over threads.

pub mod chain;
pub mod edit;
pub mod md;
pub mod ps;
pub(crate) mod px32;
pub mod sc;
pub mod sharp;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A clock for the preview's timing breakdown (ms); set by the WebAssembly binding
/// (performance.now), none elsewhere.
static CLOCK: std::sync::OnceLock<fn() -> f64> = std::sync::OnceLock::new();
pub fn set_clock(f: fn() -> f64) {
    let _ = CLOCK.set(f);
}
pub(crate) fn now() -> f64 {
    CLOCK.get().map_or(0.0, |f| f())
}

/// Rows per band when splitting the image over threads.
pub const BAND_ROWS: usize = 64;

use core::sync::atomic::{AtomicBool, Ordering};

// Runtime switch for the thread pool: on for native builds; in WebAssembly only once the page
// has started the worker pool (set_parallel(true)), so the same .wasm also runs without one.
static PARALLEL: AtomicBool = AtomicBool::new(cfg!(not(target_arch = "wasm32")));

/// Turn the use of the thread pool on or off (no effect without the `parallel` feature).
pub fn set_parallel(on: bool) {
    PARALLEL.store(on, Ordering::Relaxed);
}

#[cfg(feature = "parallel")]
fn parallel() -> bool {
    PARALLEL.load(Ordering::Relaxed)
}

/// Number of worker threads the core uses (1 without the `parallel` feature or switched off).
pub fn threads() -> usize {
    #[cfg(feature = "parallel")]
    {
        if parallel() {
            return rayon::current_num_threads();
        }
    }
    1
}

/// Runs `f(band, first_row)` over bands of whole rows, in parallel when enabled.
/// `row_bytes` = width * bytes per pixel.
pub fn for_each_band<F>(buf: &mut [u8], row_bytes: usize, f: F)
where
    F: Fn(&mut [u8], usize) + Sync + Send,
{
    if row_bytes == 0 {
        return;
    }
    let chunk = row_bytes * BAND_ROWS;
    #[cfg(feature = "parallel")]
    {
        if parallel() {
            use rayon::prelude::*;
            buf.par_chunks_mut(chunk).enumerate().for_each(|(i, b)| f(b, i * BAND_ROWS));
            return;
        }
    }
    for (i, b) in buf.chunks_mut(chunk).enumerate() {
        f(b, i * BAND_ROWS);
    }
}

/// for_each_band for any element type and band height (rows of `row_len` elements).
pub fn for_each_band_t<T, F>(buf: &mut [T], row_len: usize, band_rows: usize, f: F)
where
    T: Send,
    F: Fn(&mut [T], usize) + Sync + Send,
{
    if row_len == 0 {
        return;
    }
    let chunk = row_len * band_rows;
    #[cfg(feature = "parallel")]
    {
        if parallel() {
            use rayon::prelude::*;
            buf.par_chunks_mut(chunk).enumerate().for_each(|(i, b)| f(b, i * band_rows));
            return;
        }
    }
    for (i, b) in buf.chunks_mut(chunk).enumerate() {
        f(b, i * band_rows);
    }
}

/// Like for_each_band, but each band returns a value (e.g. partial statistics); the values come
/// back in band order, so merging them is deterministic.
pub fn map_bands<T, F>(buf: &mut [u8], row_bytes: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(&mut [u8], usize) -> T + Sync + Send,
{
    if row_bytes == 0 {
        return Vec::new();
    }
    let chunk = row_bytes * BAND_ROWS;
    #[cfg(feature = "parallel")]
    {
        if parallel() {
            use rayon::prelude::*;
            return buf.par_chunks_mut(chunk).enumerate().map(|(i, b)| f(b, i * BAND_ROWS)).collect();
        }
    }
    buf.chunks_mut(chunk).enumerate().map(|(i, b)| f(b, i * BAND_ROWS)).collect()
}

/// `map_bands` for reading only.
pub fn map_ref_bands<T, F>(buf: &[u8], row_bytes: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(&[u8], usize) -> T + Sync + Send,
{
    if row_bytes == 0 {
        return Vec::new();
    }
    let chunk = row_bytes * BAND_ROWS;
    #[cfg(feature = "parallel")]
    {
        if parallel() {
            use rayon::prelude::*;
            return buf.par_chunks(chunk).enumerate().map(|(i, b)| f(b, i * BAND_ROWS)).collect();
        }
    }
    buf.chunks(chunk).enumerate().map(|(i, b)| f(b, i * BAND_ROWS)).collect()
}

/// The four histograms the UI draws, counted from the final pixels:
/// `hin` = before the curves (R, G, B: 3 x 256), `hout` = after them, `hmx` / `hmn` = max / min
/// channel of the output. Same counts as processPixels / psPass / scPass in edit-math.js.
#[derive(Clone)]
pub struct Hist {
    pub hin: Vec<u32>,
    pub hout: Vec<u32>,
    pub hmx: Vec<u32>,
    pub hmn: Vec<u32>,
}

impl Default for Hist {
    fn default() -> Self {
        Hist { hin: vec![0; 768], hout: vec![0; 768], hmx: vec![0; 256], hmn: vec![0; 256] }
    }
}

impl Hist {
    #[inline(always)]
    pub fn add(&mut self, pre: [u8; 3], out: [u8; 3]) {
        self.hin[pre[0] as usize] += 1;
        self.hin[256 + pre[1] as usize] += 1;
        self.hin[512 + pre[2] as usize] += 1;
        self.hout[out[0] as usize] += 1;
        self.hout[256 + out[1] as usize] += 1;
        self.hout[512 + out[2] as usize] += 1;
        self.hmx[out[0].max(out[1]).max(out[2]) as usize] += 1;
        self.hmn[out[0].min(out[1]).min(out[2]) as usize] += 1;
    }

    pub fn merge(parts: impl IntoIterator<Item = Option<Hist>>) -> Option<Hist> {
        let mut acc: Option<Hist> = None;
        for p in parts.into_iter().flatten() {
            match &mut acc {
                None => acc = Some(p),
                Some(a) => {
                    for (x, y) in [(&mut a.hin, &p.hin), (&mut a.hout, &p.hout), (&mut a.hmx, &p.hmx), (&mut a.hmn, &p.hmn)] {
                        x.iter_mut().zip(y).for_each(|(u, v)| *u += v);
                    }
                }
            }
        }
        acc
    }
}

/// Final write of one pixel: curves, output stride 3 (RGB) or 4 (RGBA, alpha 255), histograms.
#[inline(always)]
pub(crate) fn write_px(o: &mut [u8], pre: [u8; 3], luts: &[[u8; 256]; 3], hist: &mut Option<Hist>) {
    let out = [luts[0][pre[0] as usize], luts[1][pre[1] as usize], luts[2][pre[2] as usize]];
    o[0] = out[0];
    o[1] = out[1];
    o[2] = out[2];
    if o.len() == 4 {
        o[3] = 255;
    }
    if let Some(h) = hist {
        h.add(pre, out);
    }
}

/// Per-channel curves (master curve folded in), in place. Phase-0 smoke test of the whole
/// chain (Python / WASM binding, data passing, parity harness); the full edit comes in phase 1.
pub fn apply_luts(buf: &mut [u8], width: usize, bpp: usize, luts: &[[u8; 256]; 3]) {
    assert!(bpp == 3 || bpp == 4, "bpp must be 3 or 4");
    assert!(width == 0 || buf.len().is_multiple_of(width * bpp), "buffer is not whole rows");
    for_each_band(buf, width * bpp, |band, _row| {
        for px in band.chunks_exact_mut(bpp) {
            px[0] = luts[0][px[0] as usize];
            px[1] = luts[1][px[1] as usize];
            px[2] = luts[2][px[2] as usize];
        }
    });
}

/// Three 256-entry tables from a flat slice of 768 values (R, G, B).
pub fn luts_from_flat(flat: &[u8]) -> Option<[[u8; 256]; 3]> {
    if flat.len() != 768 {
        return None;
    }
    let mut l = [[0u8; 256]; 3];
    for c in 0..3 {
        l[c].copy_from_slice(&flat[c * 256..(c + 1) * 256]);
    }
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luts_rgb_and_rgba() {
        let rev: [u8; 256] = core::array::from_fn(|i| (255 - i) as u8);
        let id: [u8; 256] = core::array::from_fn(|i| i as u8);
        let inv = [rev, id, rev]; // G identity
        let mut rgb = vec![0u8, 10, 200, 255, 1, 2];
        apply_luts(&mut rgb, 2, 3, &inv);
        assert_eq!(rgb, [255, 10, 55, 0, 1, 253]);
        let mut rgba = vec![0u8, 10, 200, 7];
        apply_luts(&mut rgba, 1, 4, &inv);
        assert_eq!(rgba, [255, 10, 55, 7]);
    }

    #[test]
    fn bands_cover_every_row() {
        let w = 3;
        let h = BAND_ROWS * 2 + 5;
        let mut buf = vec![0u8; w * 3 * h];
        for_each_band(&mut buf, w * 3, |band, row| {
            for (i, r) in band.chunks_exact_mut(w * 3).enumerate() {
                r.fill(((row + i) % 251) as u8);
            }
        });
        for (y, r) in buf.chunks_exact(w * 3).enumerate() {
            assert!(r.iter().all(|&v| v == (y % 251) as u8));
        }
    }
}
