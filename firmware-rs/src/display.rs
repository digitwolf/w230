//! 5x5 RGB matrix rendering for the M5Stack ATOM Matrix (25x WS2812, GPIO27).
//!
//! The whole matrix is the gear digit: N (green), 1-5 (cyan), dash (dim red)
//! for unknown. The top-left pixel is overridden as a status dot when the
//! K-line link is down.

use crate::gear::Gear;
use smart_leds::RGB8;

/// 5x5 glyphs, one u8 row bitmask each (bit 4 = leftmost column).
const GLYPH_N: [u8; 5] = [0b10001, 0b11001, 0b10101, 0b10011, 0b10001];
const GLYPH_1: [u8; 5] = [0b00100, 0b01100, 0b00100, 0b00100, 0b01110];
const GLYPH_2: [u8; 5] = [0b01110, 0b00010, 0b01110, 0b01000, 0b01110];
const GLYPH_3: [u8; 5] = [0b01110, 0b00010, 0b00110, 0b00010, 0b01110];
const GLYPH_4: [u8; 5] = [0b01010, 0b01010, 0b01110, 0b00010, 0b00010];
const GLYPH_5: [u8; 5] = [0b01110, 0b01000, 0b01110, 0b00010, 0b01110];
const GLYPH_DASH: [u8; 5] = [0b00000, 0b00000, 0b01110, 0b00000, 0b00000];

const GREEN: RGB8 = RGB8::new(0, 255, 40);
const CYAN: RGB8 = RGB8::new(0, 180, 255);
const DIM_RED: RGB8 = RGB8::new(120, 10, 0);
const STATUS_RED: RGB8 = RGB8::new(255, 0, 0);
const OFF: RGB8 = RGB8::new(0, 0, 0);

/// Render the gear into a 25-pixel row-major frame buffer.
/// `brightness` scales 0..=255. `link_up` = K-line connected.
pub fn render(gear: Gear, link_up: bool, brightness: u8) -> [RGB8; 25] {
    let (glyph, colour) = match gear {
        Gear::Neutral => (&GLYPH_N, GREEN),
        Gear::G(1) => (&GLYPH_1, CYAN),
        Gear::G(2) => (&GLYPH_2, CYAN),
        Gear::G(3) => (&GLYPH_3, CYAN),
        Gear::G(4) => (&GLYPH_4, CYAN),
        Gear::G(5) => (&GLYPH_5, CYAN),
        _ => (&GLYPH_DASH, DIM_RED),
    };

    let mut px = [OFF; 25];
    for (row, mask) in glyph.iter().enumerate() {
        for col in 0..5 {
            if mask & (1 << (4 - col)) != 0 {
                px[row * 5 + col] = scale(colour, brightness);
            }
        }
    }
    if !link_up {
        px[0] = scale(STATUS_RED, brightness); // top-left: NO-LINK dot
    }
    px
}

fn scale(c: RGB8, brightness: u8) -> RGB8 {
    let s = |v: u8| ((v as u16 * brightness as u16) / 255) as u8;
    RGB8::new(s(c.r), s(c.g), s(c.b))
}
