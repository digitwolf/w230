//! Kawasaki W230 gear indicator — M5Stack ATOM Matrix (ESP32-PICO-D4).
//!
//! Reads the KDS diagnostic K-line through a LINTTL3 (TJA1021/SIT1021T)
//! TTL-UART<->LIN module and shows the current gear on the 5x5 LED matrix:
//! green N, cyan 1-6, dim red dash when unknown, all red = no link.
//! Neutral comes from the switch wire on G23 and nowhere else (ECU reg 0x03
//! reads 00 00 whenever the bike is MOVING as well as during neutral+clutch —
//! useless for any gating; kept as dashboard telemetry only). Gears 1-6 come
//! from the RPM/speed ratio (factory-preset bands, refined by ride learning).
//!
//! Wiring (module pins per vendor sheet — note TX/RX are named from the
//! MODULE's perspective, so they cross over to the ESP32):
//!   module VIN  <- switched 12 V (fused)      module LIN <- KDS K-line (GY/BL)
//!   module INH  <- tie to VIN for HOST mode   module GND <- bike ground
//!   module TX   -> ESP32 G32 (UART RX)        module SLP <- ESP32 G22 (high = awake)
//!   module RX   <- ESP32 G26 (UART TX)        module GND <- ESP32 GND (common)
//!
//! All K-line traffic (TX frames, echoes, ECU replies) is hex-logged over USB:
//! `espflash monitor` at 115200 to watch it.

mod ble;
mod config_store;
mod kds;
mod learn_store;
mod ota;
mod web;

use ble::{Attr, Ble};
use config_store::{ConfigStore, BOOT_CHECK, BOOT_CHECK_AND_INSTALL};
use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{PinDriver, Pull};
use esp_idf_hal::prelude::*;
use esp_idf_hal::uart::{config::Config as UartConfig, UartDriver};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use kds::Kds;
use learn_store::RatioLearner;
use log::{info, warn};
use ota::{OtaHandle, OtaRequest};
use smart_leds::SmartLedsWrite;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use w230_core::ble_proto::{
    self as proto, Command, EventRing, LiveStatus, OtaState, OtaStatus, WifiStatus,
};
use w230_core::display;
use w230_core::gear::{Gear, GearEstimator, GearInputs};
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

/// Firmware version, from esp32/Cargo.toml (build.rs keeps sdkconfig's
/// CONFIG_APP_PROJECT_VER identical so the image descriptor agrees).
const FW_VERSION: &str = env!("CARGO_PKG_VERSION");
const HARDWARE: &str = "atom-matrix";
/// Where releases live. `scripts/release.sh` publishes here; the app can
/// point a device at a staging manifest instead (Settings → manifest URL).
const OTA_MANIFEST_URL: &str = match option_env!("W230_OTA_MANIFEST_URL") {
    Some(u) => u,
    None => "https://d2ilb6j4crje4c.cloudfront.net/w230/manifest.json",
};
/// Boot-time update behaviour when nothing is stored yet: check only, never
/// install unattended (a reboot at key-on is not what a rider expects).
const DEFAULT_BOOT_POLICY: u8 = BOOT_CHECK;
/// Boot check waits this long so the gear display comes up first.
const BOOT_CHECK_DELAY: Duration = Duration::from_secs(8);
/// A freshly updated image must survive this long, with the LED thread and
/// BLE up and the poll loop cycling, before its rollback is cancelled.
const SELF_TEST_UPTIME: Duration = Duration::from_secs(20);
/// JSON attributes are refreshed at this rate while a phone is connected.
const BLE_SLOW_PUBLISH: Duration = Duration::from_secs(1);
/// After a failed install the red cross stays up this long.
const OTA_FAIL_SHOW: Duration = Duration::from_secs(4);

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const RECONNECT_INTERVAL: Duration = Duration::from_millis(1000);
const BRIGHTNESS_STEPS: [u8; 3] = [40, 120, 255];
// Short interval: on the bike, key-off cuts our power instantly — anything
// unsaved is gone. Writes only happen when new samples arrived (dirty flag),
// so this costs ~20 NVS writes/minute of actual riding — fine for NVS wear
// during the calibration phase; can be relaxed once bands are learned.
const LEARN_SAVE_INTERVAL: Duration = Duration::from_secs(3);
/// Button hold this long = wipe the learned calibration.
const LEARN_RESET_HOLD: Duration = Duration::from_secs(3);
/// The interlock register changes on human timescales — reading it every Nth
/// cycle buys ~130 ms on the other cycles.
const INTERLOCK_EVERY_N_CYCLES: u32 = 8;

