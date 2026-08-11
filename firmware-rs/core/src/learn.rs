//! Self-learning gear-ratio calibration — the pure math.
//!
//! While riding (clutch out, moving, engine turning) every RPM/speed sample is
//! quantised into a 300-bin histogram of `rpm/speed` ratios — the decimation
//! that lets unlimited ride time fit in a fixed 604-byte blob. Steady riding
//! piles up one sharp peak per gear; [`RatioHistogram::derive_bands`] extracts
//! the six peaks and turns them into the classifier's ratio bands.
//!
//! Persistence (NVS) lives in the firmware crate; this type only serialises
//! to/from the flat blob.

use log::info;

use crate::gear::NUM_GEARS;

pub const RATIO_MIN: f32 = 20.0;
pub const BINS: usize = 300; // 1.0-wide bins covering ratio 20..320
pub const BLOB_LEN: usize = 4 + BINS * 2; // [samples: u32 LE][counts: u16 LE × BINS]

const MIN_RPM: f32 = 1200.0;
/// Below this the 1-byte integer speed quantises too coarsely (±8% ratio
/// error at 6 km/h) and smears the low-gear peaks.
const MIN_SPEED: f32 = 10.0;
/// Samples in a peak's 3-bin neighbourhood before it counts as a gear.
const MIN_PEAK_MASS: u32 = 15;
/// Two peaks closer than this (relative ratio) are one gear, keep the bigger.
const PEAK_SEPARATION: f32 = 0.06;
/// Adjacent learned bands must step by a plausible gearbox factor; anything
/// outside means a fake peak got in (or a gear was skipped during a partial
/// ride), and the calibration is rejected whole.
const STEP_MIN: f32 = 1.08;
const STEP_MAX: f32 = 1.60;
/// Fewest peaks that make a usable partial calibration.
const MIN_PEAKS: usize = 2;

/// Why a sample was or wasn't counted — the firmware persists per-ride
/// tallies of these so a USB-less ride can be debugged afterwards.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SampleOutcome {
    Accepted,
    ClutchPulled,
    InNeutral,
    RpmLow,
    SpeedLow,
    OutOfRange,
    BinFull,
}

/// Fixed-size decimating histogram of rpm/speed ratios.
#[derive(Clone)]
pub struct RatioHistogram {
    hist: [u16; BINS],
    samples: u32,
}

impl Default for RatioHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl RatioHistogram {
    pub fn new() -> Self {
        Self {
            hist: [0; BINS],
            samples: 0,
        }
    }

