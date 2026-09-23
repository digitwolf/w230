//! Gear determination with three prioritised sources:
//!   1. Hardware neutral switch -> definitive "N", no debounce either way.
//!   2. Direct KDS gear register (the W230 has none — kept for other models).
//!   3. RPM/speed ratio classifier with per-gear bands. An optional clutch
//!      input pauses classification (engine decoupled = ratio meaningless);
//!      the W230 firmware has no usable clutch source and leaves it false.
//!
//! `Gear::Unknown` renders as a dash on the matrix.

use log::info;

pub const NUM_GEARS: usize = 6; // W230 is a 6-speed

const MIN_SPEED: f32 = 3.0; // below this the ratio is meaningless
const MIN_RPM: f32 = 600.0;
const DEBOUNCE_N: u8 = 3; // consecutive samples before committing
/// A confident classification (very close to a band centre) commits faster.
const DEBOUNCE_CONFIDENT: u8 = 2;
const BAND_TOL: f32 = 0.14; // ±14% window around a band centre
/// Within this of a band centre the classification counts as confident.
const CONFIDENT_TOL: f32 = 0.05;
/// Speed below this counts as standing still.
const STANDSTILL_SPEED: f32 = 1.0;
/// Consecutive standstill samples after which a held gear digit decays to the
/// dash: stopped with the clutch in, the rider may downshift without the box
/// ever telling us, so a stale digit becomes a lie after a few seconds.
const STANDSTILL_FORGET: u8 = 8;
/// Launch detection: below this speed, a ratio at-or-above the 1st-gear band
/// can only be a slipping clutch on a launch — show 1st before hookup.
const LAUNCH_MAX_SPEED: f32 = 12.0;
const LAUNCH_MIN_RPM: f32 = 1100.0;

