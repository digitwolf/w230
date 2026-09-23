//! Wire formats of the BLE "W230 Gear" GATT service — the contract between
//! the firmware and the iOS app. Everything here is pure encode/decode so the
//! exact bytes are pinned by host tests; the firmware crate owns the GATT
//! plumbing and the app mirrors these layouts in Swift.
//!
//! Versioning: [`PROTOCOL_VERSION`] is the first byte of the live packet and
//! the `proto` field of the device-info JSON. The app refuses protocol
//! versions it does not know; additive JSON fields never bump it, changed
//! byte layouts or opcode meanings do. See `docs/ble-protocol.md`.

use crate::gear::{Gear, FACTORY_BANDS, NUM_GEARS};
use crate::learn::{BINS, RATIO_MIN};

/// Bumped whenever a byte layout or opcode changes meaning.
pub const PROTOCOL_VERSION: u8 = 1;

/// 128-bit UUID base; the 16-bit slot (bytes 2..4) selects the attribute.
/// Expressed as the canonical string's u128 so both ends compare literally:
/// `8f6c0000-b5a3-4b2e-9d61-3b1c7a2e5f10` ↔ `0x8f6c0000_b5a3_4b2e_9d61_3b1c7a2e5f10`.
pub const UUID_BASE: u128 = 0x8f6c0000_b5a3_4b2e_9d61_3b1c7a2e5f10;

/// Attribute UUID from its 16-bit slot.
pub const fn uuid(slot: u16) -> u128 {
    UUID_BASE | ((slot as u128) << 96)
}

/// Advertised device name (also the first scan-filter the app applies).
pub const DEVICE_NAME: &str = "W230-GEAR";

pub const SLOT_SERVICE: u16 = 0x0001;
/// 20-byte binary telemetry, notified every poll cycle. Layout: [`LiveStatus`].
pub const SLOT_LIVE: u16 = 0x0002;
/// JSON: firmware identity, partition/rollback state, health.
pub const SLOT_DEVICE_INFO: u16 = 0x0003;
/// JSON: the persisted ride black box (reset reasons, gate tallies, heap floor).
pub const SLOT_BLACK_BOX: u16 = 0x0004;
/// JSON: factory bands, learned bands, detected peaks.
pub const SLOT_CALIBRATION: u16 = 0x0005;
/// Binary: histogram bins 0..150 (see [`hist_page`]).
pub const SLOT_HIST_0: u16 = 0x0006;
/// Binary: histogram bins 150..300.
pub const SLOT_HIST_1: u16 = 0x0007;
/// JSON: ring of recent firmware events (link drops, saves, OTA steps).
pub const SLOT_EVENTS: u16 = 0x0008;
/// Write (encrypted): [`Command`] opcodes.
pub const SLOT_COMMAND: u16 = 0x0009;
/// Write (encrypted): WiFi credentials, see [`WifiCredentials::decode`].
pub const SLOT_WIFI_CONFIG: u16 = 0x000A;
/// JSON + notify: WiFi station state.
pub const SLOT_WIFI_STATUS: u16 = 0x000B;
/// JSON + notify: OTA state machine.
pub const SLOT_OTA_STATUS: u16 = 0x000C;
/// JSON: user settings (brightness, boot-check policy, manifest URL).
pub const SLOT_SETTINGS: u16 = 0x000D;

/// Bytes per histogram page (two pages cover all [`BINS`] bins).
pub const HIST_PAGE_BINS: usize = BINS / 2;
/// GATT attribute ceiling; every JSON body is trimmed to fit.
pub const MAX_ATTR_LEN: usize = 512;

/// Gear byte in the live packet.
pub const GEAR_UNKNOWN: u8 = 0;
pub const GEAR_NEUTRAL: u8 = 7;

pub const FLAG_LINK_UP: u8 = 1 << 0;
pub const FLAG_NEUTRAL_SWITCH: u8 = 1 << 1;
pub const FLAG_INTERLOCK_KNOWN: u8 = 1 << 2;
pub const FLAG_INTERLOCK_CLOSED: u8 = 1 << 3;
pub const FLAG_LEARN_FLASH: u8 = 1 << 4;
pub const FLAG_OTA_BUSY: u8 = 1 << 5;
pub const FLAG_DEMO: u8 = 1 << 6;

