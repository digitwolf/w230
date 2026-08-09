//! Gear determination with three prioritised sources:
//!   1. Hardware neutral switch (GPIO23) -> definitive "N".
//!   2. Direct KDS gear register (the W230 has none — kept for other models).
//!   3. RPM/speed ratio classifier with per-gear bands, gated by the clutch
//!      switch (ECU reg 0x03): with the lever pulled the engine is decoupled,
//!      so the ratio is meaningless and classification pauses.
//!
//! `Gear::Unknown` renders as a dash on the matrix.

use log::info;

pub const NUM_GEARS: usize = 6; // W230 is a 6-speed

const MIN_SPEED: f32 = 3.0; // below this the ratio is meaningless
const MIN_RPM: f32 = 600.0;
const DEBOUNCE_N: u8 = 3; // consecutive samples before committing
const BAND_TOL: f32 = 0.14; // ±14% window around a band centre


#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Gear {
    Unknown,
    Neutral,
    G(u8), // 1..=6
}

pub struct GearEstimator {
    /// Learned ratio bands; `None` until a calibration ride has covered all
    /// gears — the classifier stays silent rather than guess from placeholders.
    bands: Option<[f32; NUM_GEARS]>,
    gear: Gear,
    candidate: Gear,
    stable: u8,
}

impl GearEstimator {
    pub fn new() -> Self {
        Self {
            bands: None,
            gear: Gear::Unknown,
            candidate: Gear::Unknown,
            stable: 0,
        }
    }

    pub fn set_bands(&mut self, bands: [f32; NUM_GEARS]) {
        self.bands = Some(bands);
    }

    pub fn clear_bands(&mut self) {
        self.bands = None;
    }

    pub fn bands(&self) -> Option<[f32; NUM_GEARS]> {
        self.bands
    }

    /// Feed the latest sample; returns the debounced gear.
    pub fn update(
        &mut self,
        rpm: Option<f32>,
        speed: Option<f32>,
        gear_reg: Option<u8>,
        neutral_switch: bool,
        clutch_pulled: bool,
    ) -> Gear {
        // The hardware neutral switch is definitive — no debounce in either
        // direction. Entering N: show it now. Leaving N: drop it now (holding
        // a stale N invites dropping the clutch in gear).
        if neutral_switch {
            if self.gear != Gear::Neutral {
                info!("GEAR: {:?} -> Neutral (switch)", self.gear);
            }
            self.gear = Gear::Neutral;
            self.candidate = Gear::Neutral;
            self.stable = DEBOUNCE_N;
            return self.gear;
        }
        if self.gear == Gear::Neutral {
            info!("GEAR: Neutral -> Unknown (switch released)");
            self.gear = Gear::Unknown;
            self.candidate = Gear::Unknown;
            self.stable = 1;
        }

        let raw = if let Some(g) = gear_reg.filter(|g| *g as usize <= NUM_GEARS) {
            // 2) Plausible KDS gear register (0 = neutral per ECU map).
            if g == 0 {
                Gear::Neutral
            } else {
                Gear::G(g)
            }
        } else if clutch_pulled {
            // Engine decoupled — the ratio says nothing; hold the last gear.
            Gear::Unknown
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
        // Unknown never displaces a known gear here (coasting clutch-in
        // mid-ride shouldn't blank the digit); stale "N" is impossible — the
        // switch path above commits and drops N without debounce.
        if self.stable >= DEBOUNCE_N && self.candidate != Gear::Unknown {
            if self.gear != self.candidate {
                info!("GEAR: {:?} -> {:?}", self.gear, self.candidate);
            }
            self.gear = self.candidate;
        }
        self.gear
    }

    fn classify(&self, rpm: Option<f32>, speed: Option<f32>) -> Gear {
        let Some(bands) = &self.bands else {
            return Gear::Unknown; // not calibrated yet
        };
        let (rpm, speed) = match (rpm, speed) {
            (Some(r), Some(s)) if r >= MIN_RPM && s >= MIN_SPEED => (r, s),
            _ => return Gear::Unknown,
        };
        let ratio = rpm / speed;
        let mut best: Option<(usize, f32)> = None;
        for (i, c) in bands.iter().enumerate() {
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