/// Factory-provisional ratio bands (rpm per km/h), computed from Kawasaki's
/// official 2026 W230 spec: primary 2.871, final 2.714, gears 3.000/2.067/
/// 1.556/1.261/1.040/0.852, rear tire 110/90-17 (1.98 m rollout). Used until
/// ride-learned bands replace them (which also absorb speedo optimism and
/// tire wear — expect learned values a few percent off these).
pub const FACTORY_BANDS: [f32; NUM_GEARS] = [196.9, 135.7, 102.1, 82.8, 68.3, 55.9];

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Gear {
    #[default]
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
    /// Learned ratio bands and how many are valid (partial calibration maps
    /// the highest ratio to 1st gear); `None` until a calibration ride —
    /// the classifier stays silent rather than guess from placeholders.
    bands: Option<([f32; NUM_GEARS], usize)>,
    gear: Gear,
    candidate: Gear,
    candidate_confident: bool,
    stable: u8,
    standstill: u8,
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
            candidate_confident: false,
            stable: 0,
            standstill: 0,
        }
    }

    pub fn set_bands(&mut self, bands: [f32; NUM_GEARS], count: usize) {
        assert!((1..=NUM_GEARS).contains(&count));
        self.bands = Some((bands, count));
    }

    pub fn clear_bands(&mut self) {
        self.bands = None;
    }

    /// The valid learned bands (length = calibrated gear count).
    pub fn bands(&self) -> Option<&[f32]> {
        self.bands.as_ref().map(|(b, c)| &b[..*c])
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
            self.standstill = 0;
            return self.gear;
        }
        if self.gear == Gear::Neutral {
            info!("GEAR: Neutral -> Unknown (switch released)");
            self.gear = Gear::Unknown;
            self.candidate = Gear::Unknown;
            self.stable = 1;
        }

        // Standstill decay: a held digit becomes untrustworthy after a few
        // seconds stopped (clutch-in downshifts are invisible to us).
        match inputs.speed {
            Some(s) if s < STANDSTILL_SPEED && self.gear != Gear::Unknown => {
                self.standstill = self.standstill.saturating_add(1);
                if self.standstill >= STANDSTILL_FORGET {
                    info!("GEAR: {:?} -> Unknown (standing still)", self.gear);
                    self.gear = Gear::Unknown;
                    self.candidate = Gear::Unknown;
                    self.standstill = 0;
                }
            }
            Some(_) => self.standstill = 0,
            None => {}
        }

        let (raw, confident) = if let Some(g) = inputs.gear_reg.filter(|g| *g as usize <= NUM_GEARS)
        {
            // Plausible KDS gear register (0 = neutral per ECU map).
            if g == 0 {
                (Gear::Neutral, true)
            } else {
                (Gear::G(g), true)
            }
        } else if inputs.clutch_pulled {
            // Engine decoupled — the ratio says nothing; hold the last gear.
            (Gear::Unknown, false)
        } else {
            self.classify(inputs.rpm, inputs.speed)
        };

        // Debounce so a transient sample doesn't flicker the display; a
        // confident classification (ratio near a band centre) commits sooner.
        if raw == self.candidate {
            self.stable = self.stable.saturating_add(1);
            self.candidate_confident = confident;
        } else {
            self.candidate = raw;
            self.candidate_confident = confident;
            self.stable = 1;
        }
        let needed = if self.candidate_confident {
            DEBOUNCE_CONFIDENT
        } else {
            DEBOUNCE_N
        };
        // Unknown never displaces a known gear here (coasting clutch-in
        // mid-ride shouldn't blank the digit); stale "N" is impossible — the
        // switch path above commits and drops N without debounce.
        if self.stable >= needed && self.candidate != Gear::Unknown {
            if self.gear != self.candidate {
                info!("GEAR: {:?} -> {:?}", self.gear, self.candidate);
            }
            self.gear = self.candidate;
        }
        self.gear
    }

    /// Classify a ratio to the nearest band; the bool is the confidence
    /// (within [`CONFIDENT_TOL`] of the band centre).
    fn classify(&self, rpm: Option<f32>, speed: Option<f32>) -> (Gear, bool) {
        let Some((bands, count)) = &self.bands else {
            return (Gear::Unknown, false); // not calibrated yet
        };
        let (rpm, speed) = match (rpm, speed) {
            (Some(r), Some(s)) if r >= MIN_RPM && s >= MIN_SPEED => (r, s),
            _ => return (Gear::Unknown, false),
        };
        let ratio = rpm / speed;
        let mut best: Option<(usize, f32)> = None;
        for (i, c) in bands[..*count].iter().enumerate() {
            let err = (ratio - c).abs() / c;
            if best.map_or(true, |(_, be)| err < be) {
                best = Some((i, err));
            }
        }
        match best {
            Some((i, err)) if err <= BAND_TOL => (Gear::G(i as u8 + 1), err <= CONFIDENT_TOL),
            _ => {
                // Launch: slipping the clutch from a stop pushes the ratio
                // ABOVE the 1st-gear band while speed is still walking pace.
                // No other state produces that signature, so show 1st early
                // rather than a dash until hookup (~10 km/h).
                if speed <= LAUNCH_MAX_SPEED
                    && rpm >= LAUNCH_MIN_RPM
                    && ratio >= bands[0] * (1.0 - BAND_TOL)
                {
                    (Gear::G(1), false)
                } else {
                    (Gear::Unknown, false)
                }
            }
        }
    }
}

