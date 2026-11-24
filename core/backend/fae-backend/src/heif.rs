//! HEIC / HEIF through libheif, loaded at run time (wave B1): the same library (and HEVC decoder,
//! libde265) pillow-heif ships, so the pixels are the ones the app showed before. No build step: the
//! shell says where the library files are (`Config::heif_dir`: next to the app, or pillow-heif's
//! `pillow_heif.libs`); without them HEIC files can't be opened (the error says so).
//!
//! Decoding as pillow-heif does for Pillow: the primary image, interleaved RGB or RGBA, high bit depths
//! converted to 8 bits by libheif, the file's rotation / mirroring applied by libheif, the raw ICC
//! profile ("prof" / "rICC") kept.

use crate::decode::Image;
use crate::{Error, ErrorKind};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::{Path, PathBuf};

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

type Ctx = c_void;
type Handle = c_void;
type Img = c_void;

const COLORSPACE_RGB: c_int = 1;
const CHROMA_RGB: c_int = 10;
const CHROMA_RGBA: c_int = 11;
const CHANNEL_INTERLEAVED: c_int = 10;
/// heif_decoding_options: version (u8), ignore_transformations (u8), 3 progress callbacks, user data,
/// then (version 2 on) convert_hdr_to_8bit (u8): its offset on 64-bit targets
const OPT_CONVERT_HDR: usize = 8 + 4 * std::mem::size_of::<usize>();

struct Fns {
    init: unsafe extern "C" fn(*mut c_void) -> HeifError,
    ctx_alloc: unsafe extern "C" fn() -> *mut Ctx,
    ctx_free: unsafe extern "C" fn(*mut Ctx),
    read_mem: unsafe extern "C" fn(*mut Ctx, *const c_void, usize, *const c_void) -> HeifError,
    primary: unsafe extern "C" fn(*mut Ctx, *mut *mut Handle) -> HeifError,
    handle_release: unsafe extern "C" fn(*const Handle),
    has_alpha: unsafe extern "C" fn(*const Handle) -> c_int,
    profile_type: unsafe extern "C" fn(*const Handle) -> u32,
    profile_size: unsafe extern "C" fn(*const Handle) -> usize,
    profile: unsafe extern "C" fn(*const Handle, *mut c_void) -> HeifError,
    opts_alloc: unsafe extern "C" fn() -> *mut u8,
    opts_free: unsafe extern "C" fn(*mut u8),
    decode: unsafe extern "C" fn(*const Handle, *mut *mut Img, c_int, c_int, *const u8) -> HeifError,
    plane: unsafe extern "C" fn(*const Img, c_int, *mut c_int) -> *const u8,
    width: unsafe extern "C" fn(*const Img, c_int) -> c_int,
    height: unsafe extern "C" fn(*const Img, c_int) -> c_int,
    img_release: unsafe extern "C" fn(*const Img),
    version: unsafe extern "C" fn() -> *const c_char,
}

/// libheif, loaded.
pub struct Lib {
    _lib: libloading::Library,
    f: Fns,
    pub path: PathBuf,
}

// libheif's decoding functions are thread-safe on separate contexts
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

fn err(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::Failed, msg)
}

/// The libheif file in `dir` (libheif.dll, libheif-<hash>.dll, libheif.so.1, libheif-<hash>.so.1.x, ...).
pub fn find(dir: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            (n.starts_with("libheif") || n.starts_with("heif"))
                && (n.ends_with(".dll") || n.ends_with(".dylib") || n.contains(".so"))
        })
        .collect();
    v.sort();
    v.into_iter().next()
}

