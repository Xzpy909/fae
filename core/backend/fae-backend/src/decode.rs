//! Opening images (wave B1): the bytes of a file → 8-bit RGB or RGBA, upright, plus its ICC profile, as
//! fastedit/engine.py did with Pillow (`Image.open` + `prepare`: EXIF orientation, then RGB, or RGBA when
//! the image has alpha or a transparent colour). The decoders are chosen to give Pillow's pixels exactly
//! (tests/stage/stage_check.py):
//! - JPEG: libjpeg-turbo's decoder (the mozjpeg crate; integer DCT, fancy upsampling, as Pillow), CMYK /
//!   YCCK converted with Pillow's integer formula (inverted when the file has an Adobe marker);
//! - PNG: the png crate (palette, low bit depths and tRNS expanded, 16 bits → the high byte, as Pillow's
//!   raw modes do; **16-bit grey is scaled the same way** — Pillow clipped it, so such files opened white);
//! - WebP: image-webp; GIF: the first frame; TIFF, BMP, PNM, TGA: the image crate; ICO: the largest entry;
//! - JPEG XL: jxl-oxide; HEIC / HEIF: libheif (heif.rs, loaded at run time);
//! - AVIF: not here yet → [`Error`] kind Unknown: the shell opens it itself (Pillow) until B4.

use crate::{heif, Error, ErrorKind};

/// An opened image: w × h pixels, 3 (RGB) or 4 (RGBA) bytes each, upright.
#[derive(Clone, Debug)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub channels: u8,
    pub px: Vec<u8>,
    /// the embedded ICC profile (kept for the output files, no conversion)
    pub icc: Option<Vec<u8>>,
    /// what the bytes were ("jpeg", "png", ...)
    pub format: &'static str,
}

fn fail(fmt: &str, e: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Failed, format!("{fmt}: {e}"))
}

fn unsupported(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::Unknown, msg)
}