/// Gear-register hunt: scan all local identifiers once after connecting, then
/// keep re-reading the supported ones and log every value change. Shift
/// through the gears and watch which register follows. (Used 2026-08-02 to
/// find the clutch switch in reg 0x03; no gear-number register exists.)
const DIAG_SCAN: bool = false;

/// Demo mode: override the display with gears 1..6, stepping every
/// `DEMO_STEP`. TEMPORARY — turn off for real use.
const DEMO_MODE: bool = false;
const DEMO_STEP: Duration = Duration::from_secs(15);

/// WiFi softAP + HTTP dashboard. Diagnostic tooling only: WiFi's interrupt
/// load lives on core 0 and its TX bursts are the prime suspect for WS2812
/// glitch pixels; leave off so both cores serve signal processing/display.
const WIFI_DIAG: bool = false;

/// Deep scan: probe services 0x1A (ECU identification) and 0x22 (common
/// identifiers 0x0000-0x0FFF, ~10 min), then change-watch everything found.
/// (Run 2026-08-02: service 0x22 absent entirely; 0x1A yields ID strings only
/// — confirmed no neutral/gear data exists beyond the 59 0x21 registers.)
const DEEP_SCAN: bool = false;

/// Fast-path neutral check, callable between the slow KDS register reads: the
/// switch commits instantly in the estimator, so sampling it here cuts N
/// latency from a full cycle (~0.4 s) to one register read (~0.13 s). Only the
/// neutral transition re-renders; ratio classification stays once per cycle
/// where a coherent rpm/speed pair exists.
#[allow(clippy::too_many_arguments)]
fn neutral_tick(
    neutral_low: bool,
    neutral_was: &mut bool,
    estimator: &mut GearEstimator,
    led_tx: &mpsc::SyncSender<[rgb::RGB8; 25]>,
    link_up: bool,
    brightness: u8,
) {
    if neutral_low == *neutral_was {
        return;
    }
    info!(
        "NEUTRAL PIN: {}",
        if neutral_low {
            "LOW (neutral)"
        } else {
            "HIGH (in gear)"
        }
    );
    *neutral_was = neutral_low;
    let gear = estimator.update(&GearInputs {
        neutral_switch: neutral_low,
        ..Default::default()
    });
    if !DEMO_MODE {
        let _ = led_tx.try_send(display::render(gear, link_up, brightness, false));
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("W230 gear indicator starting (ATOM Matrix + LINTTL3/TJA1021)");

    let p = Peripherals::take()?;
    let (wifi_modem, bt_modem) = p.modem.split();

    // --- LED matrix: 25x WS2812 on GPIO27, driven from CPU core 1 ---
    // WiFi interrupts live on core 0 and preempt the RMT refill ISR there,
    // corrupting WS2812 timing (random glitch pixels). Creating the RMT
    // driver inside a core-1-pinned thread allocates its ISR on core 1,
    // where nothing competes with it.
    let (led_tx, led_rx) = mpsc::sync_channel::<[rgb::RGB8; 25]>(4);
    {
        let rmt = p.rmt.channel0;
        let led_pin = p.pins.gpio27;
        esp_idf_hal::task::thread::ThreadSpawnConfiguration {
            name: Some(b"led\0"),
            pin_to_core: Some(esp_idf_hal::cpu::Core::Core1),
            stack_size: 4096,
            ..Default::default()
        }
        .set()?;
        std::thread::Builder::new()
            .stack_size(4096)
            .spawn(move || {
                let mut leds = match Ws2812Esp32Rmt::new(rmt, led_pin) {
                    Ok(l) => l,
                    Err(e) => {
                        warn!("LED driver init failed: {e}");
                        return;
                    }
                };
                while let Ok(frame) = led_rx.recv() {
                    if let Err(e) = leds.write(frame.into_iter()) {
                        warn!("LED write failed: {e}");
                    }
                }
            })?;
        esp_idf_hal::task::thread::ThreadSpawnConfiguration::default().set()?;
    }

    // --- Button (GPIO39, active low, external pull-up on board) ---
    let button = PinDriver::input(p.pins.gpio39)?;

    // --- Neutral switch input (G23, bottom header): grounds when in neutral ---
    let mut neutral = PinDriver::input(p.pins.gpio23)?;
    neutral.set_pull(Pull::Up)?;

    // --- TJA1021 SLP (G22): drive high = normal mode (not sleep) ---
    let mut slp = PinDriver::output(p.pins.gpio22)?;
    slp.set_high()?;
    info!("TJA1021 SLP driven high (normal mode)");

    // --- K-line UART (Grove port): G26 = ESP32 TX, G32 = ESP32 RX ---
    // The UART owns the pins for good; the ISO-14230 fast-init low pulse is
    // generated inside Kds by a temporary baud-rate drop, so the request can
    // follow the wake pattern with no driver-setup delay.
    let uart = UartDriver::new(
        p.uart1,
        p.pins.gpio26,
        p.pins.gpio32,
        Option::<esp_idf_hal::gpio::Gpio0>::None,
        Option::<esp_idf_hal::gpio::Gpio0>::None,
        &UartConfig::new().baudrate(Hertz(10_400)),
    )?;
    let mut kds = Kds::new(uart);

    // --- Self-learning ratio calibration, persisted in NVS ---
    // (The learning pipeline is unit-tested on the host: `cargo test-host`.)
    let nvs_part = EspDefaultNvsPartition::take()?;
    let nvs = EspNvs::new(nvs_part.clone(), "gearlearn", true)?;
    let mut learner = RatioLearner::new(nvs);
    learner.dump(); // post-ride diagnostic: full histogram in the boot log

    // --- User settings (WiFi credentials, update policy, brightness) ---
    let config = Arc::new(Mutex::new(ConfigStore::open(
        nvs_part.clone(),
        DEFAULT_BOOT_POLICY,
    )?));
    let sysloop = EspSystemEventLoop::take()?;

    // --- BLE GATT service for the iOS app ---
    // Failure here is logged, not fatal: the gear display must not depend
    // on the phone link. (No BLE also means no self-test pass → a bad OTA
    // image that breaks Bluetooth rolls back, which is the intent.)
    let ble = match Ble::start(bt_modem, nvs_part.clone()) {
        Ok(b) => Some(b),
        Err(e) => {
            warn!("BLE: start failed: {e} — continuing without the app link");
            None
        }
    };

    // --- WiFi: either the legacy softAP dashboard or the OTA station ---
    let (webdiag, ota) = if WIFI_DIAG {
        info!("WIFI_DIAG on: softAP dashboard replaces OTA for this build");
        (Some(web::start(wifi_modem, sysloop, nvs_part)?), None)
    } else {
        let ota = match ota::spawn(
            wifi_modem,
            sysloop,
            nvs_part,
            config.clone(),
            OTA_MANIFEST_URL,
            FW_VERSION,
        ) {
            Ok(h) => Some(h),
            Err(e) => {
                warn!("OTA: worker start failed: {e} — updates unavailable this boot");
                None
            }
        };
        (None, ota)
    };
    let boot_slot_info = ota.as_ref().map(|o| {
        let sh = o.shared.lock().unwrap();
        (
            sh.slot.clone(),
            sh.pending_verify,
            sh.rolled_back_from.clone(),
        )
    });
    info!(
        "W230 firmware {FW_VERSION} (BLE proto {}), slot {:?}",
        proto::PROTOCOL_VERSION,
        boot_slot_info
    );

    let mut estimator = GearEstimator::new();
    // Factory-provisional bands: digits work with zero calibration; learned
    // bands replace them below as soon as riding data yields peaks.
    estimator.set_bands(w230_core::gear::FACTORY_BANDS, w230_core::gear::NUM_GEARS);
    let mut learned_count = 0usize;
    if let Some((bands, n)) = learner.derive_bands(&w230_core::gear::FACTORY_BANDS) {
        estimator.set_bands(bands, w230_core::gear::NUM_GEARS);
        learned_count = n;
    }

    let demo_start = Instant::now();
    let mut demo_last_gear: u8 = 0;
    let mut last_reconnect = Instant::now() - RECONNECT_INTERVAL;
    let mut last_learn_save = Instant::now();
    let mut diag_regs: Option<Vec<(u8, Vec<u8>)>> = None;
    let mut deep_watch: Option<Vec<(u16, Vec<u8>)>> = None;
    let mut brightness_idx: usize =
        (config.lock().unwrap().cfg.brightness_idx as usize).min(BRIGHTNESS_STEPS.len() - 1);
    let mut button_was_down = false;
    let mut button_down_at = Instant::now();
    let mut neutral_was_active = false;
    let mut cycle: u32 = 0;
    // Interlock state (neutral+clutch chain) between its every-Nth-cycle reads.
    let mut interlock: Option<bool> = None;
    // Previous cycle's raw RPM reading + timestamp, for time-aligning the
    // rpm/speed pair under acceleration.
    let mut prev_rpm: Option<(f32, Instant)> = None;
    // Green-dash heartbeat: lit briefly after each calibration NVS write.
    let mut learn_flash_until = Instant::now() - Duration::from_secs(1);
    // Last frame pushed to the strip: WS2812 writes are vulnerable to WiFi
    // interrupt jitter (random glitch pixels), so only rewrite on change.
    let mut last_frame: Option<[rgb::RGB8; 25]> = None;

    // --- BLE / OTA bookkeeping ---
    let boot = Instant::now();
    let mut events = EventRing::default();
    events.push(0, format!("boot fw {FW_VERSION}"));
    if let Some((slot, pending, rolled)) = &boot_slot_info {
        events.push(
            0,
            format!(
                "slot {slot}{}",
                if *pending { " pending-verify" } else { "" }
            ),
        );
        if let Some(r) = rolled {
            events.push(0, format!("rolled back from {r}"));
        }
    }
    let mut last_slow_publish = Instant::now() - BLE_SLOW_PUBLISH;
    let mut published: PublishedCache = Default::default();
    let mut boot_check_done = false;
    let mut self_test_done = boot_slot_info.as_ref().map_or(true, |(_, p, _)| !*p);
    let mut ota_status = OtaStatus::default();
    let mut wifi_status = WifiStatus::default();
    let mut ota_fail_until: Option<Instant> = None;
    let mut ota_was_busy = false;
    let mut link_was_up = false;

    loop {
        let uptime_s = boot.elapsed().as_secs() as u32;

        // --- OTA state: mirror worker → BLE/events, drive the display ---
        if let Some(o) = &ota {
            let mut sh = o.shared.lock().unwrap();
            if sh.changed {
                sh.changed = false;
                if sh.ota.state != ota_status.state {
                    events.push(uptime_s, format!("ota {}", sh.ota.state.as_str()));
                    if let Some(e) = &sh.ota.error {
                        events.push(
                            uptime_s,
                            format!("ota: {}", e.chars().take(40).collect::<String>()),
                        );
                    }
                    if sh.ota.state == OtaState::Failed {
                        ota_fail_until = Some(Instant::now() + OTA_FAIL_SHOW);
                    }
                }
                if sh.wifi.state != wifi_status.state {
                    events.push(uptime_s, format!("wifi {}", sh.wifi.state.as_str()));
                }
                ota_status = sh.ota.clone();
                wifi_status = sh.wifi.clone();
                drop(sh);
                if let Some(b) = &ble {
                    b.notify(
                        Attr::OtaStatus,
                        proto::ota_status_json(&ota_status).as_bytes(),
                    );
                    b.notify(
                        Attr::WifiStatus,
                        proto::wifi_status_json(&wifi_status).as_bytes(),
                    );
                }
            }
        }
        // While an image is being written the K-line polling pauses (the
        // download needs the CPU and the bike must be stationary anyway);
        // the matrix shows the download as a blue fill, then a check/cross.
        let ota_busy = matches!(
            ota_status.state,
            OtaState::Downloading | OtaState::Verifying | OtaState::Rebooting
        );
        if ota_busy {
            if !ota_was_busy {
                kds.connected = false;
                learner.save();
            }
            ota_was_busy = true;
            let frame = if ota_status.state == OtaState::Rebooting {
                display::render_ota_result(true, BRIGHTNESS_STEPS[brightness_idx])
            } else {
                display::render_progress(ota_status.progress, BRIGHTNESS_STEPS[brightness_idx])
            };
            if last_frame != Some(frame) {
                let _ = led_tx.try_send(frame);
                last_frame = Some(frame);
            }
            handle_ble_commands(
                &ble,
                &ota,
                &config,
                &mut learner,
                &mut estimator,
                &mut brightness_idx,
                &mut events,
                uptime_s,
            );
            FreeRtos::delay_ms(100);
            continue;
        }
        ota_was_busy = false;
        if let Some(until) = ota_fail_until {
            if Instant::now() < until {
                let frame = display::render_ota_result(false, BRIGHTNESS_STEPS[brightness_idx]);
                if last_frame != Some(frame) {
                    let _ = led_tx.try_send(frame);
                    last_frame = Some(frame);
                }
                FreeRtos::delay_ms(100);
                continue;
            }
            ota_fail_until = None;
        }

        // Boot-time update check: once, after the display is up, only when
        // credentials exist, and never while this image is still on probation.
        if !boot_check_done && boot.elapsed() >= BOOT_CHECK_DELAY {
            boot_check_done = true;
            let (policy, has_wifi) = {
                let c = config.lock().unwrap();
                (c.cfg.boot_policy, c.cfg.ssid.is_some())
            };
            if let Some(o) = &ota {
                if policy >= BOOT_CHECK && has_wifi && self_test_done {
                    info!("OTA: boot-time check (policy {policy})");
                    o.request(OtaRequest::Check {
                        install: policy == BOOT_CHECK_AND_INSTALL,
                    });
                } else if policy >= BOOT_CHECK && has_wifi {
                    info!("OTA: boot check skipped — image pending verification");
                }
            }
        }

        // Rollback self-test: the update image has run long enough with the
        // display thread, BLE and the poll loop all alive → keep it.
        if !self_test_done && boot.elapsed() >= SELF_TEST_UPTIME {
            self_test_done = true;
            let healthy = ble.is_some()
                && led_tx
                    .try_send(last_frame.unwrap_or([rgb::RGB8::default(); 25]))
                    .is_ok();
            if healthy {
                if let Some(o) = &ota {
                    o.request(OtaRequest::MarkValid);
                }
                events.push(uptime_s, "self-test passed");
            } else {
                warn!(
                    "SELF-TEST failed (ble={}), leaving rollback armed",
                    ble.is_some()
                );
                events.push(uptime_s, "self-test FAILED");
            }
        }

        handle_ble_commands(
            &ble,
            &ota,
            &config,
            &mut learner,
            &mut estimator,
            &mut brightness_idx,
            &mut events,
            uptime_s,
        );

        // Button: short press cycles brightness, 3s hold wipes calibration.
        let down = button.is_low();
        if down && !button_was_down {
            button_down_at = Instant::now();
        }
        if !down && button_was_down {
            if button_down_at.elapsed() >= LEARN_RESET_HOLD {
                learner.clear();
                estimator.clear_bands();
                info!("LEARN: calibration wiped (button hold)");
                events.push(uptime_s, "calibration wiped (button)");
            } else {
                brightness_idx = (brightness_idx + 1) % BRIGHTNESS_STEPS.len();
                info!("brightness -> {}", BRIGHTNESS_STEPS[brightness_idx]);
                config.lock().unwrap().set_brightness(brightness_idx as u8);
            }
        }
        button_was_down = down;

        // (Re)connect: ISO-14230 fast init, then startCommunication.
        if !kds.connected && last_reconnect.elapsed() >= RECONNECT_INTERVAL {
            last_reconnect = Instant::now();
            if !kds.start_communication() {
                warn!("KDS: init failed, retrying in {RECONNECT_INTERVAL:?}");
            }
        }

        // Deep scan: identification records + 16-bit common identifiers, then
        // change-watch the found commons (shift gears and look for a flip).
        if kds.connected && DEEP_SCAN {
            match deep_watch.as_mut() {
                None => {
                    info!("DEEP: scanning ECU identification 00..FF (service 1A)...");
                    for id in 0x00..=0xFFu8 {
                        if let Some(d) = kds.read_ident(id) {
                            info!("DEEP: ident {id:02X} = {d:02X?}");
                        }
                        if !kds.connected {
                            break;
                        }
                    }
                    info!("DEEP: scanning common ids 0000..0FFF (service 22, ~10 min)...");
                    let mut found: Vec<(u16, Vec<u8>)> = Vec::new();
                    for id in 0x0000..=0x0FFFu16 {
                        if let Some(d) = kds.read_common(id, true) {
                            info!("DEEP: common {id:04X} = {d:02X?}");
                            found.push((id, d));
                        }
                        if id & 0xFF == 0xFF {
                            info!("DEEP: ... {id:04X}/0FFF");
                        }
                        if !kds.connected {
                            break;
                        }
                    }
                    info!(
                        "DEEP: scan done, {} common ids — now shift N<->1st and watch for changes",
                        found.len()
                    );
                    deep_watch = Some(found);
                }
                Some(table) => {
                    for (id, prev) in table.iter_mut() {
                        if let Some(now) = kds.read_common(*id, true) {
                            if now != *prev {
                                info!("DEEP: common {id:04X} CHANGED {prev:02X?} -> {now:02X?}");
                                *prev = now;
                            }
                        }
                    }
                }
            }
        }

        // Poll live data.
        let (mut rpm, mut speed, gear_reg) = (None, None, None);
        let mut link_up = false;
        if kds.connected && DIAG_SCAN {
            match diag_regs.as_mut() {
                None => {
                    info!("KDS diag: scanning local identifiers 00..FF (takes ~30s)...");
                    let table = kds.scan_registers();
                    info!("KDS diag: scan done, {} registers supported — now shift through the gears and watch for changes", table.len());
                    diag_regs = Some(table);
                    link_up = true;
                }
                Some(table) => {
                    let mut alive = false;
                    for (reg, prev) in table.iter_mut() {
                        if let Some(now) = kds.read_register(*reg) {
                            alive = true;
                            if now != *prev {
                                info!("KDS diag: reg {reg:02X} CHANGED {prev:02X?} -> {now:02X?}");
                                *prev = now;
                            }
                        }
                    }
                    if !alive {
                        warn!("KDS: no data — dropping link for re-init");
                        kds.connected = false;
                    }
                    link_up = kds.connected;
                }
            }
        }

        // rpm for classification/learning may be time-aligned; publish raw.
        let mut rpm_aligned = None;
        if kds.connected && !DIAG_SCAN {
            let brightness = BRIGHTNESS_STEPS[brightness_idx];
            rpm = kds.read_rpm();
            let t_rpm = Instant::now();
            neutral_tick(
                neutral.is_low(),
                &mut neutral_was_active,
                &mut estimator,
                &led_tx,
                true,
                brightness,
            );
            speed = kds.read_speed();
            // Time-align rpm to the moment the speed byte was captured: the
            // reads are ~130 ms apart, which skews the ratio under acceleration.
            rpm_aligned = rpm;
            if let (Some(r), Some((pr, pt))) = (rpm, prev_rpm) {
                if speed.is_some() {
                    let dt_ms = (t_rpm - pt).as_millis() as f32;
                    let lead_ms = t_rpm.elapsed().as_millis() as f32;
                    rpm_aligned = Some(w230_core::gear::time_align_rpm(r, pr, dt_ms, lead_ms));
                }
            }
            if let Some(r) = rpm {
                prev_rpm = Some((r, t_rpm));
            }
            neutral_tick(
                neutral.is_low(),
                &mut neutral_was_active,
                &mut estimator,
                &led_tx,
                true,
                brightness,
            );
            if cycle % INTERLOCK_EVERY_N_CYCLES == 0 {
                match kds.read_interlock() {
                    Ok(Some(v)) => interlock = Some(v),
                    // A failed/refused read means UNKNOWN — never hold a stale
                    // "neutral+clutch" latch (the ECU refuses reg 0x03 while
                    // moving; a launch-time latch once blocked a whole ride).
                    Ok(None) => interlock = None,
                    Err(raw) => {
                        learner.note_interlock_odd(raw);
                        interlock = None;
                    }
                }
            }
            // rpm and speed are read every cycle; both failing = link is dead.
            if rpm.is_none() && speed.is_none() {
                warn!("KDS: no data — dropping link for re-init");
                kds.connected = false;
                interlock = None;
                prev_rpm = None;
                learner.note_link_drop();
                learner.save(); // key-off is how rides end — don't lose the tail
                events.push(uptime_s, "k-line dropped");
            } else {
                link_up = true;
                // Learning gate: the G23 neutral wire ONLY. Reg 0x03 reads
                // 00 00 whenever moving, so any gating role for it blocks all
                // ride samples; shift transients are absorbed by the
                // histogram's peak-mass filters.
                let neutral_now = neutral.is_low();
                if let (Some(r), Some(s)) = (rpm_aligned, speed) {
                    learner.add_sample(r, s, false, neutral_now);
                }
            }
        }

        // Periodically persist the histogram and re-derive the bands, so a
        // calibration ride completes without any power cycle. Only re-derive
        // when new data was actually written — keeps the bench log quiet.
        if last_learn_save.elapsed() >= LEARN_SAVE_INTERVAL {
            last_learn_save = Instant::now();
            if learner.save() {
                learn_flash_until = Instant::now() + Duration::from_millis(800);
                if let Some((bands, n)) = learner.derive_bands(&w230_core::gear::FACTORY_BANDS) {
                    estimator.set_bands(bands, w230_core::gear::NUM_GEARS);
                    learned_count = n;
                }
            }
        }

        if estimator.bands().is_none() {
            learned_count = 0;
        }

        // Neutral: the G23 switch wire only (LOW = neutral).
        let pin_low = neutral.is_low();
        if pin_low != neutral_was_active {
            info!(
                "NEUTRAL PIN: {}",
                if pin_low {
                    "LOW (neutral)"
                } else {
                    "HIGH (in gear)"
                }
            );
            neutral_was_active = pin_low;
        }
        let neutral_active = pin_low;

        let mut gear = estimator.update(&GearInputs {
            rpm: rpm_aligned,
            speed,
            gear_reg,
            neutral_switch: neutral_active,
            clutch_pulled: false,
        });
        if DEMO_MODE {
            let g = (demo_start.elapsed().as_secs() / DEMO_STEP.as_secs()) % 6 + 1;
            let g = g as u8;
            if g != demo_last_gear {
                info!("DEMO: showing gear {g}");
                demo_last_gear = g;
            }
            gear = Gear::G(g);
            link_up = true; // render the digit, not the all-red no-link screen
        }
        let brightness = if DEMO_MODE {
            255 // demo always at full brightness
        } else {
            BRIGHTNESS_STEPS[brightness_idx]
        };
        let frame = display::render(
            gear,
            link_up,
            brightness,
            Instant::now() < learn_flash_until,
        );
        // Rewrite on change, plus a periodic refresh as glitch insurance.
        if last_frame != Some(frame) || cycle % 8 == 0 {
            let _ = led_tx.try_send(frame);
            last_frame = Some(frame);
        }

        // Publish to the WiFi dashboard; act on a requested calibration wipe.
        if let Some(webdiag) = &webdiag {
            if webdiag
                .clear_req
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                learner.clear();
                estimator.clear_bands();
                info!("LEARN: calibration wiped (web request)");
            }
            let mut s = webdiag.shared.lock().unwrap();
            s.link_up = link_up;
            s.gear = Some(gear);
            s.rpm = rpm;
            s.speed = speed;
            s.ecu_neutral = interlock;
            s.samples = learner.samples();
            s.bands = estimator.bands().map(|b| b.to_vec()).unwrap_or_default();
            s.hist.clear();
            s.hist.extend_from_slice(learner.hist());
        }

        // --- BLE telemetry ---
        if link_up != link_was_up {
            link_was_up = link_up;
            events.push(uptime_s, if link_up { "k-line up" } else { "k-line down" });
        }
        if let Some(b) = &ble {
            if let Some(o) = &ota {
                o.shared.lock().unwrap().moving = speed.is_some_and(|s| s >= 1.0);
            }
            if b.live_subscribed() {
                let live = LiveStatus {
                    link_up,
                    neutral_switch: neutral_active,
                    interlock,
                    learn_flash: Instant::now() < learn_flash_until,
                    ota_busy: ota_status.state.is_busy(),
                    demo: DEMO_MODE,
                    gear,
                    brightness_idx: brightness_idx as u8,
                    rpm,
                    speed,
                    samples: learner.samples(),
                    uptime_s,
                };
                b.notify(Attr::Live, &live.encode());
            }
            if b.connections() > 0 && last_slow_publish.elapsed() >= BLE_SLOW_PUBLISH {
                last_slow_publish = Instant::now();
                let cfg = config.lock().unwrap();
                let (slot, pending, _) = boot_slot_info.clone().unwrap_or_default();
                let mac = mac_string();
                let info = proto::DeviceInfo {
                    fw_version: FW_VERSION,
                    project: w230_core::ota_manifest::PROJECT_NAME,
                    idf_version: idf_version(),
                    built: BUILD_STAMP,
                    hardware: HARDWARE,
                    slot: if slot.is_empty() { "factory" } else { &slot },
                    ota_capable: ota.is_some() && slot.starts_with("ota_"),
                    pending_verify: pending && !self_test_done,
                    boot_policy: cfg.cfg.boot_policy,
                    uptime_s,
                    free_heap: unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
                    min_free_heap: learner.min_free_heap(),
                    reset_reason: learner.reset_reason(),
                    mac: &mac,
                    ble_connections: b.connections(),
                };
                let settings = proto::Settings {
                    brightness_idx: brightness_idx as u8,
                    brightness_steps: &BRIGHTNESS_STEPS,
                    boot_policy: cfg.cfg.boot_policy,
                    manifest_url: cfg.cfg.manifest_url.as_deref().unwrap_or(OTA_MANIFEST_URL),
                    manifest_url_is_default: cfg.cfg.manifest_url.is_none(),
                    wifi_ssid: cfg.cfg.ssid.as_deref(),
                };
                published.publish_if_changed(b, Attr::DeviceInfo, proto::device_info_json(&info));
                published.publish_if_changed(b, Attr::Settings, proto::settings_json(&settings));
                published.publish_if_changed(
                    b,
                    Attr::BlackBox,
                    proto::black_box_json(&learner.black_box()),
                );
                published.publish_if_changed(
                    b,
                    Attr::Calibration,
                    proto::calibration_json(
                        estimator.bands(),
                        learned_count,
                        learner.samples(),
                        &learner.peaks(),
                    ),
                );
                published.publish_if_changed(b, Attr::Events, events.to_json());
                published.publish_if_changed(
                    b,
                    Attr::OtaStatus,
                    proto::ota_status_json(&ota_status),
                );
                published.publish_if_changed(
                    b,
                    Attr::WifiStatus,
                    proto::wifi_status_json(&wifi_status),
                );
                // Histogram pages are cheap to compare as bytes.
                let h0 = proto::hist_page(learner.hist(), learner.samples(), 0);
                let h1 = proto::hist_page(learner.hist(), learner.samples(), 1);
                published.publish_bytes_if_changed(b, Attr::Hist0, h0);
                published.publish_bytes_if_changed(b, Attr::Hist1, h1);
            }
        }

        cycle = cycle.wrapping_add(1);
        FreeRtos::delay_ms(POLL_INTERVAL.as_millis() as u32);
    }
}

