//! Pre-render the three popup state bitmaps (idle / recording / working).
//!
//! Rendering: draw at 4× resolution with explicit per-pixel inside/outside
//! ellipse tests (full alpha), composite the logo on top, then downsample
//! with Lanczos3 — produces clean antialiased edges for the layered window.
use std::sync::OnceLock;

use image::imageops::FilterType;
use image::{Pixel, Rgba, RgbaImage};

pub const POPUP_SIZE: u32 = 18;
pub const RING_PX: u32 = 2;
const SCALE: u32 = 4;

const BODY: Rgba<u8> = Rgba([0x1f, 0x29, 0x37, 0xFF]);
const RING_IDLE: Rgba<u8> = Rgba([0x52, 0x52, 0x5b, 0xFF]); // slightly brighter than before
const RING_RECORD: Rgba<u8> = Rgba([0x22, 0xc5, 0x5e, 0xFF]); // green
const RING_WORK: Rgba<u8> = Rgba([0xf5, 0x9e, 0x0b, 0xFF]); // amber

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupState {
    Idle = 0,
    Recording = 1,
    Working = 2,
}

impl PopupState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Recording,
            2 => Self::Working,
            _ => Self::Idle,
        }
    }
}

/// Decode the bundled logo PNG. Cached after first call.
fn logo_image() -> &'static RgbaImage {
    static LOGO: OnceLock<RgbaImage> = OnceLock::new();
    LOGO.get_or_init(|| {
        let bytes = include_bytes!("../../assets/logo.png");
        let img = image::load_from_memory(bytes).expect("logo decode");
        img.into_rgba8()
    })
}

fn fill_circle(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: Rgba<u8>) {
    let r2 = r * r;
    let (w, h) = img.dimensions();
    for y in 0..h {
        let dy = y as f32 + 0.5 - cy;
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            if dx * dx + dy * dy <= r2 {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn render_state(ring_color: Rgba<u8>) -> RgbaImage {
    let big = (POPUP_SIZE * SCALE) as i64;
    let bigu = big as u32;
    let mut img = RgbaImage::from_pixel(bigu, bigu, Rgba([0, 0, 0, 0]));

    let cx = (big as f32) / 2.0;
    let cy = (big as f32) / 2.0;
    let r_outer = (big as f32) / 2.0;
    let r_inner = r_outer - (RING_PX * SCALE) as f32;

    fill_circle(&mut img, cx, cy, r_outer, ring_color);
    fill_circle(&mut img, cx, cy, r_inner, BODY);

    // Center the logo, leaving small inner padding
    let logo = logo_image();
    let inner_box = ((r_inner - (1 * SCALE) as f32) * 2.0) as u32;
    let logo_resized = image::imageops::resize(logo, inner_box, inner_box, FilterType::Lanczos3);
    let lx = (bigu as i64 - logo_resized.width() as i64) / 2;
    let ly = (bigu as i64 - logo_resized.height() as i64) / 2;
    image::imageops::overlay(&mut img, &logo_resized, lx, ly);

    // Downsample to final size with Lanczos
    image::imageops::resize(&img, POPUP_SIZE, POPUP_SIZE, FilterType::Lanczos3)
}

/// Returns BGRA premultiplied bytes for UpdateLayeredWindow.
pub fn rendered_bgra(state: PopupState) -> Vec<u8> {
    static CACHE: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        [
            to_bgra_premul(&render_state(RING_IDLE)),
            to_bgra_premul(&render_state(RING_RECORD)),
            to_bgra_premul(&render_state(RING_WORK)),
        ]
    });
    cache[state as usize].clone()
}

fn to_bgra_premul(rgba: &RgbaImage) -> Vec<u8> {
    let (w, h) = rgba.dimensions();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in rgba.pixels() {
        let [r, g, b, a] = px.0;
        let a16 = a as u16;
        let r = ((r as u16 * a16) / 255) as u8;
        let g = ((g as u16 * a16) / 255) as u8;
        let b = ((b as u16 * a16) / 255) as u8;
        // BGRA byte order
        out.push(b);
        out.push(g);
        out.push(r);
        out.push(a);
    }
    out
}
