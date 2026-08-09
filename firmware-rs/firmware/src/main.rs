//! Kawasaki W230 gear indicator — M5Stack ATOM Matrix (ESP32-PICO-D4).
//!
//! Reads the KDS diagnostic K-line through a LINTTL3 (TJA1021/SIT1021T)
//! TTL-UART<->LIN module and shows the current gear on the 5x5 LED matrix:
//! green N, cyan 1-6, dim red dash when unknown, all red = no link.
//! The ECU exposes no gear or neutral register, so N comes from the bike's
//! neutral-switch wire on G23 (required) and gears 1-6 from the RPM/speed
//! ratio. ECU reg 0x03 is the clutch switch — it pauses ratio classification
//! while the lever is pulled.
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

mod kds;
mod learn_store;
mod web;

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{PinDriver, Pull};
use esp_idf_hal::prelude::*;
use esp_idf_hal::uart::{config::Config as UartConfig, UartDriver};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use kds::Kds;
use learn_store::RatioLearner;
use log::{info, warn};
use smart_leds::SmartLedsWrite;
use std::time::{Duration, Instant};
use w230_core::display;
use w230_core::gear::{Gear, GearEstimator, GearInputs};
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const RECONNECT_INTERVAL: Duration = Duration::from_millis(1000);
const BRIGHTNESS_STEPS: [u8; 3] = [40, 120, 255];
const LEARN_SAVE_INTERVAL: Duration = Duration::from_secs(30);
/// Button hold this long = wipe the learned calibration.
const LEARN_RESET_HOLD: Duration = Duration::from_secs(3);

/// Gear-register hunt: scan all local identifiers once after connecting, then
/// keep re-reading the supported ones and log every value change. Shift
/// through the gears and watch which register follows. (Used 2026-08-02 to
/// find the clutch switch in reg 0x03; no gear-number register exists.)
const DIAG_SCAN: bool = false;

/// Demo mode: override the display with gears 1..6, stepping every
/// `DEMO_STEP`. TEMPORARY — turn off for real use.
const DEMO_MODE: bool = false;
const DEMO_STEP: Duration = Duration::from_secs(15);

/// Deep scan: probe services 0x1A (ECU identification) and 0x22 (common
/// identifiers 0x0000-0x0FFF, ~10 min), then change-watch everything found.
/// (Run 2026-08-02: service 0x22 absent entirely; 0x1A yields ID strings only
/// — confirmed no neutral/gear data exists beyond the 59 0x21 registers.)
const DEEP_SCAN: bool = false;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("W230 gear indicator starting (ATOM Matrix + LINTTL3/TJA1021)");

    let p = Peripherals::take()?;

    // --- LED matrix: 25x WS2812 on GPIO27 ---
    let mut leds = Ws2812Esp32Rmt::new(p.rmt.channel0, p.pins.gpio27)?;

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

    // --- WiFi hotspot + HTTP diagnostic dashboard ---
    let sysloop = EspSystemEventLoop::take()?;
    let webdiag = web::start(p.modem, sysloop, nvs_part)?;

    let mut estimator = GearEstimator::new();
    if let Some(bands) = learner.derive_bands() {
        estimator.set_bands(bands);
    }

    let demo_start = Instant::now();
    let mut demo_last_gear: u8 = 0;
    let mut last_reconnect = Instant::now() - RECONNECT_INTERVAL;
    let mut last_learn_save = Instant::now();
    let mut diag_regs: Option<Vec<(u8, Vec<u8>)>> = None;
    let mut deep_watch: Option<Vec<(u16, Vec<u8>)>> = None;
    let mut brightness_idx: usize = 1;
    let mut button_was_down = false;
    let mut button_down_at = Instant::now();
    let mut neutral_was_active = false;

    loop {
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
            } else {
                brightness_idx = (brightness_idx + 1) % BRIGHTNESS_STEPS.len();
                info!("brightness -> {}", BRIGHTNESS_STEPS[brightness_idx]);
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

        let mut clutch = None;
        if kds.connected && !DIAG_SCAN {
            rpm = kds.read_rpm();
            speed = kds.read_speed();
            clutch = kds.read_clutch();
            // Losing all three in one cycle = link is dead; force re-init.
            if rpm.is_none() && speed.is_none() && clutch.is_none() {
                warn!("KDS: no data — dropping link for re-init");
                kds.connected = false;
                learner.save(); // key-off is how rides end — don't lose the tail
            } else {
                link_up = true;
                if let (Some(r), Some(s)) = (rpm, speed) {
                    // Unknown clutch counts as pulled: never learn from it.
                    learner.add_sample(r, s, clutch.unwrap_or(true));
                }
            }
        }

        // Periodically persist the histogram and re-derive the bands, so a
        // calibration ride completes without any power cycle. Only re-derive
        // when new data was actually written — keeps the bench log quiet.
        if last_learn_save.elapsed() >= LEARN_SAVE_INTERVAL {
            last_learn_save = Instant::now();
            if learner.save() {
                if let Some(bands) = learner.derive_bands() {
                    estimator.set_bands(bands);
                }
            }
        }

        // Neutral switch on G23: LOW = neutral.
        let neutral_active = neutral.is_low();
        if neutral_active != neutral_was_active {
            info!("NEUTRAL PIN: {}", if neutral_active { "LOW (neutral)" } else { "HIGH (in gear)" });
            neutral_was_active = neutral_active;
        }

        let mut gear = estimator.update(&GearInputs {
            rpm,
            speed,
            gear_reg,
            neutral_switch: neutral_active,
            clutch_pulled: clutch.unwrap_or(false),
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
        let frame = display::render(gear, link_up, brightness);
        if let Err(e) = leds.write(frame.into_iter()) {
            warn!("LED write failed: {e}");
        }

        // Publish to the WiFi dashboard; act on a requested calibration wipe.
        if webdiag.clear_req.swap(false, std::sync::atomic::Ordering::Relaxed) {
            learner.clear();
            estimator.clear_bands();
            info!("LEARN: calibration wiped (web request)");
        }
        {
            let mut s = webdiag.shared.lock().unwrap();
            s.link_up = link_up;
            s.gear = Some(gear);
            s.rpm = rpm;
            s.speed = speed;
            s.clutch = clutch;
            s.samples = learner.samples();
            s.bands = estimator.bands();
            s.hist.clear();
            s.hist.extend_from_slice(learner.hist());
        }

        FreeRtos::delay_ms(POLL_INTERVAL.as_millis() as u32);
    }
}
