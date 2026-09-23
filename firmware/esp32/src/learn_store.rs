//! NVS persistence for the ratio-learning histogram (namespace `gearlearn`).
//!
//! The histogram math lives in `w230_core::learn`; this wrapper adds flash
//! load/save with dirty tracking and the boot-time diagnostic dump.

use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use log::{info, warn};
use w230_core::ble_proto::BlackBox;
use w230_core::gear::NUM_GEARS;
use w230_core::learn::{RatioHistogram, SampleOutcome, BINS, BLOB_LEN};

const NVS_KEY: &str = "hist";
/// TEMPORARY ride-debug blob: per-boot tallies of sample gating outcomes plus
/// max rpm/speed seen, persisted with each save so a USB-less ride can be
/// debugged from the next boot's log.
const NVS_DBG_KEY: &str = "dbg";
const DBG_LEN: usize = 16 * 4;
const OUTCOME_NAMES: [&str; 7] = [
    "accepted",
    "clutch-pulled",
    "in-neutral",
    "rpm-low",
    "speed-low",
    "out-of-range",
    "bin-full",
];

#[derive(Default)]
struct RideDebug {
    outcomes: [u32; 7],
    /// Cumulative boots since last wipe — distinguishes mid-ride reboots
    /// (power bounce) from link re-inits (K-line bounce).
    boots: u32,
    link_drops: u32,
    /// Reboots whose hardware reset reason was NOT a normal power-on:
    /// brownouts, panics, watchdogs. The smoking gun for the ride resets.
    abnormal_resets: u32,
    /// Hardware reset reason of the most recent boot (esp_reset_reason_t).
    last_reset_reason: u32,
    max_rpm: f32,
    max_speed: f32,
    /// Lowest free-heap seen (bytes) — a shrinking floor across boots = leak.
    min_free_heap: u32,
    /// Undecodable reg-0x03 (interlock) values: count + last raw u16 — the
    /// register's moving-state encoding, captured for offline decoding.
    interlock_odd: u32,
    interlock_last_odd: u32,
}

/// Human name for an `esp_reset_reason_t` value.
fn reset_reason_name(r: u32) -> &'static str {
    match r {
        1 => "power-on",
        3 => "software",
        4 => "panic",
        5 | 6 | 7 => "watchdog",
        8 => "deep-sleep",
        9 => "brownout",
        10 => "sdio",
        _ => "unknown",
    }
}

impl RideDebug {
    fn record(&mut self, outcome: SampleOutcome, rpm: f32, speed: f32) {
        // Engine-off idle (0/0 readings on the bench) would swamp the ride
        // tallies — only count samples where something was actually moving.
        if rpm < 1.0 && speed < 1.0 {
            return;
        }
        self.outcomes[outcome as usize] += 1;
        self.max_rpm = self.max_rpm.max(rpm);
        self.max_speed = self.max_speed.max(speed);
    }

    fn to_blob(&self) -> [u8; DBG_LEN] {
        let mut b = [0u8; DBG_LEN];
        for (i, v) in self.outcomes.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[28..32].copy_from_slice(&self.boots.to_le_bytes());
        b[32..36].copy_from_slice(&self.link_drops.to_le_bytes());
        b[36..40].copy_from_slice(&self.abnormal_resets.to_le_bytes());
        b[40..44].copy_from_slice(&self.last_reset_reason.to_le_bytes());
        b[44..48].copy_from_slice(&self.max_rpm.to_le_bytes());
        b[48..52].copy_from_slice(&self.max_speed.to_le_bytes());
        b[52..56].copy_from_slice(&self.min_free_heap.to_le_bytes());
        b[56..60].copy_from_slice(&self.interlock_odd.to_le_bytes());
        b[60..64].copy_from_slice(&self.interlock_last_odd.to_le_bytes());
        b
    }

    fn from_blob(data: &[u8]) -> Option<Self> {
        if data.len() != DBG_LEN {
            return None;
        }
        let v = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
        let f = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let mut d = Self::default();
        for i in 0..7 {
            d.outcomes[i] = v(i);
        }
        d.boots = v(7);
        d.link_drops = v(8);
        d.abnormal_resets = v(9);
        d.last_reset_reason = v(10);
        d.max_rpm = f(44);
        d.max_speed = f(48);
        d.min_free_heap = v(13);
        d.interlock_odd = v(14);
        d.interlock_last_odd = v(15);
        Some(d)
    }

    fn log(&self) {
        info!(
            "LEARN dbg (cumulative): boots={} abnormal-resets={} (last: {}) link-drops={} min-heap={} interlock-odd={}x(last {:04X}) {} | max rpm {:.0}, max speed {:.0}",
            self.boots,
            self.abnormal_resets,
            reset_reason_name(self.last_reset_reason),
            self.link_drops,
            self.min_free_heap,
            self.interlock_odd,
            self.interlock_last_odd,
            OUTCOME_NAMES
                .iter()
                .enumerate()
                .map(|(i, n)| format!("{n}={}", self.outcomes[i]))
                .collect::<Vec<_>>()
                .join(" "),
            self.max_rpm,
            self.max_speed,
        );
    }
}