/// What the bytes are (by content; TGA has no signature: by the file's extension).
pub fn sniff(data: &[u8], ext: &str) -> Option<&'static str> {
    let at = |o: usize, s: &[u8]| data.len() >= o + s.len() && &data[o..o + s.len()] == s;
    if at(0, &[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if at(0, b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if at(0, b"GIF87a") || at(0, b"GIF89a") {
        Some("gif")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("webp")
    } else if at(0, b"II*\0") || at(0, b"MM\0*") {
        Some("tiff")
    } else if at(0, b"BM") {
        Some("bmp")
    } else if at(0, &[0, 0, 1, 0]) {
        Some("ico")
    } else if data.len() > 2 && data[0] == b'P' && (b'1'..=b'7').contains(&data[1]) {
        Some("pnm")
    } else if at(0, &[0xFF, 0x0A]) || at(0, &[0, 0, 0, 0x0C, b'J', b'X', b'L', b' ']) {
        Some("jxl")
    } else if at(4, b"ftyp") {
        // ISO base media: the brands say HEIF or AVIF
        let n = (u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize).clamp(8, data.len());
        let brands: Vec<&[u8]> = data[8..n].chunks(4).filter(|c| c.len() == 4).collect();
        if brands.iter().any(|b| *b == b"avif" || *b == b"avis") {
            Some("avif")
        } else if brands.iter().any(|b| [&b"heic"[..], b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1"].contains(b)) {
            Some("heif")
        } else {
            None
        }
    } else if ext.eq_ignore_ascii_case("tga") {
        Some("tga")
    } else {
        None
    }
}

/// Open `data` (a file's bytes; `ext` = its extension without the dot). `heif` = libheif, when loaded.
pub fn decode(data: &[u8], ext: &str, heif: Option<&heif::Lib>) -> Result<Image, Error> {
    decode_draft(data, ext, heif, None)
}

/// [`decode`], with Pillow's `draft(mode, size)` for JPEG (B2, file-list thumbnails): decoded at 1/2,
/// 1/4 or 1/8 by libjpeg's DCT scaling, the largest step that keeps both sides at least `size`.
/// Other formats decode at full size (Pillow's draft does nothing for them).
pub fn decode_draft(data: &[u8], ext: &str, heif: Option<&heif::Lib>, draft: Option<(u32, u32)>) -> Result<Image, Error> {
    let format = sniff(data, ext).ok_or_else(|| unsupported("unknown image format"))?;
    let (mut img, exif) = match format {
        "jpeg" => jpeg(data, draft)?,
        "png" => png(data)?,
        "gif" => (gif(data)?, None),
        "webp" => webp(data)?,
        "tiff" => (via_image(data, image::ImageFormat::Tiff, "tiff")?, Some(data.to_vec())),
        "bmp" => (via_image(data, image::ImageFormat::Bmp, "bmp")?, None),
        "pnm" => (via_image(data, image::ImageFormat::Pnm, "pnm")?, None),
        "tga" => (via_image(data, image::ImageFormat::Tga, "tga")?, None),
        "ico" => (ico(data)?, None),
        "jxl" => (jxl(data)?, None),
        // libheif applies the file's rotation / mirroring itself (pillow-heif then leaves no EXIF orientation)
        "heif" => match heif {
            Some(h) => (h.decode(data)?, None),
            None => return Err(unsupported("HEIC: libheif not loaded")),
        },
        // the rotation / mirroring boxes as an orientation, as Pillow's AVIF plugin reports them (B4)
        "avif" => {
            let (mut im, o) = crate::avifdec::decode(data)?;
            orient(&mut im, o);
            (im, None)
        }
        _ => return Err(unsupported(format!("{format}: not in the Rust back end yet"))),
    };
    img.format = format;
    if let Some(o) = exif.as_deref().and_then(orientation) {
        orient(&mut img, o);
    }
    Ok(img)
}

// ---- EXIF orientation ---------------------------------------------------------------------------------

/// The orientation (1..8) in EXIF data (a TIFF structure, optionally after "Exif\0\0").
pub fn orientation(exif: &[u8]) -> Option<u32> {
    let raw = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif).to_vec();
    let e = exif::Reader::new().read_raw(raw).ok()?;
    let f = e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    f.value.get_uint(0)
}

/// Turn the pixels upright as Pillow's ImageOps.exif_transpose does (2..8; anything else: unchanged).
pub fn orient(img: &mut Image, o: u32) {
    if !(2..=8).contains(&o) {
        return;
    }
    let (w, h, c) = (img.w as usize, img.h as usize, img.channels as usize);
    let swap = o >= 5;
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; img.px.len()];
    for y in 0..oh {
        for x in 0..ow {
            // the source pixel of output (x, y)
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),         // FLIP_LEFT_RIGHT
                3 => (w - 1 - x, h - 1 - y), // ROTATE_180
                4 => (x, h - 1 - y),         // FLIP_TOP_BOTTOM
                5 => (y, x),                 // TRANSPOSE
                6 => (y, h - 1 - x),         // ROTATE_270 (90° clockwise)
                7 => (w - 1 - y, h - 1 - x), // TRANSVERSE
                _ => (w - 1 - y, x),         // 8: ROTATE_90 (90° counter-clockwise)
            };
            let s = (sy * w + sx) * c;
            let d = (y * ow + x) * c;
            out[d..d + c].copy_from_slice(&img.px[s..s + c]);
        }
    }
    img.px = out;
    img.w = ow as u32;
    img.h = oh as u32;
}

// ---- JPEG ---------------------------------------------------------------------------------------------