/// One poll cycle, as notified to the app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveStatus {
    pub link_up: bool,
    pub neutral_switch: bool,
    /// ECU interlock register 0x03: `None` = not read / refused.
    pub interlock: Option<bool>,
    pub learn_flash: bool,
    pub ota_busy: bool,
    pub demo: bool,
    pub gear: Gear,
    pub brightness_idx: u8,
    pub rpm: Option<f32>,
    pub speed: Option<f32>,
    pub samples: u32,
    pub uptime_s: u32,
}

impl LiveStatus {
    pub const LEN: usize = 20;

    /// Little-endian, fixed 20 bytes so it fits a 23-byte ATT MTU untouched:
    /// `[proto][flags][gear][bright][rpm u16][speed u16][ratio f32][samples u32][uptime u32]`.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[0] = PROTOCOL_VERSION;
        let mut flags = 0u8;
        if self.link_up {
            flags |= FLAG_LINK_UP;
        }
        if self.neutral_switch {
            flags |= FLAG_NEUTRAL_SWITCH;
        }
        if let Some(closed) = self.interlock {
            flags |= FLAG_INTERLOCK_KNOWN;
            if closed {
                flags |= FLAG_INTERLOCK_CLOSED;
            }
        }
        if self.learn_flash {
            flags |= FLAG_LEARN_FLASH;
        }
        if self.ota_busy {
            flags |= FLAG_OTA_BUSY;
        }
        if self.demo {
            flags |= FLAG_DEMO;
        }
        b[1] = flags;
        b[2] = match self.gear {
            Gear::Unknown => GEAR_UNKNOWN,
            Gear::Neutral => GEAR_NEUTRAL,
            Gear::G(n) => n.min(6),
        };
        b[3] = self.brightness_idx;
        let rpm = self.rpm.unwrap_or(0.0).clamp(0.0, 65535.0).round() as u16;
        b[4..6].copy_from_slice(&rpm.to_le_bytes());
        let speed = self.speed.unwrap_or(0.0).clamp(0.0, 65535.0).round() as u16;
        b[6..8].copy_from_slice(&speed.to_le_bytes());
        let ratio = match (self.rpm, self.speed) {
            (Some(r), Some(s)) if s >= 1.0 => r / s,
            _ => 0.0,
        };
        b[8..12].copy_from_slice(&ratio.to_le_bytes());
        b[12..16].copy_from_slice(&self.samples.to_le_bytes());
        b[16..20].copy_from_slice(&self.uptime_s.to_le_bytes());
        b
    }
}

/// Opcodes the app writes to [`SLOT_COMMAND`]: one byte plus payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    WipeCalibration,
    /// Index into the firmware's brightness table.
    SetBrightness(u8),
    CheckUpdate,
    InstallUpdate,
    /// Boot-time update policy: 0 = off, 1 = check only, 2 = check + install.
    SetBootPolicy(u8),
    Reboot,
    ForgetWifi,
    ClearBlackBox,
    /// Override the manifest URL (empty string = back to the built-in default).
    SetManifestUrl(String),
    /// Connect to the stored WiFi and report, without touching OTA.
    TestWifi,
}

pub const OP_WIPE_CALIBRATION: u8 = 0x01;
pub const OP_SET_BRIGHTNESS: u8 = 0x02;
pub const OP_CHECK_UPDATE: u8 = 0x03;
pub const OP_INSTALL_UPDATE: u8 = 0x04;
pub const OP_SET_BOOT_POLICY: u8 = 0x05;
pub const OP_REBOOT: u8 = 0x06;
pub const OP_FORGET_WIFI: u8 = 0x07;
pub const OP_CLEAR_BLACK_BOX: u8 = 0x08;
pub const OP_SET_MANIFEST_URL: u8 = 0x09;
pub const OP_TEST_WIFI: u8 = 0x0A;

