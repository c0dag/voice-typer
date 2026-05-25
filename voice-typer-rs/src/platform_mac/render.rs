//! macOS popup rendering: the same circular badge as Windows (dark body +
//! state-colored ring + centered logo), produced with the `image` crate and
//! written to PNG files that AppKit loads as NSImages.

use std::path::PathBuf;
use std::sync::OnceLock;

use image::imageops::FilterType;
use image::{Rgba, RgbaImage};

pub const POPUP_SIZE: u32 = 72; // hi-res master; AppKit downscales to ~19pt (Retina-crisp)
pub const RING_PX: u32 = 6; // ~2/18 ratio like Windows
const SCALE: u32 = 4;

const BODY: Rgba<u8> = Rgba([0x1f, 0x29, 0x37, 0xFF]);
const RING_IDLE: Rgba<u8> = Rgba([0x52, 0x52, 0x5b, 0xFF]);
const RING_RECORD: Rgba<u8> = Rgba([0x22, 0xc5, 0x5e, 0xFF]); // green
const RING_WORK: Rgba<u8> = Rgba([0xf5, 0x9e, 0x0b, 0xFF]); // amber

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PopupState {
    Idle = 0,
    Recording = 1,
    Working = 2,
}

#[allow(dead_code)]
impl PopupState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => PopupState::Recording,
            2 => PopupState::Working,
            _ => PopupState::Idle,
        }
    }
}

fn logo_image() -> &'static RgbaImage {
    static LOGO: OnceLock<RgbaImage> = OnceLock::new();
    LOGO.get_or_init(|| {
        let bytes = include_bytes!("../../assets/logo.png");
        image::load_from_memory(bytes).expect("logo decode").into_rgba8()
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

    let logo = logo_image();
    let inner_box = ((r_inner - (3 * SCALE) as f32) * 2.0) as u32;
    let logo_resized = image::imageops::resize(logo, inner_box, inner_box, FilterType::Lanczos3);
    let lx = (bigu as i64 - logo_resized.width() as i64) / 2;
    let ly = (bigu as i64 - logo_resized.height() as i64) / 2;
    image::imageops::overlay(&mut img, &logo_resized, lx, ly);

    image::imageops::resize(&img, POPUP_SIZE, POPUP_SIZE, FilterType::Lanczos3)
}

/// Render the three state badges to PNG files under `dir`, returning their
/// paths in order [Idle, Recording, Working]. Called once at popup startup.
pub fn write_state_pngs(dir: &std::path::Path) -> std::io::Result<[PathBuf; 3]> {
    std::fs::create_dir_all(dir)?;
    let states = [
        ("idle", RING_IDLE),
        ("recording", RING_RECORD),
        ("working", RING_WORK),
    ];
    let mut out: Vec<PathBuf> = Vec::with_capacity(3);
    for (name, color) in states {
        let img = render_state(color);
        let path = dir.join(format!("popup-{name}.png"));
        img.save(&path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        out.push(path);
    }
    Ok([out[0].clone(), out[1].clone(), out[2].clone()])
}