fn jpeg(data: &[u8], draft: Option<(u32, u32)>) -> Result<(Image, Option<Vec<u8>>), Error> {
    let f = |e| fail("jpeg", e);
    let mut d = mozjpeg::Decompress::with_markers(mozjpeg::ALL_MARKERS).from_mem(data).map_err(f)?;
    if let Some((dw, dh)) = draft {
        let (w, h) = (d.width() as u32, d.height() as u32);
        // JpegImagePlugin.draft: scale = min(w // dw, h // dh), then the largest of 8, 4, 2, 1 not above it
        let scale = (w / dw.max(1)).min(h / dh.max(1));
        let s = [8u32, 4, 2, 1].into_iter().find(|s| scale >= *s).unwrap_or(1);
        if s > 1 {
            d.scale((8 / s) as u8);
        }
    }
    let mut exif = None;
    let mut icc: Vec<(u8, Vec<u8>)> = vec![];
    let mut adobe = false;
    for m in d.markers() {
        match m.marker {
            mozjpeg::Marker::APP(1) if exif.is_none() && m.data.starts_with(b"Exif\0\0") => exif = Some(m.data.to_vec()),
            mozjpeg::Marker::APP(2) if m.data.starts_with(b"ICC_PROFILE\0") && m.data.len() >= 14 => {
                icc.push((m.data[12], m.data[14..].to_vec()))
            }
            mozjpeg::Marker::APP(14) if m.data.starts_with(b"Adobe") => adobe = true,
            _ => {}
        }
    }
    // Pillow: the ICC_PROFILE chunks joined in their sequence order
    icc.sort_by_key(|c| c.0);
    let icc = if icc.is_empty() { None } else { Some(icc.into_iter().flat_map(|c| c.1).collect()) };
    let cs = d.color_space();
    // the output size (smaller with a draft scale)
    let (w, h);
    let px = if cs == mozjpeg::ColorSpace::JCS_CMYK || cs == mozjpeg::ColorSpace::JCS_YCCK {
        let mut s = d.to_colorspace(mozjpeg::ColorSpace::JCS_CMYK).map_err(f)?;
        (w, h) = (s.width() as u32, s.height() as u32);
        let cmyk = s.read_scanlines::<u8>().map_err(f)?;
        s.finish().map_err(f)?;
        let mut px = Vec::with_capacity(cmyk.len() / 4 * 3);
        for p in cmyk.chunks_exact(4) {
            // Pillow: Adobe files store CMYK inverted ("CMYK;I"); then cmyk2rgb in integers
            let (c, m, y, k) = if adobe { (255 - p[0], 255 - p[1], 255 - p[2], 255 - p[3]) } else { (p[0], p[1], p[2], p[3]) };
            let nk = 255 - k as u32;
            for v in [c, m, y] {
                let t = v as u32 * nk + 128;
                px.push((nk - (((t >> 8) + t) >> 8)) as u8);
            }
        }
        px
    } else {
        // grey is decoded as grey and repeated (libjpeg's grey → RGB does the same)
        let mut s = d.rgb().map_err(f)?;
        (w, h) = (s.width() as u32, s.height() as u32);
        let px = s.read_scanlines::<u8>().map_err(f)?;
        s.finish().map_err(f)?;
        px
    };
    Ok((Image { w, h, channels: 3, px, icc, format: "jpeg" }, exif))
}

// ---- PNG ----------------------------------------------------------------------------------------------

fn png(data: &[u8]) -> Result<(Image, Option<Vec<u8>>), Error> {
    let f = |e| fail("png", e);
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    // palette and low bit depths expanded, tRNS -> alpha, 16 bits -> the high byte (Pillow's raw modes)
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(f)?;
    let mut buf = vec![0u8; r.output_buffer_size().ok_or_else(|| fail("png", "image too big"))?];
    let fi = r.next_frame(&mut buf).map_err(f)?;
    buf.truncate(fi.buffer_size());
    let info = r.info();
    let icc = info.icc_profile.as_ref().map(|c| c.to_vec());
    let exif = info.exif_metadata.as_ref().map(|c| c.to_vec());
    let (w, h) = (fi.width, fi.height);
    let (channels, px) = match fi.color_type {
        png::ColorType::Rgb => (3, buf),
        png::ColorType::Rgba => (4, buf),
        png::ColorType::Grayscale => (3, buf.iter().flat_map(|&v| [v, v, v]).collect()),
        png::ColorType::GrayscaleAlpha => (4, buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()),
        png::ColorType::Indexed => return Err(fail("png", "palette not expanded")),
    };
    Ok((Image { w, h, channels, px, icc, format: "png" }, exif))
}

