//! 5x5 RGB matrix rendering for the M5Stack ATOM Matrix (25x WS2812).
//!
//! The whole matrix is the gear digit: N (green), 1-6 (cyan), dash (dim red)
//! for unknown. When the K-line link is down the whole matrix goes red.

use crate::gear::Gear;
use rgb::RGB8;

/// 5x5 glyphs, one u8 row bitmask each (bit 4 = leftmost column).
const GLYPH_N: [u8; 5] = [0b10001, 0b11001, 0b10101, 0b10011, 0b10001];
const GLYPH_1: [u8; 5] = [0b00100, 0b01100, 0b00100, 0b00100, 0b01110];
const GLYPH_2: [u8; 5] = [0b01110, 0b00010, 0b01110, 0b01000, 0b01110];
const GLYPH_3: [u8; 5] = [0b01110, 0b00010, 0b00110, 0b00010, 0b01110];
const GLYPH_4: [u8; 5] = [0b01010, 0b01010, 0b01110, 0b00010, 0b00010];
const GLYPH_5: [u8; 5] = [0b01110, 0b01000, 0b01110, 0b00010, 0b01110];
const GLYPH_6: [u8; 5] = [0b01110, 0b01000, 0b01110, 0b01010, 0b01110];
const GLYPH_DASH: [u8; 5] = [0b00000, 0b00000, 0b01110, 0b00000, 0b00000];

const GREEN: RGB8 = RGB8::new(0, 255, 40);
const CYAN: RGB8 = RGB8::new(0, 180, 255);
const DIM_RED: RGB8 = RGB8::new(120, 10, 0);
const DIM_GREEN: RGB8 = RGB8::new(0, 140, 20);
const STATUS_RED: RGB8 = RGB8::new(255, 0, 0);
const OFF: RGB8 = RGB8::new(0, 0, 0);

/// Render the gear into a 25-pixel row-major frame buffer.
/// `brightness` scales 0..=255. `link_up` = K-line connected.
/// `learn_flash` recolours the unknown-gear dash green — the "calibration
/// data just persisted" heartbeat, visible without the dashboard.
pub fn render(gear: Gear, link_up: bool, brightness: u8, learn_flash: bool) -> [RGB8; 25] {
    if !link_up {
        return [scale(STATUS_RED, brightness); 25]; // NO-LINK: all red
    }

    let dash_colour = if learn_flash { DIM_GREEN } else { DIM_RED };
    let (glyph, colour) = match gear {
        Gear::Neutral => (&GLYPH_N, GREEN),
        Gear::G(1) => (&GLYPH_1, CYAN),
        Gear::G(2) => (&GLYPH_2, CYAN),
        Gear::G(3) => (&GLYPH_3, CYAN),
        Gear::G(4) => (&GLYPH_4, CYAN),
        Gear::G(5) => (&GLYPH_5, CYAN),
        Gear::G(6) => (&GLYPH_6, CYAN),
        _ => (&GLYPH_DASH, dash_colour),
    };

    let mut px = [OFF; 25];
    for (row, mask) in glyph.iter().enumerate() {
        for col in 0..5 {
            if mask & (1 << (4 - col)) != 0 {
                px[row * 5 + col] = scale(colour, brightness);
            }
        }
    }
    px
}

fn scale(c: RGB8, brightness: u8) -> RGB8 {
    let s = |v: u8| ((v as u16 * brightness as u16) / 255) as u8;
    RGB8::new(s(c.r), s(c.g), s(c.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(frame: &[RGB8; 25]) -> usize {
        frame
            .iter()
            .filter(|p| p.r as u16 + p.g as u16 + p.b as u16 > 0)
            .count()
    }

    #[test]
    fn no_link_is_all_red() {
        let f = render(Gear::Neutral, false, 255, false);
        assert!(f.iter().all(|p| *p == RGB8::new(255, 0, 0)));
    }

    #[test]
    fn neutral_is_green_n() {
        let f = render(Gear::Neutral, true, 255, false);
        let expected: u32 = GLYPH_N.iter().map(|m| m.count_ones()).sum();
        assert_eq!(lit(&f) as u32, expected);
        assert!(f.iter().all(|p| p.r == 0)); // green has no red component
    }

    #[test]
    fn every_gear_digit_renders_in_cyan() {
        for g in 1..=6u8 {
            let f = render(Gear::G(g), true, 255, false);
            assert!(lit(&f) > 0, "gear {g} rendered nothing");
            assert!(
                f.iter().filter(|p| **p != OFF).all(|p| *p == CYAN),
                "gear {g} not cyan"
            );
        }
    }

    #[test]
    fn unknown_is_the_dim_red_dash() {
        let f = render(Gear::Unknown, true, 255, false);
        assert_eq!(lit(&f), 3); // middle-row 3-pixel dash
        assert_eq!(f[10], OFF);
        assert_eq!(f[11], DIM_RED);
        assert_eq!(f[12], DIM_RED);
        assert_eq!(f[13], DIM_RED);
    }

    #[test]
    fn brightness_zero_blanks_everything() {
        assert_eq!(lit(&render(Gear::Neutral, true, 0, false)), 0);
        assert_eq!(lit(&render(Gear::Neutral, false, 0, false)), 0);
    }

    #[test]
    fn learn_flash_turns_dash_green() {
        let f = render(Gear::Unknown, true, 255, true);
        assert_eq!(f[11], DIM_GREEN);
        // Digits and N are never recoloured by the flash.
        let n = render(Gear::Neutral, true, 255, true);
        assert!(n.iter().filter(|p| **p != OFF).all(|p| *p == GREEN));
    }

    #[test]
    fn full_brightness_is_identity() {
        let f = render(Gear::G(1), true, 255, false);
        assert!(f.iter().filter(|p| **p != OFF).all(|p| *p == CYAN));
    }
}
