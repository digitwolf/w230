//! Over-the-air updates: WiFi station + HTTPS manifest/image download into
//! the inactive OTA slot, with every safety net ESP-IDF offers switched on.
//!
//! Runs on its own thread so TLS never stalls the gear loop. Requests arrive
//! over a channel (boot-time check, app "check now"/"install", WiFi test);
//! progress is published into [`OtaShared`] which the main loop mirrors to
//! the BLE `ota-status` / `wifi-status` attributes and the LED matrix.
//!
//! Safety chain for an install, in order:
//!   1. manifest must parse strictly (project name, https URL, hex sha256,
//!      plausible size) — see `w230_core::ota_manifest`;
//!   2. version must be newer than the running one (no downgrades) and the
//!      running one must satisfy `min_version`;
//!   3. the bike must not be moving (main loop's `moving` flag);
//!   4. TLS against the ESP x509 bundle (Amazon roots included), HTTP 200
//!      and, when sent, a Content-Length matching the manifest;
//!   5. the image's embedded app descriptor must carry the same project name
//!      as the running image (esp-idf-sys stamps `libespidf`; a hand-built
//!      IDF project or another product would differ) and exactly the manifest
//!      version — checked before the first byte is committed;
//!   6. byte count and SHA-256 of the stream must equal the manifest;
//!   7. `esp_ota_end` re-validates the image (magic, segments, appended hash);
//!   8. only then is the boot slot switched, and the new image boots in
//!      PENDING_VERIFY: `main.rs` marks it valid after its self-test, else
//!      the bootloader rolls back on the next reset
//!      (`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`).

use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use embedded_svc::http::client::Client;
use embedded_svc::http::Method;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_hal::modem::WifiModem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::ota::{EspFirmwareInfoLoad, EspOta, FirmwareInfo, SlotState};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use log::{info, warn};
use sha2::{Digest, Sha256};

use w230_core::ble_proto::{OtaState, OtaStatus, WifiState, WifiStatus};
use w230_core::ota_manifest::{decide, Manifest, UpdateDecision};

use crate::config_store::ConfigStore;

/// WiFi is powered down after this long without an OTA request; the radio
/// costs current and interrupt jitter the gear display doesn't want.
const WIFI_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK: usize = 4096;
/// Bytes needed before the embedded app descriptor can be inspected
/// (image header 24 + segment header 8 + esp_app_desc_t 256).
const APP_DESC_END: usize = 24 + 8 + 256;

pub enum OtaRequest {
    /// Fetch the manifest; optionally go straight on to install.
    Check { install: bool },
    /// Install the manifest found by the last check (re-fetches if stale).
    Install,
    /// Bring WiFi up, report IP/RSSI, leave it to idle out.
    TestWifi,
    /// Self-test passed: cancel the pending rollback for this image.
    MarkValid,
}

#[derive(Default)]
pub struct OtaShared {
    pub ota: OtaStatus,
    pub wifi: WifiStatus,
    /// Set by the worker when either status changed; cleared by the reader.
    pub changed: bool,
    /// Set by the main loop: bike is moving (speed > 0), installs refused.
    pub moving: bool,
    /// Running slot label / rollback state, filled at start-up.
    pub slot: String,
    pub pending_verify: bool,
    pub rolled_back_from: Option<String>,
}

pub struct OtaHandle {
    pub tx: SyncSender<OtaRequest>,
    pub shared: Arc<Mutex<OtaShared>>,
}

impl OtaHandle {
    pub fn request(&self, r: OtaRequest) {
        if self.tx.try_send(r).is_err() {
            warn!("OTA: request queue full");
        }
    }
}

struct Worker {
    rx: Receiver<OtaRequest>,
    shared: Arc<Mutex<OtaShared>>,
    config: Arc<Mutex<ConfigStore>>,
    default_manifest_url: &'static str,
    current_version: &'static str,
    ota: EspOta,
    /// Project name from the running image's app descriptor; downloads must match.
    running_project: String,
    wifi: BlockingWifi<EspWifi<'static>>,
    wifi_up: bool,
    last_wifi_use: Instant,
    manifest: Option<Manifest>,
}