/// Compile-time build stamp for the device-info attribute.
const BUILD_STAMP: &str = match option_env!("W230_BUILD_STAMP") {
    Some(s) => s,
    None => "dev",
};

fn idf_version() -> &'static str {
    // e.g. "v5.3.3" — from the linked ESP-IDF, not the crate.
    static VER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VER.get_or_init(|| unsafe {
        let p = esp_idf_svc::sys::esp_get_idf_version();
        if p.is_null() {
            "unknown".into()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    })
}

fn mac_string() -> String {
    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_BT,
        );
    }
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Last value pushed per attribute, so unchanged JSON isn't rewritten into
/// the GATT table every second.
#[derive(Default)]
struct PublishedCache {
    strings: std::collections::HashMap<u8, String>,
    bytes: std::collections::HashMap<u8, Vec<u8>>,
}

impl PublishedCache {
    fn publish_if_changed(&mut self, ble: &Ble, attr: Attr, value: String) {
        let key = attr as u8;
        if self.strings.get(&key) != Some(&value) {
            ble.publish(attr, value.as_bytes());
            self.strings.insert(key, value);
        }
    }

    fn publish_bytes_if_changed(&mut self, ble: &Ble, attr: Attr, value: Vec<u8>) {
        let key = attr as u8;
        if self.bytes.get(&key) != Some(&value) {
            ble.publish(attr, &value);
            self.bytes.insert(key, value);
        }
    }
}