pub struct RatioLearner {
    nvs: EspNvs<NvsDefault>,
    hist: RatioHistogram,
    dirty: bool,
    debug: RideDebug,
}

impl RatioLearner {
    pub fn new(nvs: EspNvs<NvsDefault>) -> Self {
        let mut s = Self {
            nvs,
            hist: RatioHistogram::new(),
            dirty: false,
            debug: RideDebug::default(),
        };
        s.load();
        s
    }

    fn load(&mut self) {
        // Debug tallies are cumulative across boots (wiped with clear()).
        let mut dbg = [0u8; DBG_LEN];
        if let Ok(Some(data)) = self.nvs.get_blob(NVS_DBG_KEY, &mut dbg) {
            if let Some(d) = RideDebug::from_blob(data) {
                self.debug = d;
            }
        }
        self.debug.boots += 1;
        // Hardware reset reason: brownout/panic/watchdog = the ride-reset
        // smoking gun; power-on = normal key cycle.
        let reason = unsafe { esp_idf_svc::sys::esp_reset_reason() } as u32;
        self.debug.last_reset_reason = reason;
        if !matches!(reason, 1 | 3) {
            self.debug.abnormal_resets += 1;
        }
        self.debug.log();
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
        // TEMPORARY: boot marker proves the histogram write path end-to-end
        // (shows up in the dump as ratio 20.5, growing by one per boot).
        if self.hist.debug_mark() {
            self.dirty = true;
        }
    }

    /// Persist to NVS if anything changed since the last save.
    /// Returns true when a write actually happened (new data since last time).
    pub fn save(&mut self) -> bool {
        // Track the free-heap floor (leak detector for the ride-reset hunt).
        let free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
        if self.debug.min_free_heap == 0 || free < self.debug.min_free_heap {
            self.debug.min_free_heap = free;
        }
        // Debug tallies ride along on every save (temporary diagnostics).
        let _ = self.nvs.set_blob(NVS_DBG_KEY, &self.debug.to_blob());
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

    /// The black box as the BLE app sees it.
    pub fn black_box(&self) -> BlackBox {
        BlackBox {
            boots: self.debug.boots,
            abnormal_resets: self.debug.abnormal_resets,
            last_reset_reason: reset_reason_name(self.debug.last_reset_reason).to_string(),
            link_drops: self.debug.link_drops,
            min_free_heap: self.debug.min_free_heap,
            interlock_odd: self.debug.interlock_odd,
            interlock_last_odd: self.debug.interlock_last_odd as u16,
            max_rpm: self.debug.max_rpm,
            max_speed: self.debug.max_speed,
            outcomes: self.debug.outcomes,
        }
    }

    /// Human name of this boot's hardware reset reason.
    pub fn reset_reason(&self) -> &'static str {
        reset_reason_name(self.debug.last_reset_reason)
    }

    pub fn min_free_heap(&self) -> u32 {
        self.debug.min_free_heap
    }

    /// Reset only the black-box tallies, keeping the learned histogram.
    pub fn clear_black_box(&mut self) {
        self.debug = RideDebug {
            boots: 1,
            ..RideDebug::default()
        };
        let _ = self.nvs.set_blob(NVS_DBG_KEY, &self.debug.to_blob());
    }

    /// Accepted histogram peaks (ratio, mass), strongest first.
    pub fn peaks(&self) -> Vec<(f32, u32)> {
        self.hist.peaks()
    }

    /// Wipe everything (long-press / app reset), debug tallies included.
    pub fn clear(&mut self) {
        self.hist.clear();
        self.debug = RideDebug::default();
        self.dirty = true;
        self.save();
    }

    /// Count a K-line link drop in the persisted debug tallies.
    pub fn note_link_drop(&mut self) {
        self.debug.link_drops += 1;
    }

    /// Black-box an undecodable interlock (reg 0x03) value.
    pub fn note_interlock_odd(&mut self, raw: u16) {
        self.debug.interlock_odd += 1;
        self.debug.interlock_last_odd = raw as u32;
    }

    pub fn add_sample(&mut self, rpm: f32, speed: f32, clutch_pulled: bool, in_neutral: bool) {
        let outcome = self.hist.add_sample(rpm, speed, clutch_pulled, in_neutral);
        self.debug.record(outcome, rpm, speed);
        if outcome == SampleOutcome::Accepted {
            self.dirty = true;
        }
    }

    pub fn derive_bands(&self, reference: &[f32; NUM_GEARS]) -> Option<([f32; NUM_GEARS], usize)> {
        self.hist.derive_bands(reference)
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