/// Spawn the worker. Also reads the running slot's rollback state so the
/// main loop can decide whether a self-test is owed.
pub fn spawn(
    modem: WifiModem,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    config: Arc<Mutex<ConfigStore>>,
    default_manifest_url: &'static str,
    current_version: &'static str,
) -> anyhow::Result<OtaHandle> {
    let (tx, rx) = sync_channel(4);
    let shared = Arc::new(Mutex::new(OtaShared::default()));

    let ota = EspOta::new()?;
    {
        let mut sh = shared.lock().unwrap();
        sh.ota.current = current_version.to_string();
        match ota.get_running_slot() {
            Ok(slot) => {
                sh.slot = slot.label.to_string();
                sh.pending_verify = slot.state == SlotState::Unverified;
                info!(
                    "OTA: running from '{}' ({:?}){}",
                    slot.label,
                    slot.state,
                    if sh.pending_verify {
                        " — rollback armed until self-test passes"
                    } else {
                        ""
                    }
                );
            }
            Err(e) => warn!("OTA: running slot unknown: {e}"),
        }
        if let Ok(Some(bad)) = ota.get_last_invalid_slot() {
            warn!("OTA: slot '{}' holds a rolled-back image", bad.label);
            sh.rolled_back_from = Some(bad.label.to_string());
        }
        let cfg = config.lock().unwrap();
        sh.wifi.configured = cfg.cfg.ssid.is_some();
        sh.wifi.ssid = cfg.cfg.ssid.clone();
        sh.changed = true;
    }

    // The station driver is created once (it owns the modem) and started
    // only when a request needs it.
    let wifi = BlockingWifi::wrap(EspWifi::new(modem, sysloop.clone(), Some(nvs))?, sysloop)?;

    let running_project = running_app_project();
    info!("OTA: running image descriptor project '{running_project}'");

    let worker = Worker {
        rx,
        shared: shared.clone(),
        config,
        default_manifest_url,
        current_version,
        ota,
        running_project,
        wifi,
        wifi_up: false,
        last_wifi_use: Instant::now(),
        manifest: None,
    };
    std::thread::Builder::new()
        .name("ota".into())
        .stack_size(20 * 1024) // TLS handshake + JSON parse live here
        .spawn(move || worker.run())?;
    Ok(OtaHandle { tx, shared })
}