impl Command {
    /// Decode a command write; `None` for unknown opcodes or bad payloads
    /// (the firmware ignores those rather than guessing).
    pub fn decode(bytes: &[u8]) -> Option<Command> {
        let (&op, rest) = bytes.split_first()?;
        Some(match op {
            OP_WIPE_CALIBRATION => Command::WipeCalibration,
            OP_SET_BRIGHTNESS => Command::SetBrightness(*rest.first()?),
            OP_CHECK_UPDATE => Command::CheckUpdate,
            OP_INSTALL_UPDATE => Command::InstallUpdate,
            OP_SET_BOOT_POLICY => {
                let p = *rest.first()?;
                if p > 2 {
                    return None;
                }
                Command::SetBootPolicy(p)
            }
            OP_REBOOT => Command::Reboot,
            OP_FORGET_WIFI => Command::ForgetWifi,
            OP_CLEAR_BLACK_BOX => Command::ClearBlackBox,
            OP_SET_MANIFEST_URL => {
                let url = core::str::from_utf8(rest).ok()?.trim().to_string();
                if !url.is_empty() && !url.starts_with("https://") {
                    return None; // OTA is HTTPS-only; refuse to store anything else
                }
                Command::SetManifestUrl(url)
            }
            OP_TEST_WIFI => Command::TestWifi,
            _ => return None,
        })
    }
}

/// Payload of [`SLOT_WIFI_CONFIG`]: `[0x01][ssid_len][ssid][psk_len][psk]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiCredentials {
    pub ssid: String,
    pub psk: String,
}

pub const WIFI_CONFIG_VERSION: u8 = 0x01;

impl WifiCredentials {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&ver, rest) = bytes.split_first()?;
        if ver != WIFI_CONFIG_VERSION {
            return None;
        }
        let (&sl, rest) = rest.split_first()?;
        let sl = sl as usize;
        if sl == 0 || sl > 32 || rest.len() < sl + 1 {
            return None;
        }
        let ssid = core::str::from_utf8(&rest[..sl]).ok()?.to_string();
        let rest = &rest[sl..];
        let (&pl, rest) = rest.split_first()?;
        let pl = pl as usize;
        if pl > 63 || rest.len() < pl {
            return None;
        }
        let psk = core::str::from_utf8(&rest[..pl]).ok()?.to_string();
        Some(Self { ssid, psk })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![WIFI_CONFIG_VERSION, self.ssid.len() as u8];
        v.extend_from_slice(self.ssid.as_bytes());
        v.push(self.psk.len() as u8);
        v.extend_from_slice(self.psk.as_bytes());
        v
    }
}

/// One histogram page: `[page u8][samples u32 LE][count u16 LE × 150]`.
pub fn hist_page(hist: &[u16], samples: u32, page: u8) -> Vec<u8> {
    let start = page as usize * HIST_PAGE_BINS;
    let mut out = Vec::with_capacity(5 + HIST_PAGE_BINS * 2);
    out.push(page);
    out.extend_from_slice(&samples.to_le_bytes());
    for i in start..start + HIST_PAGE_BINS {
        let c = hist.get(i).copied().unwrap_or(0);
        out.extend_from_slice(&c.to_le_bytes());
    }
    out
}

/// Ratio (bin centre) for a histogram bin index — same convention as the
/// serial dump and CSV, so app charts line up with the boot log.
pub fn bin_ratio(bin: usize) -> f32 {
    RATIO_MIN + bin as f32 + 0.5
}

// ---------------------------------------------------------------------------
// JSON bodies. Hand-built like `diag.rs` (no serde on the hot path); every
// builder trims to MAX_ATTR_LEN so a long-read never truncates mid-token.

/// Truncate to at most `max` bytes without splitting a UTF-8 character
/// (`String::truncate` panics on a char boundary miss; event text and
/// release notes can contain dashes, arrows and accents).
pub fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_opt_str(s: Option<&str>) -> String {
    s.map_or("null".to_string(), json_str)
}