// ---- GIF ----------------------------------------------------------------------------------------------

fn gif(data: &[u8]) -> Result<Image, Error> {
    let f = |e| fail("gif", e);
    let mut o = gif::DecodeOptions::new();
    o.set_color_output(gif::ColorOutput::Indexed);
    let mut d = o.read_info(std::io::Cursor::new(data)).map_err(f)?;
    let (sw, sh) = (d.width() as usize, d.height() as usize);
    let global = d.global_palette().map(|p| p.to_vec());
    let bg = d.bg_color();
    let fr = d.read_next_frame().map_err(f)?.ok_or_else(|| fail("gif", "no frame"))?.clone();
    let pal = fr.palette.clone().or(global).ok_or_else(|| fail("gif", "no palette"))?;
    let trns = fr.transparent;
    // Pillow: the first frame in the logical screen; RGBA when the frame has a transparent index
    let ch = if trns.is_some() { 4 } else { 3 };
    let colour = |i: usize| -> [u8; 4] {
        let c = pal.get(i * 3..i * 3 + 3).map_or([0, 0, 0], |c| [c[0], c[1], c[2]]);
        [c[0], c[1], c[2], if Some(i as u8) == trns { 0 } else { 255 }]
    };
    let fill = match (trns, bg) {
        (Some(t), _) => colour(t as usize),
        (None, Some(b)) => colour(b),
        _ => [0, 0, 0, 255],
    };
    let mut px = Vec::with_capacity(sw * sh * ch);
    for _ in 0..sw * sh {
        px.extend_from_slice(&fill[..ch]);
    }
    let (fl, ft, fw, fh) = (fr.left as usize, fr.top as usize, fr.width as usize, fr.height as usize);
    for y in 0..fh.min(sh.saturating_sub(ft)) {
        for x in 0..fw.min(sw.saturating_sub(fl)) {
            let c = colour(fr.buffer[y * fw + x] as usize);
            let d = ((ft + y) * sw + fl + x) * ch;
            px[d..d + ch].copy_from_slice(&c[..ch]);
        }
    }
    Ok(Image { w: sw as u32, h: sh as u32, channels: ch as u8, px, icc: None, format: "gif" })
}

// ---- WebP ---------------------------------------------------------------------------------------------

fn webp(data: &[u8]) -> Result<(Image, Option<Vec<u8>>), Error> {
    let f = |e| fail("webp", e);
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).map_err(f)?;
    let (w, h) = d.dimensions();
    let channels = if d.has_alpha() { 4 } else { 3 };
    let icc = d.icc_profile().map_err(f)?;
    let exif = d.exif_metadata().map_err(f)?;
    let mut px = vec![0u8; d.output_buffer_size().ok_or_else(|| fail("webp", "image too big"))?];
    d.read_image(&mut px).map_err(f)?;
    Ok((Image { w, h, channels, px, icc, format: "webp" }, exif))
}

// ---- through the image crate (TIFF, BMP, PNM, TGA) ---------------------------------------------------------

fn via_image(data: &[u8], fmt: image::ImageFormat, name: &'static str) -> Result<Image, Error> {
    let f = |e| fail(name, e);
    let im = image::load_from_memory_with_format(data, fmt).map_err(f)?;
    let icc = if fmt == image::ImageFormat::Tiff { tiff_icc(data) } else { None };
    let alpha = im.color().has_alpha();
    let (w, h) = (im.width(), im.height());
    let (channels, px) = if alpha { (4, im.into_rgba8().into_raw()) } else { (3, im.into_rgb8().into_raw()) };
    Ok(Image { w, h, channels, px, icc, format: name })
}