/// First-order time alignment for the rpm/speed pair: the two registers are
/// read ~a hundred ms apart, so under acceleration the ratio skews. Given the
/// previous cycle's rpm reading `dt_ms` ago, estimate rpm `lead_ms` after the
/// current reading (i.e. at the moment the speed byte was captured). The
/// correction is clamped to ±10% and disabled for stale history.
pub fn time_align_rpm(rpm_now: f32, rpm_prev: f32, dt_ms: f32, lead_ms: f32) -> f32 {
    if !(1.0..=2000.0).contains(&dt_ms) || lead_ms <= 0.0 {
        return rpm_now;
    }
    let slope = (rpm_now - rpm_prev) / dt_ms; // rpm per ms
    let corrected = rpm_now + slope * lead_ms;
    let limit = rpm_now * 0.10;
    corrected.clamp(rpm_now - limit, rpm_now + limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: [f32; NUM_GEARS] = [240.0, 165.0, 125.0, 100.0, 82.0, 70.0];

    fn calibrated() -> GearEstimator {
        let mut e = GearEstimator::new();
        e.set_bands(BANDS, NUM_GEARS);
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
    fn confident_ratio_commits_in_two_samples() {
        let mut e = calibrated();
        // 4000 rpm / 40 km/h = ratio 100, dead on the 4th-gear band centre.
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::Unknown);
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::G(4));
    }

    #[test]
    fn marginal_ratio_needs_full_debounce() {
        let mut e = calibrated();
        // Ratio 109.6: within the 4th-gear ±14% window but 9.6% off centre —
        // classified, yet not confident → three samples.
        assert_eq!(e.update(&riding(4000.0, 36.5)), Gear::Unknown);
        assert_eq!(e.update(&riding(4000.0, 36.5)), Gear::Unknown);
        assert_eq!(e.update(&riding(4000.0, 36.5)), Gear::G(4));
    }

    #[test]
    fn held_gear_decays_to_unknown_at_standstill() {
        let mut e = calibrated();
        for _ in 0..3 {
            e.update(&riding(4000.0, 40.0));
        }
        assert_eq!(e.update(&riding(4000.0, 40.0)), Gear::G(4));
        // Stopped in gear, clutch in, idling: digit must decay, not persist.
        let mut stopped = riding(1300.0, 0.0);
        stopped.clutch_pulled = true;
        for _ in 0..7 {
            assert_eq!(e.update(&stopped), Gear::G(4)); // grace period holds
        }
        assert_eq!(e.update(&stopped), Gear::Unknown); // 8th sample: decay
    }

    #[test]
    fn time_align_rpm_corrects_acceleration_skew() {
        // Steady state: no correction.
        assert_eq!(time_align_rpm(4000.0, 4000.0, 350.0, 130.0), 4000.0);
        // Accelerating 1000→1100 rpm over 350ms, speed read 130ms later:
        // expect ~+37 rpm.
        let v = time_align_rpm(1100.0, 1000.0, 350.0, 130.0);
        assert!((v - 1137.0).abs() < 1.0, "got {v}");
        // Absurd slope clamps at ±10%.
        assert_eq!(time_align_rpm(1000.0, 100.0, 10.0, 130.0), 1100.0);
        // Stale history disables the correction.
        assert_eq!(time_align_rpm(1100.0, 1000.0, 5000.0, 130.0), 1100.0);
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
    fn partial_calibration_classifies_only_learned_gears() {
        let mut e = GearEstimator::new();
        e.set_bands(BANDS, 2); // only 1st (240) and 2nd (165) learned
                               // Riding at 2nd-gear ratio: classified.
        for _ in 0..3 {
            e.update(&riding(3300.0, 20.0)); // ratio 165
        }
        assert_eq!(e.update(&riding(3300.0, 20.0)), Gear::G(2));
        // Riding at what would be 4th (ratio 100): outside the learned set.
        let mut e2 = GearEstimator::new();
        e2.set_bands(BANDS, 2);
        for _ in 0..5 {
            assert_eq!(e2.update(&riding(4000.0, 40.0)), Gear::Unknown);
        }
    }

    #[test]
    fn launch_slip_shows_first_gear_early() {
        let mut e = calibrated();
        // Pulling away: 2500 rpm at 8 km/h → ratio 312, far above 1st's 240.
        for _ in 0..3 {
            e.update(&riding(2500.0, 8.0));
        }
        assert_eq!(e.update(&riding(2500.0, 8.0)), Gear::G(1));
        // But a low-speed ratio in the dead zone BELOW the launch signature
        // must stay unknown: 1170 rpm at 6 km/h → ratio 195 (between the 165
        // band's +14% and the launch floor of 240·0.86 ≈ 206).
        let mut e2 = calibrated();
        for _ in 0..5 {
            assert_eq!(e2.update(&riding(1170.0, 6.0)), Gear::Unknown);
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