impl Lib {
    /// Load libheif from `dir` (its dependencies, libde265 etc., next to it).
    pub fn load(dir: &Path) -> Result<Lib, Error> {
        let path = find(dir).ok_or_else(|| err(format!("HEIC: no libheif in {}", dir.display())))?;
        let lib = unsafe { open(&path) }.map_err(|e| err(format!("HEIC: loading {}: {e}", path.display())))?;
        macro_rules! sym {
            ($name:literal) => {
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| err(format!("HEIC: {} has no {}: {e}", path.display(), $name)))?
            };
        }
        let f = Fns {
            init: sym!("heif_init"),
            ctx_alloc: sym!("heif_context_alloc"),
            ctx_free: sym!("heif_context_free"),
            read_mem: sym!("heif_context_read_from_memory_without_copy"),
            primary: sym!("heif_context_get_primary_image_handle"),
            handle_release: sym!("heif_image_handle_release"),
            has_alpha: sym!("heif_image_handle_has_alpha_channel"),
            profile_type: sym!("heif_image_handle_get_color_profile_type"),
            profile_size: sym!("heif_image_handle_get_raw_color_profile_size"),
            profile: sym!("heif_image_handle_get_raw_color_profile"),
            opts_alloc: sym!("heif_decoding_options_alloc"),
            opts_free: sym!("heif_decoding_options_free"),
            decode: sym!("heif_decode_image"),
            plane: sym!("heif_image_get_plane_readonly"),
            width: sym!("heif_image_get_width"),
            height: sym!("heif_image_get_height"),
            img_release: sym!("heif_image_release"),
            version: sym!("heif_get_version"),
        };
        let e = unsafe { (f.init)(std::ptr::null_mut()) };
        if e.code != 0 {
            return Err(err(format!("HEIC: heif_init: {}", msg(e))));
        }
        Ok(Lib { _lib: lib, f, path })
    }

    pub fn version(&self) -> String {
        unsafe { CStr::from_ptr((self.f.version)()) }.to_string_lossy().into_owned()
    }

    /// Decode the primary image of a HEIC / HEIF file.
    pub fn decode(&self, data: &[u8]) -> Result<Image, Error> {
        let f = &self.f;
        unsafe {
            let ctx = (f.ctx_alloc)();
            if ctx.is_null() {
                return Err(err("HEIC: out of memory"));
            }
            let _ctx = Guard(|| (f.ctx_free)(ctx));
            check((f.read_mem)(ctx, data.as_ptr() as *const c_void, data.len(), std::ptr::null()))?;
            let mut h: *mut Handle = std::ptr::null_mut();
            check((f.primary)(ctx, &mut h))?;
            let _h = Guard(|| (f.handle_release)(h));
            let alpha = (f.has_alpha)(h) != 0;
            // the ICC profile, when the file has one ("prof" restricted / "rICC" unrestricted; nclx has none)
            let pt = (f.profile_type)(h);
            let icc = if pt == u32::from_be_bytes(*b"prof") || pt == u32::from_be_bytes(*b"rICC") {
                let n = (f.profile_size)(h);
                let mut v = vec![0u8; n];
                check((f.profile)(h, v.as_mut_ptr() as *mut c_void))?;
                Some(v)
            } else {
                None
            };
            let opts = (f.opts_alloc)();
            let _o = Guard(|| (f.opts_free)(opts));
            if !opts.is_null() && *opts >= 2 {
                *opts.add(OPT_CONVERT_HDR) = 1; // convert_hdr_to_8bit, as pillow-heif for Pillow
            }
            let mut img: *mut Img = std::ptr::null_mut();
            let chroma = if alpha { CHROMA_RGBA } else { CHROMA_RGB };
            check((f.decode)(h, &mut img, COLORSPACE_RGB, chroma, opts))?;
            let _i = Guard(|| (f.img_release)(img));
            let w = (f.width)(img, CHANNEL_INTERLEAVED);
            let hh = (f.height)(img, CHANNEL_INTERLEAVED);
            let mut stride: c_int = 0;
            let p = (f.plane)(img, CHANNEL_INTERLEAVED, &mut stride);
            if p.is_null() || w <= 0 || hh <= 0 {
                return Err(err("HEIC: no image plane"));
            }
            let c = if alpha { 4 } else { 3 };
            let row = w as usize * c;
            let mut px = Vec::with_capacity(row * hh as usize);
            for y in 0..hh as usize {
                px.extend_from_slice(std::slice::from_raw_parts(p.add(y * stride as usize), row));
            }
            Ok(Image { w: w as u32, h: hh as u32, channels: c as u8, px, icc, format: "heif" })
        }
    }
}

struct Guard<F: FnMut()>(F);
impl<F: FnMut()> Drop for Guard<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}

fn msg(e: HeifError) -> String {
    if e.message.is_null() {
        format!("error {}/{}", e.code, e.subcode)
    } else {
        unsafe { CStr::from_ptr(e.message) }.to_string_lossy().into_owned()
    }
}

fn check(e: HeifError) -> Result<(), Error> {
    if e.code == 0 { Ok(()) } else { Err(err(format!("HEIC: {}", msg(e)))) }
}

#[cfg(windows)]
unsafe fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    // look for the library's own dependencies (libde265, ...) in its folder
    use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
    Library::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH).map(Into::into)
}

#[cfg(not(windows))]
unsafe fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    libloading::Library::new(path)
}