fn json_f32_list(v: &[f32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|x| format!("{x:.1}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Firmware identity + platform health for the `Device` screen.
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo<'a> {
    pub fw_version: &'a str,
    pub project: &'a str,
    pub idf_version: &'a str,
    pub built: &'a str,
    pub hardware: &'a str,
    /// Running app partition label (`ota_0`, `ota_1`, or `factory`).
    pub slot: &'a str,
    /// False on the legacy single-app partition table (needs a USB reflash).
    pub ota_capable: bool,
    /// True until this boot's self-test marks the image valid (rollback armed).
    pub pending_verify: bool,
    pub boot_policy: u8,
    pub uptime_s: u32,
    pub free_heap: u32,
    pub min_free_heap: u32,
    pub reset_reason: &'a str,
    pub mac: &'a str,
    pub ble_connections: u8,
}

pub fn device_info_json(d: &DeviceInfo) -> String {
    format!(
        "{{\"proto\":{},\"fw\":{},\"project\":{},\"idf\":{},\"built\":{},\"hw\":{},\
         \"slot\":{},\"otaCapable\":{},\"pendingVerify\":{},\"bootPolicy\":{},\
         \"uptime\":{},\"freeHeap\":{},\"minFreeHeap\":{},\"resetReason\":{},\"mac\":{},\"bleConns\":{}}}",
        PROTOCOL_VERSION,
        json_str(d.fw_version),
        json_str(d.project),
        json_str(d.idf_version),
        json_str(d.built),
        json_str(d.hardware),
        json_str(d.slot),
        d.ota_capable,
        d.pending_verify,
        d.boot_policy,
        d.uptime_s,
        d.free_heap,
        d.min_free_heap,
        json_str(d.reset_reason),
        json_str(d.mac),
        d.ble_connections,
    )
}

/// The persisted ride black box, as the app sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlackBox {
    pub boots: u32,
    pub abnormal_resets: u32,
    pub last_reset_reason: String,
    pub link_drops: u32,
    pub min_free_heap: u32,
    pub interlock_odd: u32,
    pub interlock_last_odd: u16,
    pub max_rpm: f32,
    pub max_speed: f32,
    /// Sample-gate tallies in `SampleOutcome` order: accepted, clutch,
    /// neutral, rpm-low, speed-low, out-of-range, bin-full.
    pub outcomes: [u32; 7],
}

pub fn black_box_json(b: &BlackBox) -> String {
    format!(
        "{{\"boots\":{},\"abnormalResets\":{},\"lastReset\":{},\"linkDrops\":{},\"minFreeHeap\":{},\
         \"interlockOdd\":{},\"interlockLastOdd\":{},\"maxRpm\":{:.0},\"maxSpeed\":{:.0},\
         \"gates\":{{\"accepted\":{},\"clutch\":{},\"neutral\":{},\"rpmLow\":{},\"speedLow\":{},\"outOfRange\":{},\"binFull\":{}}}}}",
        b.boots,
        b.abnormal_resets,
        json_str(&b.last_reset_reason),
        b.link_drops,
        b.min_free_heap,
        b.interlock_odd,
        b.interlock_last_odd,
        b.max_rpm,
        b.max_speed,
        b.outcomes[0],
        b.outcomes[1],
        b.outcomes[2],
        b.outcomes[3],
        b.outcomes[4],
        b.outcomes[5],
        b.outcomes[6],
    )
}

/// Calibration state: what the classifier is using and the evidence for it.
pub fn calibration_json(
    bands: Option<&[f32]>,
    learned: usize,
    samples: u32,
    peaks: &[(f32, u32)],
) -> String {
    let peaks_json = peaks
        .iter()
        .map(|(r, m)| format!("[{r:.1},{m}]"))
        .collect::<Vec<_>>()
        .join(",");
    let body = format!(
        "{{\"factory\":{},\"bands\":{},\"learned\":{},\"samples\":{},\"peaks\":[{}]}}",
        json_f32_list(&FACTORY_BANDS),
        bands.map_or("null".to_string(), json_f32_list),
        learned.min(NUM_GEARS),
        samples,
        peaks_json,
    );
    // Peaks are strongest-first, so dropping from the tail loses the least.
    if body.len() <= MAX_ATTR_LEN || peaks.is_empty() {
        body
    } else {
        calibration_json(bands, learned, samples, &peaks[..peaks.len() - 1])
    }
}

/// OTA state machine, mirrored by the app's update screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OtaState {
    #[default]
    Idle,
    Connecting,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Verifying,
    /// New image activated; a reboot follows within seconds.
    Rebooting,
    Failed,
}

