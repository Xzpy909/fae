//! AVIF output through SvtAv1EncApp (B4; avif.py): RGB → 10-bit full-range BT.709 YUV 4:4:4, the
//! encoder's IVF / OBU stream → a still-image AVIF file (own muxer, byte-exact against avif.py's).

use crate::{Error, ErrorKind};

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::Failed, msg)
}

/// MSB-first bit reader over a byte slice (avif.py `_Bits`).
struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn f(&mut self, n: u32) -> Result<u64, Error> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = *self.d.get(self.p >> 3).ok_or_else(|| bad("AV1 sequence header: truncated"))?;
            v = (v << 1) | ((byte >> (7 - (self.p & 7))) & 1) as u64;
            self.p += 1;
        }
        Ok(v)
    }

    fn uvlc(&mut self) -> Result<u64, Error> {
        let mut lz = 0;
        while self.f(1)? == 0 {
            lz += 1;
            if lz >= 32 {
                return Ok((1 << 32) - 1);
            }
        }
        Ok((1u64 << lz) - 1 + self.f(lz)?)
    }
}

fn leb128(d: &[u8], p: usize) -> Result<(usize, usize), Error> {
    let mut v = 0usize;
    for i in 0..8 {
        let b = *d.get(p + i).ok_or_else(|| bad("bad leb128"))?;
        v |= ((b & 0x7F) as usize) << (7 * i);
        if b & 0x80 == 0 {
            return Ok((v, p + i + 1));
        }
    }
    Err(bad("bad leb128"))
}