/// Drain app commands and WiFi credentials from the BLE channels.
#[allow(clippy::too_many_arguments)]
fn handle_ble_commands(
    ble: &Option<Ble>,
    ota: &Option<OtaHandle>,
    config: &Arc<Mutex<ConfigStore>>,
    learner: &mut RatioLearner,
    estimator: &mut GearEstimator,
    brightness_idx: &mut usize,
    events: &mut EventRing,
    uptime_s: u32,
) {
    let Some(b) = ble else { return };
    while let Ok(creds) = b.wifi_credentials.try_recv() {
        config.lock().unwrap().set_wifi(&creds.ssid, &creds.psk);
        events.push(uptime_s, format!("wifi set: {}", creds.ssid));
        if let Some(o) = ota {
            o.request(OtaRequest::TestWifi);
        }
    }
    while let Ok(cmd) = b.commands.try_recv() {
        match cmd {
            Command::WipeCalibration => {
                learner.clear();
                estimator.clear_bands();
                info!("LEARN: calibration wiped (app)");
                events.push(uptime_s, "calibration wiped (app)");
            }
            Command::SetBrightness(i) => {
                let i = (i as usize).min(BRIGHTNESS_STEPS.len() - 1);
                *brightness_idx = i;
                config.lock().unwrap().set_brightness(i as u8);
                info!("brightness -> {} (app)", BRIGHTNESS_STEPS[i]);
            }
            Command::CheckUpdate => {
                events.push(uptime_s, "app: check update");
                match ota {
                    Some(o) => o.request(OtaRequest::Check { install: false }),
                    None => warn!("OTA: unavailable (WIFI_DIAG build or worker failed)"),
                }
            }
            Command::InstallUpdate => {
                events.push(uptime_s, "app: install update");
                if let Some(o) = ota {
                    o.request(OtaRequest::Install);
                }
            }
            Command::SetBootPolicy(p) => {
                config.lock().unwrap().set_boot_policy(p);
                events.push(uptime_s, format!("boot policy {p}"));
            }
            Command::Reboot => {
                info!("reboot requested by app");
                learner.save();
                FreeRtos::delay_ms(300);
                esp_idf_hal::reset::restart();
            }
            Command::ForgetWifi => {
                config.lock().unwrap().clear_wifi();
                events.push(uptime_s, "wifi forgotten");
                if let Some(o) = ota {
                    let mut sh = o.shared.lock().unwrap();
                    sh.wifi = WifiStatus::default();
                    sh.changed = true;
                }
            }
            Command::ClearBlackBox => {
                learner.clear_black_box();
                events.push(uptime_s, "black box cleared");
            }
            Command::SetManifestUrl(url) => {
                config.lock().unwrap().set_manifest_url(if url.is_empty() {
                    None
                } else {
                    Some(&url)
                });
                events.push(
                    uptime_s,
                    if url.is_empty() {
                        "manifest url: default"
                    } else {
                        "manifest url: custom"
                    },
                );
            }
            Command::TestWifi => {
                if let Some(o) = ota {
                    o.request(OtaRequest::TestWifi);
                }
            }
        }
    }
}
