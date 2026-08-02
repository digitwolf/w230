//! Gear determination with three prioritised sources:
//!   1. Neutral switch  -> definitive "N".
//!   2. Direct KDS gear register (if the W230 exposes one).
//!   3. RPM/speed ratio classifier with per-gear bands.
//!
//! `Gear::Unknown` renders as a dash on the matrix.

use log::info;

pub const NUM_GEARS: usize = 5; // W230 is a 5-speed

const MIN_SPEED: f32 = 3.0; // below this the ratio is meaningless
const MIN_RPM: f32 = 600.0;
const DEBOUNCE_N: u8 = 3; // consecutive samples before committing
const BAND_TOL: f32 = 0.14; // ±14% window around a band centre

/// Default ratio bands (unitless rpm/speed) — placeholders until calibrated.
pub const DEFAULT_BANDS: [f32; NUM_GEARS] = [240.0, 165.0, 125.0, 100.0, 82.0];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Gear {
    Unknown,
    Neutral,
    G(u8), // 1..=5
}

pub struct GearEstimator {
    pub bands: [f32; NUM_GEARS],
    gear: Gear,
    candidate: Gear,
    stable: u8,
}

impl GearEstimator {
    pub fn new(bands: [f32; NUM_GEARS]) -> Self {
        Self {
            bands,
            gear: Gear::Unknown,
            candidate: Gear::Unknown,
            stable: 0,
        }
    }

    /// Feed the latest sample; returns the debounced gear.
    pub fn update(
        &mut self,
        rpm: Option<f32>,
        speed: Option<f32>,
        gear_reg: Option<u8>,
        neutral_switch: bool,
    ) -> Gear {
        let raw = if neutral_switch {
            // 1) Hardware neutral switch wins outright.
            Gear::Neutral
        } else if let Some(g) = gear_reg.filter(|g| *g as usize <= NUM_GEARS) {
            // 2) Plausible KDS gear register (0 = neutral per ECU map).
            if g == 0 {
                Gear::Neutral
            } else {
                Gear::G(g)
            }
        } else {
            // 3) Ratio fallback.
            self.classify(rpm, speed)
        };

        // Debounce so a transient sample doesn't flicker the display.
        if raw == self.candidate {
            self.stable = self.stable.saturating_add(1);
        } else {
            self.candidate = raw;
            self.stable = 1;
        }
        if self.stable >= DEBOUNCE_N && self.candidate != Gear::Unknown {
            if self.gear != self.candidate {
                info!("GEAR: {:?} -> {:?}", self.gear, self.candidate);
            }
            self.gear = self.candidate;
        }
        self.gear
    }

    fn classify(&self, rpm: Option<f32>, speed: Option<f32>) -> Gear {
        let (rpm, speed) = match (rpm, speed) {
            (Some(r), Some(s)) if r >= MIN_RPM && s >= MIN_SPEED => (r, s),
            _ => return Gear::Unknown,
        };
        let ratio = rpm / speed;
        let mut best: Option<(usize, f32)> = None;
        for (i, c) in self.bands.iter().enumerate() {
            let err = (ratio - c).abs() / c;
            if best.map_or(true, |(_, be)| err < be) {
                best = Some((i, err));
            }
        }
        match best {
            Some((i, err)) if err <= BAND_TOL => Gear::G(i as u8 + 1),
            _ => Gear::Unknown,
        }
    }
}
