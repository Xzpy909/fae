//! Pixels between the stage's steps: w × h, 3 (RGB) or 4 (RGBA) interleaved 8-bit channels, rows top
//! to bottom (what decode.rs gives, and what fastedit/ handed around as Pillow images).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Px {
    pub w: usize,
    pub h: usize,
    /// 3 or 4
    pub c: usize,
    pub data: Vec<u8>,
}

impl Px {
    pub fn new(w: usize, h: usize, c: usize) -> Px {
        Px { w, h, c, data: vec![0; w * h * c] }
    }

    pub fn from_vec(w: usize, h: usize, c: usize, data: Vec<u8>) -> Px {
        assert_eq!(data.len(), w * h * c, "pixel buffer size");
        Px { w, h, c, data }
    }

    pub fn alpha(&self) -> bool {
        self.c == 4
    }

    pub fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.w * self.c..(y + 1) * self.w * self.c]
    }

    pub fn view(&self) -> View<'_> {
        View { w: self.w, h: self.h, c: self.c, data: &self.data }
    }

    /// Only the colour channels (RGBA → RGB, as Pillow's convert("RGB"): alpha dropped).
    pub fn to_rgb(self) -> Px {
        if self.c == 3 {
            return self;
        }
        let mut d = Vec::with_capacity(self.w * self.h * 3);
        for p in self.data.chunks_exact(4) {
            d.extend_from_slice(&p[..3]);
        }
        Px { w: self.w, h: self.h, c: 3, data: d }
    }
}

impl From<crate::decode::Image> for Px {
    fn from(im: crate::decode::Image) -> Px {
        Px::from_vec(im.w as usize, im.h as usize, im.channels as usize, im.px)
    }
}

/// Borrowed pixels (the caller's buffer, e.g. Python bytes: no copy on the way in).
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    pub w: usize,
    pub h: usize,
    pub c: usize,
    pub data: &'a [u8],
}

impl<'a> View<'a> {
    pub fn new(w: usize, h: usize, c: usize, data: &'a [u8]) -> View<'a> {
        assert_eq!(data.len(), w * h * c, "pixel buffer size");
        View { w, h, c, data }
    }

    pub fn alpha(&self) -> bool {
        self.c == 4
    }

    pub fn row(&self, y: usize) -> &'a [u8] {
        &self.data[y * self.w * self.c..(y + 1) * self.w * self.c]
    }

    pub fn to_px(&self) -> Px {
        Px { w: self.w, h: self.h, c: self.c, data: self.data.to_vec() }
    }
}