/// [(obu_type, the whole OBU incl. header)]; every OBU must have its size field.
pub fn split_obus(data: &[u8]) -> Result<Vec<(u8, &[u8])>, Error> {
    let (mut out, mut p) = (vec![], 0);
    while p < data.len() {
        let start = p;
        let h = data[p];
        let (typ, ext, has_size) = ((h >> 3) & 0xF, (h >> 2) & 1, (h >> 1) & 1);
        p += 1 + ext as usize;
        if has_size == 0 {
            return Err(bad("OBU without size field"));
        }
        let (size, q) = leb128(data, p)?;
        p = q + size;
        if p > data.len() {
            return Err(bad("OBU past the end of the stream"));
        }
        out.push((typ, &data[start..p]));
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqHeader {
    pub profile: u8,
    pub level: u8,
    pub tier: u8,
    pub high_bitdepth: u8,
    pub twelve_bit: u8,
    pub mono: u8,
    pub sx: u8,
    pub sy: u8,
    pub csp: u8,
    pub depth: u8,
}

/// The fields of a sequence header OBU that av1C and pixi need (avif.py `parse_seq_header`).
pub fn parse_seq_header(obu: &[u8]) -> Result<SeqHeader, Error> {
    let h = *obu.first().ok_or_else(|| bad("empty OBU"))?;
    let p = 1 + ((h >> 2) & 1) as usize;
    let (_, p) = leb128(obu, p)?;
    let mut b = Bits { d: &obu[p..], p: 0 };
    let prof = b.f(3)? as u8;
    b.f(1)?; // still_picture
    let reduced = b.f(1)?;
    let mut tier = 0u8;
    let level: u8;
    if reduced != 0 {
        level = b.f(5)? as u8;
    } else {
        let timing = b.f(1)?;
        let (mut dmi, mut bdl) = (0, 0);
        if timing != 0 {
            b.f(32)?;
            b.f(32)?;
            if b.f(1)? != 0 {
                b.uvlc()?;
            }
            dmi = b.f(1)?;
            if dmi != 0 {
                bdl = b.f(5)? as u32 + 1;
                b.f(32)?;
                b.f(5)?;
                b.f(5)?;
            }
        }
        let idd = b.f(1)?;
        let cnt = b.f(5)? + 1;
        let mut lvl = None;
        for i in 0..cnt {
            b.f(12)?;
            let lv = b.f(5)? as u8;
            let t = if lv > 7 { b.f(1)? as u8 } else { 0 };
            if i == 0 {
                lvl = Some(lv);
                tier = t;
            }
            if dmi != 0 && b.f(1)? != 0 {
                b.f(bdl)?;
                b.f(bdl)?;
                b.f(1)?;
            }
            if idd != 0 && b.f(1)? != 0 {
                b.f(4)?;
            }
        }
        level = lvl.unwrap_or(0);
    }
    let (wb, hb) = (b.f(4)? as u32 + 1, b.f(4)? as u32 + 1);
    b.f(wb)?;
    b.f(hb)?;
    if reduced == 0 && b.f(1)? != 0 {
        b.f(4)?;
        b.f(3)?;
    }
    b.f(1)?;
    b.f(1)?;
    b.f(1)?;
    if reduced == 0 {
        b.f(1)?;
        b.f(1)?;
        b.f(1)?;
        b.f(1)?;
        let order_hint = b.f(1)?;
        if order_hint != 0 {
            b.f(1)?;
            b.f(1)?;
        }
        let force_sct = if b.f(1)? != 0 { 2 } else { b.f(1)? };
        if force_sct > 0 && b.f(1)? == 0 {
            b.f(1)?;
        }
        if order_hint != 0 {
            b.f(3)?;
        }
    }
    b.f(1)?;
    b.f(1)?;
    b.f(1)?;
    // color_config
    let hbd = b.f(1)? as u8;
    let twelve = if prof == 2 && hbd != 0 { b.f(1)? as u8 } else { 0 };
    let mono = if prof == 1 { 0 } else { b.f(1)? as u8 };
    let (mut cp, mut tc, mut mc) = (2, 2, 2);
    if b.f(1)? != 0 {
        cp = b.f(8)?;
        tc = b.f(8)?;
        mc = b.f(8)?;
    }
    let mut csp = 0u8;
    let (sx, sy);
    if mono != 0 {
        (sx, sy) = (1, 1);
    } else if cp == 1 && tc == 13 && mc == 0 {
        (sx, sy) = (0, 0);
    } else {
        b.f(1)?;
        if prof == 0 {
            (sx, sy) = (1, 1);
        } else if prof == 1 {
            (sx, sy) = (0, 0);
        } else if twelve != 0 {
            sx = b.f(1)? as u8;
            sy = if sx != 0 { b.f(1)? as u8 } else { 0 };
        } else {
            (sx, sy) = (1, 0);
        }
        if sx != 0 && sy != 0 {
            csp = b.f(2)? as u8;
        }
    }
    let depth = if twelve != 0 { 12 } else if hbd != 0 { 10 } else { 8 };
    Ok(SeqHeader { profile: prof, level, tier, high_bitdepth: hbd, twelve_bit: twelve, mono, sx, sy, csp, depth })
}

fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(payload);
    v
}

fn fbx(typ: &[u8; 4], ver: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = (((ver as u32) << 24) | flags).to_be_bytes().to_vec();
    p.extend_from_slice(payload);
    bx(typ, &p)
}

/// Wrap a single-frame AV1 OBU stream into a still-image AVIF file (avif.py `mux_avif`, byte for byte).
/// `nclx` = (colour primaries, transfer, matrix, full range).
pub fn mux(obu_stream: &[u8], width: u32, height: u32, icc: Option<&[u8]>, nclx: (u16, u16, u16, u8)) -> Result<Vec<u8>, Error> {
    let obus = split_obus(obu_stream)?;
    let seq = obus.iter().find(|(t, _)| *t == 1).map(|(_, o)| *o).ok_or_else(|| bad("no sequence header in AV1 stream"))?;
    // drop temporal delimiters / padding
    let item: Vec<u8> = obus.iter().filter(|(t, _)| *t != 2 && *t != 15).flat_map(|(_, o)| o.iter().copied()).collect();
    let s = parse_seq_header(seq)?;
    let mut av1c = vec![
        0x81,
        (s.profile << 5) | (s.level & 0x1F),
        (s.tier << 7) | (s.high_bitdepth << 6) | (s.twelve_bit << 5) | (s.mono << 4) | (s.sx << 3) | (s.sy << 2) | (s.csp & 3),
        0,
    ];
    av1c.extend_from_slice(seq);
    let ch: u8 = if s.mono != 0 { 1 } else { 3 };
    let mut pixi = vec![ch];
    pixi.extend(std::iter::repeat_n(s.depth, ch as usize));
    let mut ispe = width.to_be_bytes().to_vec();
    ispe.extend_from_slice(&height.to_be_bytes());
    let mut nclx_p = b"nclx".to_vec();
    nclx_p.extend_from_slice(&nclx.0.to_be_bytes());
    nclx_p.extend_from_slice(&nclx.1.to_be_bytes());
    nclx_p.extend_from_slice(&nclx.2.to_be_bytes());
    nclx_p.push((nclx.3 & 1) << 7);
    let icc = icc.filter(|i| !i.is_empty());
    let mut props = [fbx(b"ispe", 0, 0, &ispe), fbx(b"pixi", 0, 0, &pixi), bx(b"av1C", &av1c), bx(b"colr", &nclx_p)].concat();
    if let Some(icc) = icc {
        let mut p = b"prof".to_vec();
        p.extend_from_slice(icc);
        props.extend(bx(b"colr", &p));
    }
    let mut assoc: Vec<(u8, bool)> = vec![(1, false), (2, false), (3, true), (4, false)];
    if icc.is_some() {
        assoc.push((5, false));
    }
    let mut ipma_p = vec![0, 0, 0, 1, 0, 1, assoc.len() as u8];
    ipma_p.extend(assoc.iter().map(|&(idx, ess)| if ess { 0x80 } else { 0 } | idx));
    let ipma = fbx(b"ipma", 0, 0, &ipma_p);
    let iprp = bx(b"iprp", &[bx(b"ipco", &props), ipma].concat());
    let ftyp = bx(b"ftyp", b"avif\0\0\0\0avifmif1miaf");
    let mut hdlr_p = vec![0u8; 4];
    hdlr_p.extend_from_slice(b"pict");
    hdlr_p.extend_from_slice(&[0u8; 13]);
    let hdlr = fbx(b"hdlr", 0, 0, &hdlr_p);
    let pitm = fbx(b"pitm", 0, 0, &[0, 1]);
    let mut infe_p = vec![0, 1, 0, 0];
    infe_p.extend_from_slice(b"av01\0");
    let mut iinf_p = vec![0, 1];
    iinf_p.extend(fbx(b"infe", 2, 0, &infe_p));
    let iinf = fbx(b"iinf", 0, 0, &iinf_p);
    let build = |offset: u32| {
        let mut p = vec![0x44, 0x00, 0, 1, 0, 1, 0, 0, 0, 1];
        p.extend_from_slice(&offset.to_be_bytes());
        p.extend_from_slice(&(item.len() as u32).to_be_bytes());
        let iloc = fbx(b"iloc", 0, 0, &p);
        fbx(b"meta", 0, 0, &[&hdlr[..], &pitm, &iloc, &iinf, &iprp].concat())
    };
    let meta = build(0);
    let offset = (ftyp.len() + meta.len() + 8) as u32; // data starts right after the mdat header
    let meta = build(offset);
    Ok([ftyp, meta, bx(b"mdat", &item)].concat())
}

/// IVF container → the first frame's OBUs (a raw OBU stream passes through) (avif.py `ivf_payload`).
pub fn ivf_payload(data: &[u8]) -> Result<&[u8], Error> {
    if !data.starts_with(b"DKIF") {
        return Ok(data);
    }
    let short = || bad("IVF: truncated");
    let hdr = u16::from_le_bytes(data.get(6..8).ok_or_else(short)?.try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(data.get(hdr..hdr + 4).ok_or_else(short)?.try_into().unwrap()) as usize;
    data.get(hdr + 12..hdr + 12 + size).ok_or_else(short)
}

/// 8-bit RGB (3 or 4 channels: alpha ignored) → planar 10-bit little-endian Y, Cb, Cr, full range,
/// BT.709 matrix: avif.py `rgb_to_yuv444p10`, with NumPy's float32 arithmetic step by step (same values).
pub fn rgb_to_yuv444p10(px: &[u8], c: usize) -> Vec<u8> {
    let n = px.len() / c;
    let mut out = vec![0u8; n * 6];
    let (yp, rest) = out.split_at_mut(n * 2);
    let (up, vp) = rest.split_at_mut(n * 2);
    // Python floats become float32 as NumPy casts them (f64 → f32), not as f32 literals would
    let k = (1.0f64 / 255.0) as f32;
    let (kr, kg, kb, dcb, dcr) = (0.2126f64 as f32, 0.7152f64 as f32, 0.0722f64 as f32, 1.8556f64 as f32, 1.5748f64 as f32);
    let q = |v: f32| -> [u8; 2] { (v.round_ties_even().clamp(0.0, 1023.0) as u16).to_le_bytes() };
    use rayon::prelude::*;
    const CH: usize = 1 << 16;
    yp.par_chunks_mut(CH * 2)
        .zip(up.par_chunks_mut(CH * 2))
        .zip(vp.par_chunks_mut(CH * 2))
        .zip(px.par_chunks(CH * c))
        .for_each(|(((y, u), v), src)| {
            for (i, p) in src.chunks_exact(c).enumerate() {
                let (r, g, b) = (p[0] as f32 * k, p[1] as f32 * k, p[2] as f32 * k);
                let yy = kr * r + kg * g + kb * b;
                let cb = (b - yy) / dcb;
                let cr = (r - yy) / dcr;
                y[i * 2..i * 2 + 2].copy_from_slice(&q(yy * 1023.0));
                u[i * 2..i * 2 + 2].copy_from_slice(&q(cb * 1023.0 + 512.0));
                v[i * 2..i * 2 + 2].copy_from_slice(&q(cr * 1023.0 + 512.0));
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb_and_ivf() {
        assert_eq!(leb128(&[0x85, 0x01], 0).unwrap(), (133, 2));
        let mut ivf = b"DKIF\0\0\x20\0".to_vec();
        ivf.resize(32, 0);
        ivf.extend_from_slice(&3u32.to_le_bytes());
        ivf.extend_from_slice(&[0; 8]);
        ivf.extend_from_slice(&[7, 8, 9, 10]);
        assert_eq!(ivf_payload(&ivf).unwrap(), &[7, 8, 9]);
        assert_eq!(ivf_payload(&[1, 2]).unwrap(), &[1, 2]);
    }

    #[test]
    fn yuv_neutral() {
        let v = rgb_to_yuv444p10(&[255, 255, 255, 0, 0, 0], 3);
        assert_eq!(&v[0..4], &[0xFF, 0x03, 0, 0]); // Y: 1023, 0
        assert_eq!(&v[4..8], &[0, 2, 0, 2]); // Cb: 512, 512
    }
}

// ---- rav1e (B4: replaces Pillow's libavif / aom for alpha, the UI's "aom" choice, no SvtAv1EncApp) --------

/// ravif's quality (0..100) → rav1e quantizer.
fn quality_to_quantizer(quality: f32) -> usize {
    let q = quality / 100.0;
    let x = if q >= 0.82 { (1.0 - q) * 2.6 } else if q > 0.25 { q.mul_add(-0.5, 1.0 - 0.125) } else { 1.0 - q };
    (x * 255.0).round().clamp(0.0, 255.0) as usize
}

/// rav1e's settings for a still image at `speed` (ravif's tweaks of the speed preset).
fn speed_settings(speed: u8, quantizer: usize) -> rav1e::config::SpeedSettings {
    use rav1e::config::*;
    use rav1e::prelude::*;
    let low_quality = quantizer < quality_to_quantizer(55.0);
    let high_quality = quantizer > quality_to_quantizer(80.0);
    let max_block = if high_quality { 16 } else { 64 };
    let mut s = SpeedSettings::from_preset(speed);
    s.multiref = false;
    s.rdo_lookahead_frames = 1;
    s.scene_detection_mode = SceneDetectionSpeed::None;
    s.motion.include_near_mvs = false;
    s.motion.use_satd_subpel = false;
    s.fast_deblock = speed >= 7 && !high_quality;
    s.transform.reduced_tx_set = speed == 4 || speed >= 9;
    s.transform.tx_domain_rate = speed >= 10;
    s.transform.rdo_tx_decision = speed <= 4 && !high_quality;
    s.transform.enable_inter_tx_split = speed >= 9;
    s.partition.encode_bottomup = speed <= 2;
    s.cdef = low_quality && speed <= 9;
    s.lrf = low_quality && speed <= 8;
    s.sgr_complexity = if speed <= 2 { SGRComplexityLevel::Full } else { SGRComplexityLevel::Reduced };
    s.prediction.fine_directional_intra = speed <= 6;
    s.prediction.prediction_modes = if speed <= 1 { PredictionModesSetting::ComplexAll } else { PredictionModesSetting::Simple };
    let (lo, hi) = match speed {
        0 => (4, 64.min(max_block)),
        1 if low_quality => (4, 64.min(max_block)),
        2 if low_quality => (4, 32.min(max_block)),
        1..=4 => (4, 16),
        5..=8 => (8, 16),
        _ => (16, 16),
    };
    let sz = |s: u32| match s {
        4 => BlockSize::BLOCK_4X4,
        8 => BlockSize::BLOCK_8X8,
        16 => BlockSize::BLOCK_16X16,
        32 => BlockSize::BLOCK_32X32,
        _ => BlockSize::BLOCK_64X64,
    };
    s.partition.partition_range = PartitionRange::new(sz(lo), sz(hi));
    s
}

/// One still frame through rav1e: 10-bit full range, 4:4:4 (3 planes) or monochrome (1 plane, alpha).
/// The OBUs (sequence header + frame).
fn rav1e_encode(w: usize, h: usize, planes: &[&[u16]], quantizer: usize, speed: u8) -> Result<Vec<u8>, Error> {
    use rav1e::prelude::*;
    let mono = planes.len() == 1;
    let min_tile = match speed {
        0 => 4096,
        1 => 2048,
        2 => 1024,
        3 => 512,
        4 => 256,
        _ => 128,
    } * if quantizer > quality_to_quantizer(80.0) { 2 } else { 1 };
    let threads = rayon::current_num_threads();
    let enc = EncoderConfig {
        width: w,
        height: h,
        bit_depth: 10,
        chroma_sampling: if mono { ChromaSampling::Cs400 } else { ChromaSampling::Cs444 },
        chroma_sample_position: ChromaSamplePosition::Unknown,
        pixel_range: PixelRange::Full,
        color_description: if mono {
            None
        } else {
            Some(ColorDescription {
                color_primaries: ColorPrimaries::BT709,
                transfer_characteristics: TransferCharacteristics::SRGB,
                matrix_coefficients: MatrixCoefficients::BT709,
            })
        },
        still_picture: true,
        quantizer,
        min_quantizer: quantizer as u8,
        tune: Tune::Psychovisual,
        tiles: threads.min((w * h) / (min_tile * min_tile)).max(1),
        speed_settings: speed_settings(speed, quantizer),
        ..EncoderConfig::with_speed_preset(speed)
    };
    let cfg = Config::new().with_encoder_config(enc).with_threads(threads);
    let mut ctx: Context<u16> = cfg.new_context().map_err(|e| bad(format!("rav1e: {e}")))?;
    let mut frame = ctx.new_frame();
    for (i, p) in planes.iter().enumerate() {
        frame.planes[i].copy_from_raw_u8(&p.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(), w * 2, 2);
    }
    ctx.send_frame(frame).map_err(|e| bad(format!("rav1e: {e}")))?;
    ctx.flush();
    let mut out = ctx.container_sequence_header()[4..].to_vec(); // av1C's configOBUs: the sequence header
    loop {
        match ctx.receive_packet() {
            Ok(p) => out.extend_from_slice(&p.data),
            Err(EncoderStatus::Encoded) => continue,
            Err(EncoderStatus::LimitReached) => break,
            Err(e) => return Err(bad(format!("rav1e: {e}"))),
        }
    }
    Ok(out)
}

/// AVIF through rav1e: 10-bit 4:4:4 full-range BT.709 colour (the same YUV as the SvtAv1EncApp path)
/// and, for RGBA, a 10-bit monochrome alpha item. `quality` / `speed` as the UI's aom settings.
pub fn encode_rav1e(px: &[u8], w: usize, h: usize, c: usize, quality: i64, speed: i64, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let q = quality.clamp(0, 100) as f32;
    let speed = speed.clamp(0, 10) as u8;
    let yuv = rgb_to_yuv444p10(px, c);
    let s: Vec<u16> = yuv.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let n = w * h;
    let color = rav1e_encode(w, h, &[&s[..n], &s[n..2 * n], &s[2 * n..]], quality_to_quantizer(q), speed)?;
    if c != 4 {
        return mux(&color, w as u32, h as u32, icc, (1, 13, 1, 1));
    }
    let a: Vec<u16> = px.chunks_exact(4).map(|p| ((p[3] as u16) << 2) | (p[3] as u16 >> 6)).collect();
    let aq = ((q + 100.0) / 2.0).min(q + q / 4.0 + 2.0);
    let alpha = rav1e_encode(w, h, &[&a], quality_to_quantizer(aq), speed)?;
    mux_alpha(&color, &alpha, w as u32, h as u32, icc, (1, 13, 1, 1))
}

/// av1C of a stream: its 4 config bytes + the sequence header OBU; and the item data (no TD / padding).
fn av1c_and_item(stream: &[u8]) -> Result<(Vec<u8>, Vec<u8>, SeqHeader), Error> {
    let obus = split_obus(stream)?;
    let seq = obus.iter().find(|(t, _)| *t == 1).map(|(_, o)| *o).ok_or_else(|| bad("no sequence header in AV1 stream"))?;
    let item: Vec<u8> = obus.iter().filter(|(t, _)| *t != 2 && *t != 15).flat_map(|(_, o)| o.iter().copied()).collect();
    let s = parse_seq_header(seq)?;
    let mut av1c = vec![
        0x81,
        (s.profile << 5) | (s.level & 0x1F),
        (s.tier << 7) | (s.high_bitdepth << 6) | (s.twelve_bit << 5) | (s.mono << 4) | (s.sx << 3) | (s.sy << 2) | (s.csp & 3),
        0,
    ];
    av1c.extend_from_slice(seq);
    Ok((av1c, item, s))
}

/// A still image with an alpha item (auxl, auxC "…:auxiliary:alpha"), colour first in mdat.
pub fn mux_alpha(color: &[u8], alpha: &[u8], width: u32, height: u32, icc: Option<&[u8]>, nclx: (u16, u16, u16, u8)) -> Result<Vec<u8>, Error> {
    let (av1c, item, s) = av1c_and_item(color)?;
    let (av1c_a, item_a, sa) = av1c_and_item(alpha)?;
    let mut ispe = width.to_be_bytes().to_vec();
    ispe.extend_from_slice(&height.to_be_bytes());
    let pixi = |s: &SeqHeader| {
        let ch: u8 = if s.mono != 0 { 1 } else { 3 };
        let mut p = vec![ch];
        p.extend(std::iter::repeat_n(s.depth, ch as usize));
        fbx(b"pixi", 0, 0, &p)
    };
    let mut nclx_p = b"nclx".to_vec();
    nclx_p.extend_from_slice(&nclx.0.to_be_bytes());
    nclx_p.extend_from_slice(&nclx.1.to_be_bytes());
    nclx_p.extend_from_slice(&nclx.2.to_be_bytes());
    nclx_p.push((nclx.3 & 1) << 7);
    let icc = icc.filter(|i| !i.is_empty());
    // 1 ispe, 2 pixi, 3 av1C, 4 colr nclx, [5 colr prof], then the alpha's pixi, av1C, auxC
    let mut props = vec![fbx(b"ispe", 0, 0, &ispe), pixi(&s), bx(b"av1C", &av1c), bx(b"colr", &nclx_p)];
    let mut color_assoc: Vec<(u8, bool)> = vec![(1, false), (2, false), (3, true), (4, false)];
    if let Some(icc) = icc {
        let mut p = b"prof".to_vec();
        p.extend_from_slice(icc);
        props.push(bx(b"colr", &p));
        color_assoc.push((5, false));
    }
    let base = props.len() as u8;
    props.push(pixi(&sa));
    props.push(bx(b"av1C", &av1c_a));
    let mut auxc = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0".to_vec();
    auxc.splice(0..0, [0u8; 4]);
    props.push(bx(b"auxC", &auxc));
    let alpha_assoc: Vec<(u8, bool)> = vec![(1, false), (base + 1, false), (base + 2, true), (base + 3, true)];
    let mut ipma_p = vec![0, 0, 0, 2];
    for (id, assoc) in [(1u16, &color_assoc), (2u16, &alpha_assoc)] {
        ipma_p.extend_from_slice(&id.to_be_bytes());
        ipma_p.push(assoc.len() as u8);
        ipma_p.extend(assoc.iter().map(|&(idx, ess)| if ess { 0x80 } else { 0 } | idx));
    }
    let iprp = bx(b"iprp", &[bx(b"ipco", &props.concat()), fbx(b"ipma", 0, 0, &ipma_p)].concat());
    let ftyp = bx(b"ftyp", b"avif\0\0\0\0avifmif1miaf");
    let mut hdlr_p = vec![0u8; 4];
    hdlr_p.extend_from_slice(b"pict");
    hdlr_p.extend_from_slice(&[0u8; 13]);
    let hdlr = fbx(b"hdlr", 0, 0, &hdlr_p);
    let pitm = fbx(b"pitm", 0, 0, &[0, 1]);
    let infe = |id: u16| {
        let mut p = id.to_be_bytes().to_vec();
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(b"av01\0");
        fbx(b"infe", 2, 0, &p)
    };
    let iinf = fbx(b"iinf", 0, 0, &[&[0u8, 2][..], &infe(1), &infe(2)].concat());
    let iref = fbx(b"iref", 0, 0, &bx(b"auxl", &[0, 2, 0, 1, 0, 1]));
    let build = |offset: u32| {
        let mut p = vec![0x44, 0x00, 0, 2];
        for (id, off, len) in [(1u16, offset, item.len()), (2, offset + item.len() as u32, item_a.len())] {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&[0, 0, 0, 1]);
            p.extend_from_slice(&off.to_be_bytes());
            p.extend_from_slice(&(len as u32).to_be_bytes());
        }
        let iloc = fbx(b"iloc", 0, 0, &p);
        fbx(b"meta", 0, 0, &[&hdlr[..], &pitm, &iloc, &iinf, &iref, &iprp].concat())
    };
    let meta = build(0);
    let offset = (ftyp.len() + meta.len() + 8) as u32;
    let meta = build(offset);
    Ok([ftyp, meta, bx(b"mdat", &[item, item_a].concat())].concat())
}