impl OtaState {
    pub fn as_str(self) -> &'static str {
        match self {
            OtaState::Idle => "idle",
            OtaState::Connecting => "connecting",
            OtaState::Checking => "checking",
            OtaState::UpToDate => "upToDate",
            OtaState::Available => "available",
            OtaState::Downloading => "downloading",
            OtaState::Verifying => "verifying",
            OtaState::Rebooting => "rebooting",
            OtaState::Failed => "failed",
        }
    }

    pub fn is_busy(self) -> bool {
        matches!(
            self,
            OtaState::Connecting
                | OtaState::Checking
                | OtaState::Downloading
                | OtaState::Verifying
                | OtaState::Rebooting
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OtaStatus {
    pub state: OtaState,
    /// 0..=100 while downloading.
    pub progress: u8,
    pub current: String,
    pub available: Option<String>,
    pub notes: Option<String>,
    pub size: Option<u32>,
    pub error: Option<String>,
    /// Uptime seconds of the last completed check, `None` if never.
    pub last_check_uptime: Option<u32>,
    pub last_check_ok: bool,
}

pub fn ota_status_json(s: &OtaStatus) -> String {
    let mut body = format!(
        "{{\"state\":{},\"progress\":{},\"current\":{},\"available\":{},\"notes\":{},\"size\":{},\
         \"error\":{},\"lastCheck\":{},\"lastCheckOk\":{}}}",
        json_str(s.state.as_str()),
        s.progress,
        json_str(&s.current),
        json_opt_str(s.available.as_deref()),
        json_opt_str(s.notes.as_deref()),
        s.size.map_or("null".to_string(), |v| v.to_string()),
        json_opt_str(s.error.as_deref()),
        s.last_check_uptime
            .map_or("null".to_string(), |v| v.to_string()),
        s.last_check_ok,
    );
    if body.len() > MAX_ATTR_LEN {
        // Notes are the only unbounded field.
        let trimmed = OtaStatus {
            notes: s.notes.as_ref().map(|n| {
                let mut n = n.clone();
                let keep = n.len().saturating_sub(64);
                truncate_utf8(&mut n, keep);
                n
            }),
            ..s.clone()
        };
        body = ota_status_json(&trimmed);
    }
    body
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WifiState {
    #[default]
    Off,
    Connecting,
    Connected,
    Failed,
}

impl WifiState {
    pub fn as_str(self) -> &'static str {
        match self {
            WifiState::Off => "off",
            WifiState::Connecting => "connecting",
            WifiState::Connected => "connected",
            WifiState::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiStatus {
    pub configured: bool,
    pub ssid: Option<String>,
    pub state: WifiState,
    pub ip: Option<String>,
    pub rssi: Option<i32>,
    pub error: Option<String>,
}

pub fn wifi_status_json(w: &WifiStatus) -> String {
    format!(
        "{{\"configured\":{},\"ssid\":{},\"state\":{},\"ip\":{},\"rssi\":{},\"error\":{}}}",
        w.configured,
        json_opt_str(w.ssid.as_deref()),
        json_str(w.state.as_str()),
        json_opt_str(w.ip.as_deref()),
        w.rssi.map_or("null".to_string(), |v| v.to_string()),
        json_opt_str(w.error.as_deref()),
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings<'a> {
    pub brightness_idx: u8,
    pub brightness_steps: &'a [u8],
    pub boot_policy: u8,
    pub manifest_url: &'a str,
    pub manifest_url_is_default: bool,
    pub wifi_ssid: Option<&'a str>,
}

pub fn settings_json(s: &Settings) -> String {
    format!(
        "{{\"brightness\":{},\"brightnessSteps\":[{}],\"bootPolicy\":{},\"manifestUrl\":{},\"manifestDefault\":{},\"wifiSsid\":{}}}",
        s.brightness_idx,
        s.brightness_steps
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(","),
        s.boot_policy,
        json_str(s.manifest_url),
        s.manifest_url_is_default,
        json_opt_str(s.wifi_ssid),
    )
}

/// Fixed-capacity ring of short firmware events with uptime stamps, rendered
/// as a JSON array that always fits one attribute (oldest entries drop first).
#[derive(Clone, Debug, Default)]
pub struct EventRing {
    events: std::collections::VecDeque<(u32, String)>,
}

impl EventRing {
    pub const CAPACITY: usize = 24;

    pub fn push(&mut self, uptime_s: u32, text: impl Into<String>) {
        if self.events.len() == Self::CAPACITY {
            self.events.pop_front();
        }
        let mut text = text.into();
        truncate_utf8(&mut text, 48);
        self.events.push_back((uptime_s, text));
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// `[{"t":uptime,"e":"text"},...]`, newest last.
    pub fn to_json(&self) -> String {
        let mut skip = 0;
        loop {
            let body = format!(
                "[{}]",
                self.events
                    .iter()
                    .skip(skip)
                    .map(|(t, e)| format!("{{\"t\":{t},\"e\":{}}}", json_str(e)))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if body.len() <= MAX_ATTR_LEN || skip >= self.events.len() {
                return body;
            }
            skip += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_slots_land_in_the_third_group() {
        assert_eq!(uuid(0x0001), 0x8f6c0001_b5a3_4b2e_9d61_3b1c7a2e5f10);
        assert_eq!(uuid(0x000D), 0x8f6c000d_b5a3_4b2e_9d61_3b1c7a2e5f10);
        assert_eq!(uuid(0), UUID_BASE);
    }

    #[test]
    fn live_status_layout_is_pinned() {
        let s = LiveStatus {
            link_up: true,
            neutral_switch: false,
            interlock: Some(false),
            learn_flash: true,
            ota_busy: false,
            demo: false,
            gear: Gear::G(4),
            brightness_idx: 1,
            rpm: Some(4000.4),
            speed: Some(40.0),
            samples: 268,
            uptime_s: 0x01020304,
        };
        let b = s.encode();
        assert_eq!(b[0], PROTOCOL_VERSION);
        assert_eq!(b[1], FLAG_LINK_UP | FLAG_INTERLOCK_KNOWN | FLAG_LEARN_FLASH);
        assert_eq!(b[2], 4);
        assert_eq!(b[3], 1);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), 4000);
        assert_eq!(u16::from_le_bytes([b[6], b[7]]), 40);
        let ratio = f32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        assert!((ratio - 100.01).abs() < 0.01);
        assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), 268);
        assert_eq!(&b[16..20], &[0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn live_status_neutral_and_missing_readings() {
        let s = LiveStatus {
            link_up: true,
            neutral_switch: true,
            gear: Gear::Neutral,
            ..Default::default()
        };
        let b = s.encode();
        assert_eq!(b[1], FLAG_LINK_UP | FLAG_NEUTRAL_SWITCH);
        assert_eq!(b[2], GEAR_NEUTRAL);
        assert_eq!(&b[4..12], &[0u8; 8]); // rpm/speed/ratio all zero
        let unknown = LiveStatus::default().encode();
        assert_eq!(unknown[1], 0);
        assert_eq!(unknown[2], GEAR_UNKNOWN);
    }

    #[test]
    fn ratio_is_zero_below_one_kmh() {
        let s = LiveStatus {
            rpm: Some(1500.0),
            speed: Some(0.0),
            ..Default::default()
        };
        let b = s.encode();
        assert_eq!(&b[8..12], &0f32.to_le_bytes());
    }

    #[test]
    fn commands_decode() {
        assert_eq!(Command::decode(&[0x01]), Some(Command::WipeCalibration));
        assert_eq!(Command::decode(&[0x02, 2]), Some(Command::SetBrightness(2)));
        assert_eq!(Command::decode(&[0x02]), None);
        assert_eq!(Command::decode(&[0x03]), Some(Command::CheckUpdate));
        assert_eq!(Command::decode(&[0x04]), Some(Command::InstallUpdate));
        assert_eq!(Command::decode(&[0x05, 1]), Some(Command::SetBootPolicy(1)));
        assert_eq!(Command::decode(&[0x05, 3]), None);
        assert_eq!(Command::decode(&[0x06]), Some(Command::Reboot));
        assert_eq!(Command::decode(&[0x07]), Some(Command::ForgetWifi));
        assert_eq!(Command::decode(&[0x08]), Some(Command::ClearBlackBox));
        assert_eq!(
            Command::decode(b"\x09https://x.y/m.json"),
            Some(Command::SetManifestUrl("https://x.y/m.json".into()))
        );
        assert_eq!(
            Command::decode(b"\x09"),
            Some(Command::SetManifestUrl(String::new()))
        );
        assert_eq!(Command::decode(b"\x09http://plain"), None);
        assert_eq!(Command::decode(&[0x0A]), Some(Command::TestWifi));
        assert_eq!(Command::decode(&[0xFF]), None);
        assert_eq!(Command::decode(&[]), None);
    }

    #[test]
    fn wifi_credentials_round_trip() {
        let c = WifiCredentials {
            ssid: "Home WiFi".into(),
            psk: "hunter2!".into(),
        };
        let bytes = c.encode();
        assert_eq!(bytes[0], WIFI_CONFIG_VERSION);
        assert_eq!(bytes[1], 9);
        assert_eq!(WifiCredentials::decode(&bytes), Some(c));
        // Open network: empty psk is legal.
        let open = WifiCredentials {
            ssid: "cafe".into(),
            psk: String::new(),
        };
        assert_eq!(WifiCredentials::decode(&open.encode()), Some(open));
    }

    #[test]
    fn wifi_credentials_reject_malformed() {
        assert_eq!(WifiCredentials::decode(&[]), None);
        assert_eq!(WifiCredentials::decode(&[0x02, 1, b'a', 0]), None); // bad version
        assert_eq!(WifiCredentials::decode(&[0x01, 0, 0]), None); // empty ssid
        assert_eq!(WifiCredentials::decode(&[0x01, 3, b'a', b'b']), None); // short
        assert_eq!(WifiCredentials::decode(&[0x01, 1, b'a', 5, b'x']), None); // psk short
        assert_eq!(WifiCredentials::decode(&[0x01, 1, 0xFF, 0]), None); // bad utf8
    }

    #[test]
    fn hist_pages_cover_all_bins() {
        let mut hist = vec![0u16; BINS];
        hist[0] = 1;
        hist[149] = 2;
        hist[150] = 3;
        hist[299] = 4;
        let p0 = hist_page(&hist, 10, 0);
        let p1 = hist_page(&hist, 10, 1);
        assert_eq!(p0.len(), 5 + HIST_PAGE_BINS * 2);
        assert_eq!(p0[0], 0);
        assert_eq!(&p0[1..5], &10u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([p0[5], p0[6]]), 1);
        assert_eq!(u16::from_le_bytes([p0[5 + 149 * 2], p0[6 + 149 * 2]]), 2);
        assert_eq!(p1[0], 1);
        assert_eq!(u16::from_le_bytes([p1[5], p1[6]]), 3);
        assert_eq!(u16::from_le_bytes([p1[5 + 149 * 2], p1[6 + 149 * 2]]), 4);
        assert!(p0.len() <= MAX_ATTR_LEN);
        assert_eq!(bin_ratio(80), 100.5);
    }

    #[test]
    fn device_info_json_shape() {
        let d = DeviceInfo {
            fw_version: "0.2.0",
            project: "w230-gear-indicator",
            idf_version: "v5.3.3",
            built: "Sep 22 2026 10:00:00",
            hardware: "atom-matrix",
            slot: "ota_0",
            ota_capable: true,
            pending_verify: false,
            boot_policy: 1,
            uptime_s: 12,
            free_heap: 150000,
            min_free_heap: 140000,
            reset_reason: "power-on",
            mac: "AA:BB:CC:DD:EE:FF",
            ble_connections: 1,
        };
        let j = device_info_json(&d);
        assert!(j.starts_with("{\"proto\":1,\"fw\":\"0.2.0\","));
        assert!(j.contains(
            "\"slot\":\"ota_0\",\"otaCapable\":true,\"pendingVerify\":false,\"bootPolicy\":1"
        ));
        assert!(j.ends_with("\"bleConns\":1}"));
        assert!(j.len() <= MAX_ATTR_LEN);
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn black_box_json_shape() {
        let b = BlackBox {
            boots: 3,
            last_reset_reason: "brownout".into(),
            outcomes: [1, 2, 3, 4, 5, 6, 7],
            max_rpm: 7999.6,
            ..Default::default()
        };
        let j = black_box_json(&b);
        assert!(j.contains("\"boots\":3"));
        assert!(j.contains("\"lastReset\":\"brownout\""));
        assert!(j.contains("\"maxRpm\":8000"));
        assert!(j.contains("\"gates\":{\"accepted\":1,\"clutch\":2,\"neutral\":3,\"rpmLow\":4,\"speedLow\":5,\"outOfRange\":6,\"binFull\":7}"));
    }

    #[test]
    fn calibration_json_uncalibrated_and_calibrated() {
        let j = calibration_json(None, 0, 0, &[]);
        assert_eq!(
            j,
            "{\"factory\":[196.9,135.7,102.1,82.8,68.3,55.9],\"bands\":null,\"learned\":0,\"samples\":0,\"peaks\":[]}"
        );
        let bands = [200.0, 135.7, 102.1, 82.8, 68.3, 55.9];
        let j = calibration_json(Some(&bands), 1, 40, &[(200.0, 40)]);
        assert!(j.contains("\"bands\":[200.0,135.7,102.1,82.8,68.3,55.9],\"learned\":1,\"samples\":40,\"peaks\":[[200.0,40]]"));
    }

    #[test]
    fn calibration_json_trims_peaks_to_fit() {
        let peaks: Vec<(f32, u32)> = (0..200).map(|i| (100.0 + i as f32, 1000 - i)).collect();
        let j = calibration_json(None, 0, 0, &peaks);
        assert!(j.len() <= MAX_ATTR_LEN);
        assert!(j.contains("[100.0,1000]")); // strongest peak survives
    }

    #[test]
    fn ota_status_json_shape() {
        let s = OtaStatus {
            state: OtaState::Available,
            progress: 0,
            current: "0.2.0".into(),
            available: Some("0.3.0".into()),
            notes: Some("Faster shifts".into()),
            size: Some(1_200_000),
            error: None,
            last_check_uptime: Some(42),
            last_check_ok: true,
        };
        assert_eq!(
            ota_status_json(&s),
            "{\"state\":\"available\",\"progress\":0,\"current\":\"0.2.0\",\"available\":\"0.3.0\",\
             \"notes\":\"Faster shifts\",\"size\":1200000,\"error\":null,\"lastCheck\":42,\"lastCheckOk\":true}"
        );
        assert!(OtaState::Downloading.is_busy());
        assert!(!OtaState::Available.is_busy());
        let long = OtaStatus {
            notes: Some("x".repeat(1000)),
            ..Default::default()
        };
        assert!(ota_status_json(&long).len() <= MAX_ATTR_LEN);
    }

    #[test]
    fn wifi_and_settings_json_shape() {
        let w = WifiStatus {
            configured: true,
            ssid: Some("Home".into()),
            state: WifiState::Connected,
            ip: Some("192.168.1.20".into()),
            rssi: Some(-61),
            error: None,
        };
        assert_eq!(
            wifi_status_json(&w),
            "{\"configured\":true,\"ssid\":\"Home\",\"state\":\"connected\",\"ip\":\"192.168.1.20\",\"rssi\":-61,\"error\":null}"
        );
        let s = Settings {
            brightness_idx: 1,
            brightness_steps: &[40, 120, 255],
            boot_policy: 1,
            manifest_url: "https://example/m.json",
            manifest_url_is_default: true,
            wifi_ssid: None,
        };
        assert_eq!(
            settings_json(&s),
            "{\"brightness\":1,\"brightnessSteps\":[40,120,255],\"bootPolicy\":1,\"manifestUrl\":\"https://example/m.json\",\"manifestDefault\":true,\"wifiSsid\":null}"
        );
    }

    #[test]
    fn truncation_never_splits_a_character() {
        let mut s = "ota: bike is moving — stop before updating →→→→→→→→→→→→".to_string();
        truncate_utf8(&mut s, 24); // byte 24 lands inside the em dash
        assert!(s.len() <= 24);
        assert!(s.is_char_boundary(s.len()));
        let mut ring = EventRing::default();
        ring.push(1, "—".repeat(40)); // 120 bytes of 3-byte chars
        assert!(ring.to_json().len() <= MAX_ATTR_LEN);
        let long = OtaStatus {
            notes: Some("é".repeat(600)),
            ..Default::default()
        };
        assert!(ota_status_json(&long).len() <= MAX_ATTR_LEN);
    }

    #[test]
    fn event_ring_drops_oldest_and_fits() {
        let mut r = EventRing::default();
        for i in 0..40u32 {
            r.push(i, format!("event number {i} with some padding text"));
        }
        assert_eq!(r.len(), EventRing::CAPACITY);
        let j = r.to_json();
        assert!(j.len() <= MAX_ATTR_LEN);
        assert!(j.ends_with("\"e\":\"event number 39 with some padding text\"}]"));
        assert!(!j.contains("\"t\":0,"));
        assert_eq!(EventRing::default().to_json(), "[]");
    }
}
