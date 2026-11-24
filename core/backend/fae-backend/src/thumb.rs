//! File-list thumbnails (wave B2), as fastedit/engine.py `_make_thumb` did with Pillow:
//! `draft("RGB", (2s, 2s))` (JPEG: DCT-scaled decoding), EXIF orientation, RGB (alpha dropped),
//! `thumbnail((2s, 2s), BILINEAR)` (Pillow's size rounding and reducing step), JPEG quality 82 written
//! with libjpeg-turbo's defaults (4:2:0, standard tables), the same encoder settings Pillow uses.

use crate::px::Px;
use crate::resample::{self, Filter};
use crate::{Error, ErrorKind};

/// Image.thumbnail's size: fit inside `max` keeping the aspect (Pillow's rounding); None = no resize.
pub fn thumb_size(w: usize, h: usize, max: (usize, usize)) -> Option<(usize, usize)> {
    let (x, y) = max;
    if x >= w && y >= h {
        return None;
    }
    let aspect = w as f64 / h as f64;
    // round_aspect: floor or ceil, whichever the key prefers (floor on a tie), at least 1
    let pick = |n: f64, key: &dyn Fn(f64) -> f64| {
        let (f, c) = (n.floor(), n.ceil());
        (if key(c) < key(f) { c } else { f }).max(1.0) as usize
    };
    let (x, y) = (x as f64, y as f64);
    Some(if x / y >= aspect {
        (pick(y * aspect, &|n| (aspect - n / y).abs()), y as usize)
    } else {
        (x as usize, pick(x / aspect, &|n| if n == 0.0 { 0.0 } else { (aspect - x / n).abs() }))
    })
}

/// The thumbnail's pixels: `img` upright (decoded with the draft), RGB.
pub fn thumbnail(img: Px, max: (usize, usize)) -> Px {
    let img = img.to_rgb();
    match thumb_size(img.w, img.h, max) {
        Some(size) if size != (img.w, img.h) => resample::resize(img.view(), size, Filter::Bilinear, None, Some(2.0)),
        _ => img,
    }
}

/// RGB → JPEG bytes as Pillow's save(..., "JPEG", quality=q) (libjpeg-turbo defaults, baseline).
pub fn encode_jpeg(p: &Px, quality: u8) -> Result<Vec<u8>, Error> {
    let p = if p.c == 3 { p.clone() } else { p.clone().to_rgb() };
    std::panic::catch_unwind(|| -> std::io::Result<Vec<u8>> {
        let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        c.set_fastest_defaults();
        c.set_size(p.w, p.h);
        c.set_quality(quality as f32);
        let mut s = c.start_compress(Vec::new())?;
        s.write_scanlines(&p.data)?;
        s.finish()
    })
    .map_err(|_| Error::new(ErrorKind::Failed, "jpeg: encoder failed"))?
    .map_err(|e| Error::new(ErrorKind::Failed, format!("jpeg: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(thumb_size(400, 267, (224, 224)), Some((224, 150)));
        assert_eq!(thumb_size(100, 50, (224, 224)), None);
        assert_eq!(thumb_size(80, 120, (224, 224)), None);
        assert_eq!(thumb_size(10000, 10, (224, 224)), Some((224, 1)));
    }
}
