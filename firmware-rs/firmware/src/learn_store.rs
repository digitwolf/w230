//! NVS persistence for the ratio-learning histogram (namespace `gearlearn`).
//!
//! The histogram math lives in `w230_core::learn`; this wrapper adds flash
//! load/save with dirty tracking and the boot-time diagnostic dump.

use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use log::{info, warn};
use w230_core::gear::NUM_GEARS;
use w230_core::learn::{RatioHistogram, BINS, BLOB_LEN};

const NVS_KEY: &str = "hist";

pub struct RatioLearner {
    nvs: EspNvs<NvsDefault>,
    hist: RatioHistogram,
    dirty: bool,
}

impl RatioLearner {
    pub fn new(nvs: EspNvs<NvsDefault>) -> Self {
        let mut s = Self {
            nvs,
            hist: RatioHistogram::new(),
            dirty: false,
        };
        s.load();
        s
    }

    fn load(&mut self) {
        let mut buf = [0u8; BLOB_LEN];
        match self.nvs.get_blob(NVS_KEY, &mut buf) {
            Ok(Some(data)) => match RatioHistogram::from_blob(data) {
                Some(h) => {
                    info!("LEARN: loaded histogram, {} samples", h.samples());
                    self.hist = h;
                }
                None => warn!(
                    "LEARN: stored blob has wrong size {} — starting fresh",
                    data.len()
                ),
            },
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
        match self.nvs.set_blob(NVS_KEY, &self.hist.to_blob()) {
            Ok(()) => {
                self.dirty = false;
                info!("LEARN: histogram saved ({} samples)", self.hist.samples());
                true
            }
            Err(e) => {
                warn!("LEARN: NVS write failed: {e}");
                false
            }
        }
    }

    /// Wipe everything (long-press / web reset).
    pub fn clear(&mut self) {
        self.hist.clear();
        self.dirty = true;
        self.save();
    }

    pub fn add_sample(&mut self, rpm: f32, speed: f32, clutch_pulled: bool) {
        if self.hist.add_sample(rpm, speed, clutch_pulled) {
            self.dirty = true;
        }
    }

    pub fn derive_bands(&self) -> Option<[f32; NUM_GEARS]> {
        self.hist.derive_bands()
    }

    pub fn hist(&self) -> &[u16; BINS] {
        self.hist.hist()
    }

    pub fn samples(&self) -> u32 {
        self.hist.samples()
    }

    /// Serial diagnostic dump: every non-empty bin, one line each.
    pub fn dump(&self) {
        info!("LEARN: histogram dump ({} samples):", self.hist.samples());
        for (ratio, count) in self.hist.nonempty() {
            info!("LEARN:   ratio {ratio:>5.1} x{count}");
        }
    }
}
