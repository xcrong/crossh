//! UI-independent Crossh color tokens.
//!
//! Renderers convert [`Rgb`] into their native color type. Keeping the
//! palette here prevents GPUI and terminal surfaces from drifting apart.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(u32);

impl Rgb {
    pub const fn from_hex(value: u32) -> Self {
        Self(value & 0x00ff_ffff)
    }

    pub const fn hex(self) -> u32 {
        self.0
    }

    pub const fn channels(self) -> (u8, u8, u8) {
        ((self.0 >> 16) as u8, (self.0 >> 8) as u8, self.0 as u8)
    }
}

pub const fn canvas() -> Rgb {
    Rgb::from_hex(0x0d1014)
}

pub const fn sidebar() -> Rgb {
    Rgb::from_hex(0x12171c)
}

pub const fn surface() -> Rgb {
    Rgb::from_hex(0x171d23)
}

pub const fn raised() -> Rgb {
    Rgb::from_hex(0x202930)
}

pub const fn border() -> Rgb {
    Rgb::from_hex(0x28323a)
}

pub const fn border_strong() -> Rgb {
    Rgb::from_hex(0x3a4854)
}

pub const fn overlay() -> Rgb {
    Rgb::from_hex(0x262f38)
}

pub const fn text() -> Rgb {
    Rgb::from_hex(0xe7edf1)
}

pub const fn muted_text() -> Rgb {
    Rgb::from_hex(0x9aa6b0)
}

pub const fn faint_text() -> Rgb {
    Rgb::from_hex(0x65717c)
}

pub const fn accent() -> Rgb {
    Rgb::from_hex(0x7de0bd)
}

pub const fn accent_hover() -> Rgb {
    Rgb::from_hex(0x95efd0)
}

pub const fn accent_soft() -> Rgb {
    Rgb::from_hex(0x173a34)
}

pub const fn info() -> Rgb {
    Rgb::from_hex(0x87bfff)
}

pub const fn warning() -> Rgb {
    Rgb::from_hex(0xf3c66e)
}

pub const fn danger() -> Rgb {
    Rgb::from_hex(0xf28b8b)
}

pub const fn danger_hover() -> Rgb {
    Rgb::from_hex(0xffa4a4)
}

pub const fn diff_add_bg() -> Rgb {
    Rgb::from_hex(0x1c3327)
}

pub const fn diff_add_fg() -> Rgb {
    Rgb::from_hex(0x8fe3b0)
}

pub const fn diff_del_bg() -> Rgb {
    Rgb::from_hex(0x3a2222)
}

pub const fn diff_del_fg() -> Rgb {
    Rgb::from_hex(0xf2a2a2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_tokens_expose_expected_channels() {
        assert_eq!(canvas().channels(), (13, 16, 20));
        assert_eq!(accent().channels(), (125, 224, 189));
        assert_eq!(danger().hex(), 0xf28b8b);
    }
}