    pub fn hist(&self) -> &[u16; BINS] {
        &self.hist
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn clear(&mut self) {
        self.hist = [0; BINS];
        self.samples = 0;
    }

    /// Feed one poll-loop sample. Counted only when the bike is moving under
    /// engine power, in gear, with the clutch out — the only state where
    /// rpm/speed encodes the gear. (`in_neutral` guards against coasting in
    /// neutral, where the engine is decoupled and the ratio is garbage.)
    pub fn add_sample(
        &mut self,
        rpm: f32,
        speed: f32,
        clutch_pulled: bool,
        in_neutral: bool,
    ) -> SampleOutcome {
        if clutch_pulled {
            return SampleOutcome::ClutchPulled;
        }
        if in_neutral {
            return SampleOutcome::InNeutral;
        }
        if rpm < MIN_RPM {
            return SampleOutcome::RpmLow;
        }
        if speed < MIN_SPEED {
            return SampleOutcome::SpeedLow;
        }
        let bin = (rpm / speed - RATIO_MIN).floor();
        if !(0.0..BINS as f32).contains(&bin) {
            return SampleOutcome::OutOfRange;
        }
        let bin = bin as usize;
        if self.hist[bin] < u16::MAX {
            self.hist[bin] += 1;
            self.samples = self.samples.saturating_add(1);
            SampleOutcome::Accepted
        } else {
            SampleOutcome::BinFull
        }
    }

    /// TEMPORARY diagnostics: bump the lowest bin (ratio ~20.5, impossibly
    /// low for any real gear) so the persistence path is provable end-to-end
    /// with no riding data. Capped far below `MIN_PEAK_MASS` so the marker
    /// can never form a peak or affect calibration.
    pub fn debug_mark(&mut self) -> bool {
        if self.hist[0] >= 5 {
            return false;
        }
        self.hist[0] += 1;
        self.samples = self.samples.saturating_add(1);
        true
    }

    /// Serialise to the flat NVS blob layout.
    pub fn to_blob(&self) -> [u8; BLOB_LEN] {
        let mut buf = [0u8; BLOB_LEN];
        buf[..4].copy_from_slice(&self.samples.to_le_bytes());
        for (i, c) in self.hist.iter().enumerate() {
            buf[4 + i * 2..4 + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        buf
    }

    /// Deserialise from the flat blob; `None` on a size mismatch.
    pub fn from_blob(data: &[u8]) -> Option<Self> {
        if data.len() != BLOB_LEN {
            return None;
        }
        let mut s = Self::new();
        s.samples = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        for (i, chunk) in data[4..].chunks_exact(2).enumerate() {
            s.hist[i] = u16::from_le_bytes([chunk[0], chunk[1]]);
        }
        Some(s)
    }

    /// Non-empty bins as `(ratio_bin_centre, count)` — for dumps and CSV.
    pub fn nonempty(&self) -> impl Iterator<Item = (f32, u16)> + '_ {
        self.hist
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(i, c)| (RATIO_MIN + i as f32 + 0.5, *c))
    }

    /// Peak-detect the histogram into per-gear ratio bands. Returns the bands
    /// plus how many are valid (`MIN_PEAKS..=NUM_GEARS`). A partial result
    /// assumes the highest-ratio peak is 1st gear — i.e. the rider covered
    /// consecutive gears from 1st; skipped gears fail the step-sanity check
    /// rather than mislabel.
    pub fn derive_bands(&self) -> Option<([f32; NUM_GEARS], usize)> {
        let h = |j: isize| -> u32 {
            if (0..BINS as isize).contains(&j) {
                self.hist[j as usize] as u32
            } else {
                0
            }
        };
        // 1-2-1 smoothed local maxima with enough raw mass around them.
        let smooth = |j: isize| h(j - 1) + 2 * h(j) + h(j + 1);
        let mut peaks: Vec<(f32, u32)> = Vec::new();
        for i in 0..BINS as isize {
            let c = smooth(i);
            if c > 0 && c >= smooth(i - 1) && c > smooth(i + 1) {
                let mass = h(i - 1) + h(i) + h(i + 1);
                if mass >= MIN_PEAK_MASS {
                    // Centroid of the 3-bin neighbourhood, in ratio units.
                    let num = h(i - 1) as f32 * (i - 1) as f32
                        + h(i) as f32 * i as f32
                        + h(i + 1) as f32 * (i + 1) as f32;
                    let ratio = RATIO_MIN + num / mass as f32 + 0.5;
                    peaks.push((ratio, mass));
                }
            }
        }
        // Strongest first; greedily accept peaks far enough from accepted ones.
        peaks.sort_by(|a, b| b.1.cmp(&a.1));
        let mut accepted: Vec<(f32, u32)> = Vec::new();
        for (r, m) in peaks {
            if accepted
                .iter()
                .all(|(ar, _)| (r - ar).abs() / ar.max(r) > PEAK_SEPARATION)
            {
                accepted.push((r, m));
            }
        }
        for (r, m) in &accepted {
            info!("LEARN: peak ratio {r:.1} (mass {m})");
        }
        if accepted.len() < MIN_PEAKS {
            info!(
                "LEARN: only {}/{} gear peaks — ride steadily in at least two gears",
                accepted.len(),
                NUM_GEARS
            );
            return None;
        }
        accepted.truncate(NUM_GEARS);
        // Highest ratio = shortest gearing = 1st.
        accepted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let count = accepted.len();
        let mut bands = [0.0f32; NUM_GEARS];
        for (i, (r, _)) in accepted.iter().enumerate() {
            bands[i] = *r;
        }
        // Sanity: adjacent gears step by a plausible factor. A step outside
        // the window means a fake peak got in, or a partial ride skipped a
        // gear — reject rather than mislabel.
        for w in bands[..count].windows(2) {
            let step = w[0] / w[1];
            if !(STEP_MIN..=STEP_MAX).contains(&step) {
                info!(
                    "LEARN: implausible gear step {:.2} between ratios {:.1} and {:.1} — rejecting calibration",
                    step, w[0], w[1]
                );
                return None;
            }
        }
        if count < NUM_GEARS {
            info!("LEARN: partial calibration {count}/{NUM_GEARS} (assumes 1st..{count}), bands {:.1?}", &bands[..count]);
        } else {
            info!("LEARN: calibration complete, bands {bands:.1?}");
        }
        Some((bands, count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic LCG, same "synthetic rider" the on-device self-test used.
    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 16) as f32 / 65536.0 // 0..1
        }
    }

    const GEAR_RATIOS: [f32; NUM_GEARS] = [240.0, 165.0, 125.0, 100.0, 82.0, 70.0];

    fn synthetic_ride(h: &mut RatioHistogram, per_gear: usize) {
        let mut rng = Lcg(0x1234_5678);
        for r in GEAR_RATIOS {
            for _ in 0..per_gear {
                let ratio = r + (rng.next() - 0.5) * 3.0;
                let speed = 20.0 + rng.next() * 40.0;
                assert_eq!(h.add_sample(ratio * speed, speed, false, false), SampleOutcome::Accepted);
            }
        }
    }

    #[test]
    fn gating_rejects_unusable_samples() {
        let mut h = RatioHistogram::new();
        use SampleOutcome::*;
        assert_eq!(h.add_sample(4000.0, 40.0, true, false), ClutchPulled);
        assert_eq!(h.add_sample(4000.0, 40.0, false, true), InNeutral);
        assert_eq!(h.add_sample(900.0, 40.0, false, false), RpmLow);
        assert_eq!(h.add_sample(1200.0, 8.0, false, false), SpeedLow);
        assert_eq!(h.add_sample(100_000.0, 15.0, false, false), OutOfRange);
        assert_eq!(h.samples(), 0);
    }

    #[test]
    fn samples_land_in_the_right_bin() {
        let mut h = RatioHistogram::new();
        // ratio exactly 100.0 → bin index 80 (100 - RATIO_MIN)
        assert_eq!(h.add_sample(4000.0, 40.0, false, false), SampleOutcome::Accepted);
        assert_eq!(h.hist()[80], 1);
        assert_eq!(h.samples(), 1);
    }

    #[test]
    fn full_ride_derives_all_six_bands_in_order() {
        let mut h = RatioHistogram::new();
        synthetic_ride(&mut h, 40);
        // Scattered noise (shift transients) must not add peaks.
        let mut rng = Lcg(0xDEAD_BEEF);
        for _ in 0..30 {
            h.add_sample((20.0 + rng.next() * 280.0) * 30.0, 30.0, false, false);
        }
        let (bands, count) = h.derive_bands().expect("six clean peaks");
        assert_eq!(count, NUM_GEARS);
        for (b, t) in bands.iter().zip(GEAR_RATIOS.iter()) {
            assert!(
                (b - t).abs() / t < 0.03,
                "band {b:.1} too far from true ratio {t:.1}"
            );
        }
        // 1st gear = highest ratio, descending to 6th.
        assert!(bands.windows(2).all(|w| w[0] > w[1]));
    }

    #[test]
    fn partial_ride_yields_partial_bands() {
        let mut h = RatioHistogram::new();
        let mut rng = Lcg(0x1234_5678);
        for r in &GEAR_RATIOS[..4] {
            for _ in 0..40 {
                let ratio = r + (rng.next() - 0.5) * 3.0;
                h.add_sample(ratio * 30.0, 30.0, false, false);
            }
        }
        // Four consecutive gears from 1st: usable partial calibration.
        let (bands, count) = h.derive_bands().expect("partial calibration");
        assert_eq!(count, 4);
        for (b, t) in bands[..4].iter().zip(&GEAR_RATIOS[..4]) {
            assert!((b - t).abs() / t < 0.03, "band {b:.1} vs {t:.1}");
        }
    }

    #[test]
    fn sparse_noise_yields_no_bands() {
        let mut h = RatioHistogram::new();
        let mut rng = Lcg(0xCAFE_F00D);
        for _ in 0..100 {
            h.add_sample((20.0 + rng.next() * 280.0) * 30.0, 30.0, false, false);
        }
        assert_eq!(h.derive_bands(), None);
    }

    #[test]
    fn close_peaks_merge_into_one_gear() {
        let mut h = RatioHistogram::new();
        // Two heavy clusters 3% apart (ratio 100 and 103) — within the 6%
        // separation rule they must count as ONE gear, so 5 real gears + the
        // twin cluster must NOT reach six bands.
        let mut rng = Lcg(0x1234_5678);
        for r in [240.0, 165.0, 125.0, 100.0, 103.0] {
            for _ in 0..40 {
                let ratio = r + (rng.next() - 0.5) * 2.0;
                h.add_sample(ratio * 30.0, 30.0, false, false);
            }
        }
        // The twin collapses to one gear: a 4-gear partial calibration.
        let (bands, count) = h.derive_bands().expect("twin should merge");
        assert_eq!(count, 4);
        assert!((99.0..=105.0).contains(&bands[3]), "merged band {:.1}", bands[3]);
    }

    #[test]
    fn fake_peak_with_implausible_step_rejects_calibration() {
        let mut h = RatioHistogram::new();
        // Six clusters, but 100→93 is a 1.075 step — below any real gearbox's
        // adjacent-gear spacing. One of them must be a fake peak (e.g. from
        // clutch-slip samples), so no calibration may be produced.
        let mut rng = Lcg(0x1234_5678);
        for r in [240.0, 165.0, 125.0, 100.0, 93.0, 70.0] {
            for _ in 0..40 {
                let ratio = r + (rng.next() - 0.5) * 2.0;
                h.add_sample(ratio * 30.0, 30.0, false, false);
            }
        }
        assert_eq!(h.derive_bands(), None);
    }

    #[test]
    fn blob_round_trip_is_lossless() {
        let mut h = RatioHistogram::new();
        synthetic_ride(&mut h, 40);
        let restored = RatioHistogram::from_blob(&h.to_blob()).unwrap();
        assert_eq!(restored.samples(), h.samples());
        assert_eq!(restored.hist(), h.hist());
        assert_eq!(restored.derive_bands(), h.derive_bands());
    }

    #[test]
    fn from_blob_rejects_wrong_size() {
        assert!(RatioHistogram::from_blob(&[0u8; 10]).is_none());
        assert!(RatioHistogram::from_blob(&[0u8; BLOB_LEN + 1]).is_none());
    }

    #[test]
    fn bin_saturates_without_overflow() {
        let mut h = RatioHistogram::new();
        h.hist[80] = u16::MAX;
        assert_eq!(h.add_sample(4000.0, 40.0, false, false), SampleOutcome::BinFull);
        assert_eq!(h.hist()[80], u16::MAX);
    }

    #[test]
    fn debug_mark_is_capped_and_never_a_peak() {
        let mut h = RatioHistogram::new();
        for _ in 0..20 {
            h.debug_mark();
        }
        assert_eq!(h.hist()[0], 5); // capped
        assert_eq!(h.derive_bands(), None); // marker alone yields no bands
    }

    #[test]
    fn nonempty_reports_bin_centres() {
        let mut h = RatioHistogram::new();
        h.add_sample(4000.0, 40.0, false, false); // ratio 100 → bin 80, centre 100.5
        let bins: Vec<_> = h.nonempty().collect();
        assert_eq!(bins, vec![(100.5, 1)]);
    }
}
