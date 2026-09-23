//! User settings in NVS (namespace `w230cfg`): WiFi credentials, the
//! boot-time update policy, an optional manifest-URL override, brightness.
//! Written only on change from the app (BLE) or the button; read at boot and
//! by the OTA worker before it brings WiFi up.

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use log::warn;

const NS: &str = "w230cfg";
const K_SSID: &str = "ssid";
const K_PSK: &str = "psk";
const K_BOOT: &str = "bootpol";
const K_URL: &str = "murl";
const K_BRIGHT: &str = "bright";

/// Boot-time OTA policy values (also the BLE `SetBootPolicy` payload).
#[allow(dead_code)]
pub const BOOT_OFF: u8 = 0;
pub const BOOT_CHECK: u8 = 1;
pub const BOOT_CHECK_AND_INSTALL: u8 = 2;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub ssid: Option<String>,
    pub psk: String,
    pub boot_policy: u8,
    /// `None` = use the compiled-in default.
    pub manifest_url: Option<String>,
    pub brightness_idx: u8,
}

pub struct ConfigStore {
    nvs: EspNvs<NvsDefault>,
    pub cfg: Config,
}

impl ConfigStore {
    pub fn open(part: EspDefaultNvsPartition, default_boot_policy: u8) -> anyhow::Result<Self> {
        let nvs = EspNvs::new(part, NS, true)?;
        let mut s = Self {
            nvs,
            cfg: Config {
                boot_policy: default_boot_policy,
                brightness_idx: 1,
                ..Default::default()
            },
        };
        s.load();
        Ok(s)
    }

    fn get_str(&self, key: &str) -> Option<String> {
        let mut buf = [0u8; 256];
        match self.nvs.get_str(key, &mut buf) {
            Ok(Some(s)) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        }
    }

    fn load(&mut self) {
        self.cfg.ssid = self.get_str(K_SSID);
        self.cfg.psk = self.get_str(K_PSK).unwrap_or_default();
        if let Ok(Some(v)) = self.nvs.get_u8(K_BOOT) {
            self.cfg.boot_policy = v.min(BOOT_CHECK_AND_INSTALL);
        }
        self.cfg.manifest_url = self.get_str(K_URL);
        if let Ok(Some(v)) = self.nvs.get_u8(K_BRIGHT) {
            self.cfg.brightness_idx = v;
        }
    }

    pub fn set_wifi(&mut self, ssid: &str, psk: &str) {
        if let Err(e) = self
            .nvs
            .set_str(K_SSID, ssid)
            .and_then(|_| self.nvs.set_str(K_PSK, psk))
        {
            warn!("CFG: storing WiFi credentials failed: {e}");
        }
        self.cfg.ssid = Some(ssid.to_string());
        self.cfg.psk = psk.to_string();
    }

    pub fn clear_wifi(&mut self) {
        let _ = self.nvs.remove(K_SSID);
        let _ = self.nvs.remove(K_PSK);
        self.cfg.ssid = None;
        self.cfg.psk.clear();
    }

    pub fn set_boot_policy(&mut self, p: u8) {
        let p = p.min(BOOT_CHECK_AND_INSTALL);
        if let Err(e) = self.nvs.set_u8(K_BOOT, p) {
            warn!("CFG: storing boot policy failed: {e}");
        }
        self.cfg.boot_policy = p;
    }

    pub fn set_manifest_url(&mut self, url: Option<&str>) {
        match url {
            Some(u) if !u.is_empty() => {
                if let Err(e) = self.nvs.set_str(K_URL, u) {
                    warn!("CFG: storing manifest URL failed: {e}");
                }
                self.cfg.manifest_url = Some(u.to_string());
            }
            _ => {
                let _ = self.nvs.remove(K_URL);
                self.cfg.manifest_url = None;
            }
        }
    }

    pub fn set_brightness(&mut self, idx: u8) {
        let _ = self.nvs.set_u8(K_BRIGHT, idx);
        self.cfg.brightness_idx = idx;
    }
}
