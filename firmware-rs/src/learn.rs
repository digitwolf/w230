//! Self-learning gear-ratio calibration.
//!
//! While riding (clutch out, moving, engine turning) every RPM/speed sample is
//! quantised into a 300-bin histogram of `rpm/speed` ratios — the decimation
//! that lets unlimited ride time fit in a fixed 604-byte NVS blob. Steady
//! riding piles up one sharp peak per gear; `derive_bands` extracts the six
//! peaks and turns them into the classifier's ratio bands.
//!
//! The histogram is persisted to NVS (namespace `gearlearn`) and dumped to the
//! serial log on every boot, so a post-ride USB attach doubles as a diagnostic
//! download.

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use log::{info, warn};

use crate::gear::NUM_GEARS;

pub const RATIO_MIN: f32 = 20.0;
pub const BINS: usize = 300; // 1.0-wide bins covering ratio 20..320
const MIN_RPM: f32 = 1200.0;
const MIN_SPEED: f32 = 5.0;
/// Samples in a peak's 3-bin neighbourhood before it counts as a gear.
const MIN_PEAK_MASS: u32 = 15;
/// Two peaks closer than this (relative ratio) are one gear, keep the bigger.
const PEAK_SEPARATION: f32 = 0.06;

const NVS_KEY: &str = "hist";
const BLOB_LEN: usize = 4 + BINS * 2; // [samples: u32 LE][counts: u16 LE × BINS]

pub struct RatioLearner {
    nvs: EspNvs<NvsDefault>,
    hist: [u16; BINS],
    samples: u32,
    dirty: bool,
}

impl RatioLearner {
    pub fn new(nvs: EspNvs<NvsDefault>) -> Self {
        let mut s = Self {
            nvs,
            hist: [0; BINS],
            samples: 0,
            dirty: false,
        };
        s.load();
        s
    }

    fn load(&mut self) {
        let mut buf = [0u8; BLOB_LEN];
        match self.nvs.get_blob(NVS_KEY, &mut buf) {
            Ok(Some(data)) if data.len() == BLOB_LEN => {
                self.samples = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                for (i, chunk) in data[4..].chunks_exact(2).enumerate() {
                    self.hist[i] = u16::from_le_bytes([chunk[0], chunk[1]]);
                }
                info!("LEARN: loaded histogram, {} samples", self.samples);
            }
            Ok(Some(data)) => warn!("LEARN: stored blob has wrong size {} — starting fresh", data.len()),
            Ok(None) => info!("LEARN: no stored histogram — starting fresh"),
            Err(e) => warn!("LEARN: NVS read failed: {e}"),
        }
    }

    /// Persist to NVS if anything changed since the last save.
    /// Returns true when a write actually happened (new data since last time).
    pub fn save(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        let mut buf = [0u8; BLOB_LEN];
        buf[..4].copy_from_slice(&self.samples.to_le_bytes());
        for (i, c) in self.hist.iter().enumerate() {
            buf[4 + i * 2..4 + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        match self.nvs.set_blob(NVS_KEY, &buf) {
            Ok(()) => {
                self.dirty = false;
                info!("LEARN: histogram saved ({} samples)", self.samples);
                true
            }
            Err(e) => {
                warn!("LEARN: NVS write failed: {e}");
                false
            }
        }
    }

    /// Wipe everything (long-press reset).
    pub fn clear(&mut self) {
        self.hist = [0; BINS];
        self.samples = 0;
        self.dirty = true;
        self.save();
    }

    /// Feed one poll-loop sample. Ignored unless the bike is moving under
    /// engine power with the clutch out — the only state where rpm/speed
    /// actually encodes the gear.
    pub fn add_sample(&mut self, rpm: f32, speed: f32, clutch_pulled: bool) {
        if clutch_pulled || rpm < MIN_RPM || speed < MIN_SPEED {
            return;
        }
        let bin = (rpm / speed - RATIO_MIN).floor();
        if !(0.0..BINS as f32).contains(&bin) {
            return;
        }
        let bin = bin as usize;
        if self.hist[bin] < u16::MAX {
            self.hist[bin] += 1;
            self.samples = self.samples.saturating_add(1);
            self.dirty = true;
        }
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

    pub fn hist(&self) -> &[u16; BINS] {
        &self.hist
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Serial diagnostic dump: every non-empty bin, one line each.
    pub fn dump(&self) {
        info!("LEARN: histogram dump ({} samples):", self.samples);
        for (i, c) in self.hist.iter().enumerate() {
            if *c > 0 {
                info!("LEARN:   ratio {:>5.1} x{c}", RATIO_MIN + i as f32 + 0.5);
            }
        }
    }
}

/// On-device end-to-end check of the whole learning pipeline, run at boot:
/// synthetic ride -> histogram -> peak detection -> bands -> NVS save ->
/// reload -> identical bands. Uses its own NVS namespace so it can never
/// touch real calibration data. Logs PASS/FAIL; returns success.
pub fn self_test(part: EspDefaultNvsPartition) -> bool {
    const NS: &str = "gearselftest";
    let expected = [240.0f32, 165.0, 125.0, 100.0, 82.0, 70.0];

    let nvs = match EspNvs::new(part.clone(), NS, true) {
        Ok(n) => n,
        Err(e) => {
            warn!("LEARN self-test FAIL: NVS open: {e}");
            return false;
        }
    };
    let mut l = RatioLearner::new(nvs);
    l.clear();

    // Deterministic LCG "rider": 40 jittered samples per gear + stray noise.
    let mut seed = 0x1234_5678u32;
    let mut rng = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 16) as f32 / 65536.0 // 0..1
    };
    for r in expected {
        for _ in 0..40 {
            let ratio = r + (rng() - 0.5) * 3.0;
            let speed = 20.0 + rng() * 40.0;
            l.add_sample(ratio * speed, speed, false);
        }
    }
    for _ in 0..30 {
        // shift transients: scattered single counts must NOT become peaks
        l.add_sample((20.0 + rng() * 280.0) * 30.0, 30.0, false);
    }

    let Some(bands) = l.derive_bands() else {
        warn!("LEARN self-test FAIL: no bands from synthetic ride");
        return false;
    };
    for (b, t) in bands.iter().zip(expected.iter()) {
        if (b - t).abs() / t > 0.03 {
            warn!("LEARN self-test FAIL: band {b:.1} vs expected {t:.1}");
            return false;
        }
    }

    if !l.save() {
        warn!("LEARN self-test FAIL: NVS save");
        return false;
    }
    let saved_samples = l.samples;
    drop(l);

    let nvs2 = match EspNvs::new(part, NS, true) {
        Ok(n) => n,
        Err(e) => {
            warn!("LEARN self-test FAIL: NVS reopen: {e}");
            return false;
        }
    };
    let l2 = RatioLearner::new(nvs2);
    if l2.samples != saved_samples {
        warn!(
            "LEARN self-test FAIL: reload lost samples ({} vs {})",
            l2.samples, saved_samples
        );
        return false;
    }
    match l2.derive_bands() {
        Some(b2) if b2 == bands => {
            info!("LEARN self-test PASS: bands {bands:.1?} survive save/reload");
            true
        }
        _ => {
            warn!("LEARN self-test FAIL: bands differ after reload");
            false
        }
    }
}