fn tiff_icc(data: &[u8]) -> Option<Vec<u8>> {
    let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(data)).ok()?;
    d.get_tag_u8_vec(tiff::tags::Tag::Unknown(34675)).ok()
}

// ---- ICO: the largest entry (as Pillow) ------------------------------------------------------------------

fn ico(data: &[u8]) -> Result<Image, Error> {
    let f = |e: &str| fail("ico", e);
    let n = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut best: Option<(u64, usize, usize)> = None; // (w*h*bpp, offset, size)
    for i in 0..n {
        let e = data.get(6 + i * 16..6 + i * 16 + 16).ok_or_else(|| f("short directory"))?;
        let w = if e[0] == 0 { 256 } else { e[0] as u64 };
        let h = if e[1] == 0 { 256 } else { e[1] as u64 };
        let bpp = u16::from_le_bytes([e[6], e[7]]) as u64;
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let key = (w * h) << 8 | bpp;
        if best.is_none_or(|b| key > b.0) {
            best = Some((key, off, size));
        }
    }
    let (_, off, size) = best.ok_or_else(|| f("no images"))?;
    let entry = data.get(off..off + size).ok_or_else(|| f("entry outside the file"))?;
    if entry.starts_with(b"\x89PNG") {
        return Ok(png(entry)?.0);
    }
    via_image(data, image::ImageFormat::Ico, "ico")
}

// ---- JPEG XL ------------------------------------------------------------------------------------------

fn jxl(data: &[u8]) -> Result<Image, Error> {
    let f = |e: String| fail("jxl", e);
    let img = jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(data)).map_err(|e| f(e.to_string()))?;
    let icc = img.original_icc().map(|v| v.to_vec());
    let r = img.render_frame(0).map_err(|e| f(e.to_string()))?;
    let fb = r.image_all_channels();
    let (w, h, c) = (fb.width(), fb.height(), fb.channels());
    let buf = fb.buf();
    let channels: u8 = if c == 2 || c >= 4 { 4 } else { 3 };
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut px = Vec::with_capacity(w * h * channels as usize);
    for p in buf.chunks_exact(c) {
        match c {
            1 => px.extend_from_slice(&[q(p[0]); 3]),
            2 => px.extend_from_slice(&[q(p[0]), q(p[0]), q(p[0]), q(p[1])]),
            3 => px.extend(p.iter().map(|&v| q(v))),
            _ => px.extend(p[..4].iter().map(|&v| q(v))),
        }
    }
    Ok(Image { w: w as u32, h: h as u32, channels, px, icc, format: "jxl" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0", "jpg"), Some("jpeg"));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n....", "jpg"), Some("png")); // content wins over the name
        assert_eq!(sniff(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic", "heic"), Some("heif"));
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf", "avif"), Some("avif"));
        assert_eq!(sniff(b"\0\0\0\0", "tga"), Some("tga"));
        assert_eq!(sniff(b"hello", "txt"), None);
    }

    #[test]
    fn orientations_round_trip() {
        // 3 x 2 image, one channel: values 0..6
        let base = Image { w: 3, h: 2, channels: 1, px: (0..6).collect(), icc: None, format: "test" };
        let mut a = base.clone();
        orient(&mut a, 6); // 90° clockwise: 2 x 3
        assert_eq!((a.w, a.h), (2, 3));
        assert_eq!(a.px, vec![3, 0, 4, 1, 5, 2]);
        orient(&mut a, 8); // and back
        assert_eq!(a.px, base.px);
        let mut b = base.clone();
        orient(&mut b, 3);
        assert_eq!(b.px, vec![5, 4, 3, 2, 1, 0]);
        let mut c = base.clone();
        orient(&mut c, 1);
        assert_eq!(c.px, base.px);
    }
}
