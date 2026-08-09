//! Gear determination with three prioritised sources:
//!   1. Hardware neutral switch -> definitive "N", no debounce either way.
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

/// One poll-cycle's worth of inputs to the estimator.
#[derive(Copy, Clone, Debug, Default)]
pub struct GearInputs {
    pub rpm: Option<f32>,
    pub speed: Option<f32>,
    /// Direct gear register, on models that have one (the W230 does not).
    pub gear_reg: Option<u8>,
    /// Hardware neutral switch: true = in neutral.
    pub neutral_switch: bool,
    /// Clutch lever pulled (from ECU reg 0x03).
    pub clutch_pulled: bool,
}

pub struct GearEstimator {
    /// Learned ratio bands; `None` until a calibration ride has covered all
    /// gears — the classifier stays silent rather than guess from placeholders.
    bands: Option<[f32; NUM_GEARS]>,
    gear: Gear,
    candidate: Gear,
    stable: u8,
}

impl Default for GearEstimator {
    fn default() -> Self {
        Self::new()
    }
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

    /// Feed the latest sample; returns the (debounced) gear to display.
    pub fn update(&mut self, inputs: &GearInputs) -> Gear {
        // The hardware neutral switch is definitive — no debounce in either
        // direction. Entering N: show it now. Leaving N: drop it now (holding
        // a stale N invites dropping the clutch in gear).
        if inputs.neutral_switch {
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

        let raw = if let Some(g) = inputs.gear_reg.filter(|g| *g as usize <= NUM_GEARS) {
            // Plausible KDS gear register (0 = neutral per ECU map).
            if g == 0 {
                Gear::Neutral
            } else {
                Gear::G(g)
            }
        } else if inputs.clutch_pulled {
            // Engine decoupled — the ratio says nothing; hold the last gear.
            Gear::Unknown
        } else {
            self.classify(inputs.rpm, inputs.speed)
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

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: [f32; NUM_GEARS] = [240.0, 165.0, 125.0, 100.0, 82.0, 70.0];

    fn calibrated() -> GearEstimator {
        let mut e = GearEstimator::new();
        e.set_bands(BANDS);
        e
    }

    fn riding(rpm: f32, speed: f32) -> GearInputs {
        GearInputs {
            rpm: Some(rpm),
            speed: Some(speed),
            ..Default::default()
        }
    }

    #[test]
    fn neutral_switch_commits_instantly() {
        let mut e = calibrated();
        let g = e.update(&GearInputs {
            neutral_switch: true,
            ..Default::default()
        });
        assert_eq!(g, Gear::Neutral); // first sample, no debounce
    }

    #[test]
    fn leaving_neutral_drops_n_instantly() {
        let mut e = calibrated();
        e.update(&GearInputs {
            neutral_switch: true,
            ..Default::default()
        });
        // Switch released, no other data (stopped, in gear):
        let g = e.update(&GearInputs::default());
        assert_ne!(g, Gear::Neutral); // stale N must not survive one sample
    }

    #[test]
    fn ratio_classification_needs_debounce() {
        let mut e = calibrated();
        // 4000 rpm / 40 km/h = ratio 100 → 4th gear, but only after 3 samples.
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::Unknown);
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::Unknown);
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::G(4));
    }

    #[test]
    fn clutch_pull_holds_last_gear() {
        let mut e = calibrated();
        for _ in 0..3 {
            e.update(&riding(4000.0, 40.0));
        }
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::G(4));
        // Clutch in while rolling: revs no longer encode the gear — hold 4.
        let mut inputs = riding(9000.0, 40.0);
        inputs.clutch_pulled = true;
        for _ in 0..5 {
            assert_eq!(e.update(&inputs), Gear::G(4));
        }
    }

    #[test]
    fn unknown_never_blanks_known_gear() {
        let mut e = calibrated();
        for _ in 0..3 {
            e.update(&riding(4000.0, 40.0));
        }
        // Ratio 199 falls in the dead zone between bands 240 (-17%) and
        // 165 (+21%) — outside every ±14% window (e.g. mid-shift): keep 4.
        for _ in 0..5 {
            assert_eq!(e.update(&riding(3980.0, 20.0)), Gear::G(4));
        }
    }

    #[test]
    fn uncalibrated_never_guesses() {
        let mut e = GearEstimator::new();
        for _ in 0..5 {
            assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::Unknown);
        }
    }

    #[test]
    fn ratio_outside_tolerance_is_unknown() {
        let mut e = calibrated();
        // Ratio 300 sits 25% above the tallest band (240) — outside ±14%.
        for _ in 0..5 {
            assert_eq!(e.update(&riding(9000.0, 30.0)), Gear::Unknown);
        }
    }

    #[test]
    fn stopped_or_slow_is_unknown() {
        let mut e = calibrated();
        for _ in 0..5 {
            assert_eq!(e.update(&riding(1200.0, 0.0)), Gear::Unknown);
        }
    }

    #[test]
    fn gear_register_takes_priority_over_ratio() {
        let mut e = calibrated();
        let mut inputs = riding(4000.0, 40.0); // ratio says 4th
        inputs.gear_reg = Some(2);
        for _ in 0..3 {
            e.update(&inputs);
        }
        assert_eq!(e.update(&inputs), Gear::G(2));
    }
}