impl Worker {
    fn run(mut self) {
        loop {
            match self.rx.recv_timeout(Duration::from_secs(5)) {
                Ok(OtaRequest::Check { install }) => {
                    if self.check() && install {
                        self.install();
                    }
                }
                Ok(OtaRequest::Install) => {
                    if self.manifest.is_none() && !self.check() {
                        continue;
                    }
                    self.install();
                }
                Ok(OtaRequest::TestWifi) => {
                    let _ = self.wifi_connect();
                }
                Ok(OtaRequest::MarkValid) => {
                    let pending = self.shared.lock().unwrap().pending_verify;
                    if pending {
                        match self.ota.mark_running_slot_valid() {
                            Ok(()) => {
                                info!("OTA: self-test passed, image marked valid");
                                let mut sh = self.shared.lock().unwrap();
                                sh.pending_verify = false;
                                sh.changed = true;
                            }
                            Err(e) => warn!("OTA: mark valid failed: {e}"),
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.wifi_up && self.last_wifi_use.elapsed() > WIFI_IDLE_TIMEOUT {
                        self.wifi_disconnect();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn set_ota<F: FnOnce(&mut OtaStatus)>(&self, f: F) {
        let mut sh = self.shared.lock().unwrap();
        f(&mut sh.ota);
        sh.changed = true;
    }

    fn set_wifi<F: FnOnce(&mut WifiStatus)>(&self, f: F) {
        let mut sh = self.shared.lock().unwrap();
        f(&mut sh.wifi);
        sh.changed = true;
    }

    fn fail(&self, msg: String) {
        warn!("OTA: {msg}");
        self.set_ota(|o| {
            o.state = OtaState::Failed;
            o.error = Some(msg);
        });
    }

    fn manifest_url(&self) -> String {
        self.config
            .lock()
            .unwrap()
            .cfg
            .manifest_url
            .clone()
            .unwrap_or_else(|| self.default_manifest_url.to_string())
    }

    /// Connect the station with the stored credentials. Idempotent.
    fn wifi_connect(&mut self) -> bool {
        self.last_wifi_use = Instant::now();
        if self.wifi_up && self.wifi.is_connected().unwrap_or(false) {
            return true;
        }
        let (ssid, psk) = {
            let cfg = self.config.lock().unwrap();
            (cfg.cfg.ssid.clone(), cfg.cfg.psk.clone())
        };
        let Some(ssid) = ssid else {
            self.set_wifi(|w| {
                w.configured = false;
                w.state = WifiState::Failed;
                w.error = Some("no WiFi credentials stored".into());
            });
            return false;
        };
        self.set_wifi(|w| {
            w.configured = true;
            w.ssid = Some(ssid.clone());
            w.state = WifiState::Connecting;
            w.ip = None;
            w.rssi = None;
            w.error = None;
        });
        let result = (|| -> anyhow::Result<(String, i32)> {
            let conf = Configuration::Client(ClientConfiguration {
                ssid: ssid
                    .as_str()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("ssid too long"))?,
                password: psk
                    .as_str()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("password too long"))?,
                auth_method: if psk.is_empty() {
                    AuthMethod::None
                } else {
                    AuthMethod::WPA2Personal
                },
                ..Default::default()
            });
            self.wifi.set_configuration(&conf)?;
            if !self.wifi_up {
                self.wifi.start()?;
                self.wifi_up = true;
            }
            self.wifi.connect()?;
            self.wifi.wait_netif_up()?;
            let ip = self.wifi.wifi().sta_netif().get_ip_info()?.ip.to_string();
            let rssi = self.wifi.wifi().get_rssi().unwrap_or(0);
            Ok((ip, rssi))
        })();
        match result {
            Ok((ip, rssi)) => {
                info!("WIFI: connected to '{ssid}' as {ip} ({rssi} dBm)");
                self.set_wifi(|w| {
                    w.state = WifiState::Connected;
                    w.ip = Some(ip);
                    w.rssi = Some(rssi);
                });
                true
            }
            Err(e) => {
                warn!("WIFI: connect to '{ssid}' failed: {e}");
                self.set_wifi(|w| {
                    w.state = WifiState::Failed;
                    w.error = Some(format!("{e}"));
                });
                let _ = self.wifi.disconnect();
                false
            }
        }
    }

    fn wifi_disconnect(&mut self) {
        if self.wifi_up {
            let _ = self.wifi.disconnect();
            let _ = self.wifi.stop();
            self.wifi_up = false;
            info!("WIFI: powered down");
        }
        self.set_wifi(|w| {
            w.state = WifiState::Off;
            w.ip = None;
            w.rssi = None;
        });
    }

    fn http_client(&self) -> anyhow::Result<Client<EspHttpConnection>> {
        let conn = EspHttpConnection::new(&HttpConfig {
            buffer_size: Some(CHUNK),
            buffer_size_tx: Some(1024),
            timeout: Some(HTTP_TIMEOUT),
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        })?;
        Ok(Client::wrap(conn))
    }

    /// Fetch + parse the manifest and decide. Returns true when an install
    /// is possible (newer version, running version not below min_version).
    fn check(&mut self) -> bool {
        let uptime = uptime_s();
        self.set_ota(|o| {
            o.state = OtaState::Connecting;
            o.progress = 0;
            o.error = None;
        });
        if !self.wifi_connect() {
            self.fail("WiFi connection failed".into());
            return false;
        }
        self.set_ota(|o| o.state = OtaState::Checking);
        let url = self.manifest_url();
        info!("OTA: fetching manifest {url}");
        let body = match self.get_small(&url) {
            Ok(b) => b,
            Err(e) => {
                self.fail(format!("manifest fetch failed: {e}"));
                self.set_ota(|o| {
                    o.last_check_uptime = Some(uptime);
                    o.last_check_ok = false;
                });
                return false;
            }
        };
        let manifest = match Manifest::parse(&body) {
            Ok(m) => m,
            Err(e) => {
                self.fail(format!("{e}"));
                self.set_ota(|o| {
                    o.last_check_uptime = Some(uptime);
                    o.last_check_ok = false;
                });
                return false;
            }
        };
        let decision = decide(self.current_version, &manifest);
        info!(
            "OTA: manifest version {} vs running {} → {decision:?}",
            manifest.version, self.current_version
        );
        let installable = matches!(decision, UpdateDecision::Install);
        self.set_ota(|o| {
            o.last_check_uptime = Some(uptime);
            o.last_check_ok = true;
            o.notes = manifest.notes.clone();
            o.size = Some(manifest.size);
            match &decision {
                UpdateDecision::Install => {
                    o.state = OtaState::Available;
                    o.available = Some(manifest.version.clone());
                }
                UpdateDecision::UpToDate => {
                    o.state = OtaState::UpToDate;
                    o.available = None;
                }
                UpdateDecision::TooOld { min_version } => {
                    o.state = OtaState::Failed;
                    o.available = Some(manifest.version.clone());
                    o.error = Some(format!(
                        "firmware {} is below the update's minimum {min_version}: flash over USB",
                        o.current
                    ));
                }
            }
        });
        self.manifest = installable.then_some(manifest);
        installable
    }

    fn get_small(&self, url: &str) -> anyhow::Result<String> {
        let mut client = self.http_client()?;
        let req = client.request(Method::Get, url, &[("Accept", "application/json")])?;
        let mut resp = req.submit()?;
        let status = resp.status();
        if status != 200 {
            anyhow::bail!("HTTP {status}");
        }
        let mut body = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
            if body.len() > 8192 {
                anyhow::bail!("manifest larger than 8 KiB");
            }
        }
        Ok(String::from_utf8(body)?)
    }

    fn install(&mut self) {
        let Some(manifest) = self.manifest.clone() else {
            self.fail("no update available to install".into());
            return;
        };
        if self.shared.lock().unwrap().moving {
            self.fail("bike is moving — stop before updating".into());
            return;
        }
        if !self.wifi_connect() {
            self.fail("WiFi connection failed".into());
            return;
        }
        self.set_ota(|o| {
            o.state = OtaState::Downloading;
            o.progress = 0;
            o.error = None;
        });
        match self.download_and_write(&manifest) {
            Ok(()) => {
                info!(
                    "OTA: image {} written and activated, rebooting",
                    manifest.version
                );
                self.set_ota(|o| {
                    o.state = OtaState::Rebooting;
                    o.progress = 100;
                });
                // Let the BLE notification and the LED tick reach the user.
                std::thread::sleep(Duration::from_millis(2500));
                esp_idf_hal::reset::restart();
            }
            Err(e) => {
                self.fail(format!("{e}"));
                // Stale manifest could be the reason; make the next Install re-check.
                self.manifest = None;
            }
        }
    }

    fn download_and_write(&mut self, m: &Manifest) -> anyhow::Result<()> {
        info!("OTA: downloading {} ({} bytes)", m.url, m.size);
        let mut client = self.http_client()?;
        let req = client.request(
            Method::Get,
            &m.url,
            &[("Accept", "application/octet-stream")],
        )?;
        let mut resp = req.submit()?;
        let status = resp.status();
        if status != 200 {
            anyhow::bail!("HTTP {status} fetching image");
        }
        if let Some(len) = resp
            .header("Content-Length")
            .and_then(|v| v.trim().parse::<u32>().ok())
        {
            if len != m.size {
                anyhow::bail!("Content-Length {len} != manifest size {}", m.size);
            }
        }

        // The update handle borrows `self.ota` for its whole life, so status
        // goes through a clone of the shared handle instead of `self`.
        let shared = self.shared.clone();
        let running_project = self.running_project.clone();
        let set_progress = |pct: u8| {
            let mut sh = shared.lock().unwrap();
            sh.ota.progress = pct;
            sh.changed = true;
        };
        let mut update = self.ota.initiate_update()?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut head: Vec<u8> = Vec::with_capacity(APP_DESC_END);
        let mut head_checked = false;
        let mut total: u32 = 0;
        let mut last_pct = 0u8;
        let mut last_log = Instant::now();
        let result = (|| -> anyhow::Result<()> {
            loop {
                let n = resp.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                let chunk = &buf[..n];
                total = total
                    .checked_add(n as u32)
                    .ok_or_else(|| anyhow::anyhow!("overflow"))?;
                if total > m.size {
                    anyhow::bail!("image longer than manifest size");
                }
                if !head_checked {
                    head.extend_from_slice(chunk);
                    if head.len() >= APP_DESC_END {
                        verify_app_descriptor(&head[..APP_DESC_END], m, &running_project)?;
                        head_checked = true;
                        head = Vec::new();
                    }
                }
                hasher.update(chunk);
                update.write(chunk)?;
                let pct = ((total as u64 * 100) / m.size as u64) as u8;
                if pct != last_pct {
                    last_pct = pct;
                    set_progress(pct);
                }
                if last_log.elapsed() > Duration::from_secs(2) {
                    last_log = Instant::now();
                    info!("OTA: {total}/{} bytes ({pct}%)", m.size);
                }
            }
            if !head_checked {
                anyhow::bail!("image too short to carry an app descriptor");
            }
            if total != m.size {
                anyhow::bail!("received {total} bytes, manifest says {}", m.size);
            }
            let digest = hasher.finalize();
            if digest[..] != m.sha256_bytes()[..] {
                anyhow::bail!("sha256 mismatch");
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = update.abort();
            return Err(e);
        }
        {
            let mut sh = shared.lock().unwrap();
            sh.ota.state = OtaState::Verifying;
            sh.changed = true;
        }
        // esp_ota_end: image header/segment/hash validation on the slot.
        let finished = update.finish()?;
        finished.activate()?;
        drop(shared);
        self.last_wifi_use = Instant::now();
        Ok(())
    }
}

/// Project name stamped into the running image (`esp_app_desc_t`).
fn running_app_project() -> String {
    unsafe {
        let d = esp_idf_svc::sys::esp_app_get_description();
        if d.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr((*d).project_name.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// The 256-byte `esp_app_desc_t` at the head of every ESP-IDF image carries
/// the project name and version the build stamped in — refuse anything that
/// isn't built like this firmware, at the manifest's version, before writing
/// further.
fn verify_app_descriptor(head: &[u8], m: &Manifest, expected_project: &str) -> anyhow::Result<()> {
    let mut info = FirmwareInfo {
        version: Default::default(),
        released: Default::default(),
        description: Some(Default::default()),
        signature: None,
        download_id: None,
    };
    let loaded = EspFirmwareInfoLoad
        .fetch(head, &mut info)
        .map_err(|e| anyhow::anyhow!("app descriptor unreadable: {e:?}"))?;
    if !loaded {
        anyhow::bail!("app descriptor incomplete");
    }
    let project = info.description.as_deref().unwrap_or("");
    if project != expected_project {
        anyhow::bail!("image is project '{project}', expected '{expected_project}'");
    }
    if info.version.as_str() != m.version {
        anyhow::bail!(
            "image version '{}' differs from manifest '{}'",
            info.version,
            m.version
        );
    }
    info!(
        "OTA: image descriptor OK: {project} {} built {}",
        info.version, info.released
    );
    Ok(())
}

pub fn uptime_s() -> u32 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1_000_000) as u32
}
