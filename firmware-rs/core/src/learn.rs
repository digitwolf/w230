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
const MIN_SPEED: f32 = 5.0;
/// Samples in a peak's 3-bin neighbourhood before it counts as a gear.
const MIN_PEAK_MASS: u32 = 15;
/// Two peaks closer than this (relative ratio) are one gear, keep the bigger.
const PEAK_SEPARATION: f32 = 0.06;

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

    /// Feed one poll-loop sample. Returns true when the sample was counted;
    /// ignored (false) unless the bike is moving under engine power with the
    /// clutch out — the only state where rpm/speed actually encodes the gear.
    pub fn add_sample(&mut self, rpm: f32, speed: f32, clutch_pulled: bool) -> bool {
        if clutch_pulled || rpm < MIN_RPM || speed < MIN_SPEED {
            return false;
        }
        let bin = (rpm / speed - RATIO_MIN).floor();
        if !(0.0..BINS as f32).contains(&bin) {
            return false;
        }
        let bin = bin as usize;
        if self.hist[bin] < u16::MAX {
            self.hist[bin] += 1;
            self.samples = self.samples.saturating_add(1);
            true
        } else {
            false
        }
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

    /// Peak-detect the histogram into per-gear ratio bands. Returns bands only
    /// when all `NUM_GEARS` gears are confidently present — a partial ride
    /// must not produce a half-calibrated classifier.
    pub fn derive_bands(&self) -> Option<[f32; NUM_GEARS]> {
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
        if accepted.len() < NUM_GEARS {
            info!(
                "LEARN: {}/{} gear peaks found — ride all gears steadily to finish calibration",
                accepted.len(),
                NUM_GEARS
            );
            return None;
        }
        accepted.truncate(NUM_GEARS);
        // Highest ratio = shortest gearing = 1st.
        accepted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let mut bands = [0.0f32; NUM_GEARS];
        for (i, (r, _)) in accepted.iter().enumerate() {
            bands[i] = *r;
        }
        info!("LEARN: calibration complete, bands {bands:.1?}");
        Some(bands)
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
                assert!(h.add_sample(ratio * speed, speed, false));
            }
        }
    }

    #[test]
    fn gating_rejects_unusable_samples() {
        let mut h = RatioHistogram::new();
        assert!(!h.add_sample(4000.0, 40.0, true)); // clutch pulled
        assert!(!h.add_sample(900.0, 40.0, false)); // engine barely turning
        assert!(!h.add_sample(4000.0, 2.0, false)); // walking pace... ratio 2000 also out of range
        assert!(!h.add_sample(100_000.0, 5.0, false)); // ratio above histogram range
        assert_eq!(h.samples(), 0);
    }

    #[test]
    fn samples_land_in_the_right_bin() {
        let mut h = RatioHistogram::new();
        // ratio exactly 100.0 → bin index 80 (100 - RATIO_MIN)
        assert!(h.add_sample(4000.0, 40.0, false));
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
            h.add_sample((20.0 + rng.next() * 280.0) * 30.0, 30.0, false);
        }
        let bands = h.derive_bands().expect("six clean peaks");
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
    fn partial_ride_yields_no_bands() {
        let mut h = RatioHistogram::new();
        let mut rng = Lcg(0x1234_5678);
        for r in &GEAR_RATIOS[..4] {
            for _ in 0..40 {
                let ratio = r + (rng.next() - 0.5) * 3.0;
                h.add_sample(ratio * 30.0, 30.0, false);
            }
        }
        assert_eq!(h.derive_bands(), None); // 4/6 gears is not a calibration
    }

    #[test]
    fn sparse_noise_yields_no_bands() {
        let mut h = RatioHistogram::new();
        let mut rng = Lcg(0xCAFE_F00D);
        for _ in 0..100 {
            h.add_sample((20.0 + rng.next() * 280.0) * 30.0, 30.0, false);
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
                h.add_sample(ratio * 30.0, 30.0, false);
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
        assert!(!h.add_sample(4000.0, 40.0, false)); // bin full → dropped
        assert_eq!(h.hist()[80], u16::MAX);
    }

    #[test]
    fn nonempty_reports_bin_centres() {
        let mut h = RatioHistogram::new();
        h.add_sample(4000.0, 40.0, false); // ratio 100 → bin 80, centre 100.5
        let bins: Vec<_> = h.nonempty().collect();
        assert_eq!(bins, vec![(100.5, 1)]);
    }
}
