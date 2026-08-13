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
/// A learned peak must sit within this of a factory band to refine that gear;
/// bands are ~20% apart, so ±10% cannot straddle two gears.
const ANCHOR_TOL: f32 = 0.10;
/// Fewest anchored peaks that make a usable refinement.
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

    /// Peak-detect the histogram and refine `reference` (factory) bands with
    /// the learned peaks: each peak within [`ANCHOR_TOL`] of a factory band
    /// replaces that gear's value. Returns the full band set plus how many
    /// gears were refined; `None` until at least [`MIN_PEAKS`] gears anchor.
    pub fn derive_bands(&self, reference: &[f32; NUM_GEARS]) -> Option<([f32; NUM_GEARS], usize)> {
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
        // Anchor each peak to its nearest reference (factory) band: a peak
        // near a band refines that gear; peaks matching nothing are noise.
        // No ordering assumption — riding only 3rd and 4th refines 3rd and
        // 4th, never mislabels them as 1st and 2nd.
        let mut bands = *reference;
        let mut matched_mass = [0u32; NUM_GEARS];
        let mut learned = 0usize;
        for (r, m) in accepted {
            let (slot, err) = reference
                .iter()
                .enumerate()
                .map(|(i, c)| (i, (r - c).abs() / c))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap();
            if err > ANCHOR_TOL {
                info!("LEARN: peak ratio {r:.1} matches no gear (nearest err {:.0}%) — ignored", err * 100.0);
                continue;
            }
            if m > matched_mass[slot] {
                if matched_mass[slot] == 0 {
                    learned += 1;
                }
                matched_mass[slot] = m;
                bands[slot] = r;
                info!("LEARN: gear {} refined to ratio {r:.1} (mass {m})", slot + 1);
            }
        }
        if learned < MIN_PEAKS {
            info!("LEARN: only {learned} anchored gear peaks — keeping factory bands until at least {MIN_PEAKS}");
            return None;
        }
        info!("LEARN: {learned}/{NUM_GEARS} gears refined, bands {bands:.1?}");
        Some((bands, learned))
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
    /// A slightly-off factory reference: learned peaks must still anchor.
    const REFERENCE: [f32; NUM_GEARS] = [232.0, 160.0, 121.0, 97.0, 80.0, 68.0];

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
        let (bands, count) = h.derive_bands(&REFERENCE).expect("six clean peaks");
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
    fn cruising_gears_refine_their_own_slots_not_first() {
        // The field bug: riding only 3rd and 4th must refine gears 3 and 4 —
        // never be mislabelled as 1st and 2nd.
        let mut h = RatioHistogram::new();
        let mut rng = Lcg(0x1234_5678);
        for r in [GEAR_RATIOS[2], GEAR_RATIOS[3]] {
            for _ in 0..40 {
                let ratio = r + (rng.next() - 0.5) * 3.0;
                h.add_sample(ratio * 30.0, 30.0, false, false);
            }
        }
        let (bands, count) = h.derive_bands(&REFERENCE).expect("two anchored peaks");
        assert_eq!(count, 2);
        assert!((bands[2] - GEAR_RATIOS[2]).abs() / GEAR_RATIOS[2] < 0.03);
        assert!((bands[3] - GEAR_RATIOS[3]).abs() / GEAR_RATIOS[3] < 0.03);
        // Unridden gears keep the factory values.
        assert_eq!(bands[0], REFERENCE[0]);
        assert_eq!(bands[1], REFERENCE[1]);
        assert_eq!(bands[5], REFERENCE[5]);
    }

    #[test]
    fn sparse_noise_yields_no_bands() {
        let mut h = RatioHistogram::new();
        let mut rng = Lcg(0xCAFE_F00D);
        for _ in 0..100 {
            h.add_sample((20.0 + rng.next() * 280.0) * 30.0, 30.0, false, false);
        }
        assert_eq!(h.derive_bands(&REFERENCE), None);
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
        // The twin collapses to one gear: four gears refined.
        let (bands, count) = h.derive_bands(&REFERENCE).expect("twin should merge");
        assert_eq!(count, 4);
        assert!((99.0..=105.0).contains(&bands[3]), "merged band {:.1}", bands[3]);
    }

    #[test]
    fn weak_fake_peak_cannot_displace_a_true_gear() {
        let mut h = RatioHistogram::new();
        // A weak spurious cluster near 4th (ratio 93, e.g. clutch slip) must
        // lose to the stronger true 4th-gear peak at 100.
        let mut rng = Lcg(0x1234_5678);
        for _ in 0..40 {
            h.add_sample((100.0 + (rng.next() - 0.5) * 2.0) * 30.0, 30.0, false, false);
        }
        for _ in 0..16 {
            h.add_sample((93.0 + (rng.next() - 0.5) * 2.0) * 30.0, 30.0, false, false);
        }
        for r in [240.0, 165.0, 125.0] {
            for _ in 0..40 {
                h.add_sample((r + (rng.next() - 0.5) * 2.0) * 30.0, 30.0, false, false);
            }
        }
        let (bands, _) = h.derive_bands(&REFERENCE).expect("anchored");
        assert!((bands[3] - 100.0).abs() < 3.0, "4th = {:.1}", bands[3]);
    }

    #[test]
    fn blob_round_trip_is_lossless() {
        let mut h = RatioHistogram::new();
        synthetic_ride(&mut h, 40);
        let restored = RatioHistogram::from_blob(&h.to_blob()).unwrap();
        assert_eq!(restored.samples(), h.samples());
        assert_eq!(restored.hist(), h.hist());
        assert_eq!(restored.derive_bands(&REFERENCE), h.derive_bands(&REFERENCE));
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
        assert_eq!(h.derive_bands(&REFERENCE), None); // marker alone yields nothing
    }

    #[test]
    fn nonempty_reports_bin_centres() {
        let mut h = RatioHistogram::new();
        h.add_sample(4000.0, 40.0, false, false); // ratio 100 → bin 80, centre 100.5
        let bins: Vec<_> = h.nonempty().collect();
        assert_eq!(bins, vec![(100.5, 1)]);
    }
}
