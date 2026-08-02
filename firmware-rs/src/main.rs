//! Kawasaki W230 gear indicator — M5Stack ATOM Matrix (ESP32-PICO-D4).
//!
//! Reads the KDS diagnostic K-line through a LINTTL3 (TJA1021/SIT1021T)
//! TTL-UART<->LIN module and shows the current gear on the 5x5 LED matrix:
//! green N, cyan 1-5, dim red dash when unknown, red top-left dot = no link.
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

mod display;
mod gear;
mod kds;

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{PinDriver, Pull};
use esp_idf_hal::peripheral::Peripheral;
use esp_idf_hal::prelude::*;
use esp_idf_hal::uart::{config::Config as UartConfig, UartDriver};
use gear::{GearEstimator, DEFAULT_BANDS};
use kds::Kds;
use log::{info, warn};
use smart_leds::SmartLedsWrite;
use std::time::{Duration, Instant};
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const RECONNECT_INTERVAL: Duration = Duration::from_millis(1000);
const BRIGHTNESS_STEPS: [u8; 3] = [40, 120, 255];

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("W230 gear indicator starting (ATOM Matrix + LINTTL3/TJA1021)");

    let p = Peripherals::take()?;

    // --- LED matrix: 25x WS2812 on GPIO27 ---
    let mut leds = Ws2812Esp32Rmt::new(p.rmt.channel0, p.pins.gpio27)?;

    // --- Button (GPIO39, active low, external pull-up on board) ---
    let button = PinDriver::input(p.pins.gpio39)?;

    // --- Neutral switch input (G19, bottom header): grounds when in neutral ---
    let mut neutral = PinDriver::input(p.pins.gpio19)?;
    neutral.set_pull(Pull::Up)?;

    // --- TJA1021 SLP (G22): drive high = normal mode (not sleep) ---
    let mut slp = PinDriver::output(p.pins.gpio22)?;
    slp.set_high()?;
    info!("TJA1021 SLP driven high (normal mode)");

    // --- K-line UART pins (Grove port): G26 = ESP32 TX, G32 = ESP32 RX ---
    let mut uart1 = p.uart1;
    let mut tx_pin = p.pins.gpio26;
    let mut rx_pin = p.pins.gpio32;

    let mut estimator = GearEstimator::new(DEFAULT_BANDS);
    let mut kds_link: Option<Kds> = None;
    let mut last_reconnect = Instant::now() - RECONNECT_INTERVAL;
    let mut brightness_idx: usize = 1;
    let mut button_was_down = false;

    loop {
        // Button: cycle brightness on press.
        let down = button.is_low();
        if down && !button_was_down {
            brightness_idx = (brightness_idx + 1) % BRIGHTNESS_STEPS.len();
            info!("brightness -> {}", BRIGHTNESS_STEPS[brightness_idx]);
        }
        button_was_down = down;

        // (Re)connect: ISO-14230 fast init, then startCommunication.
        if kds_link.is_none() && last_reconnect.elapsed() >= RECONNECT_INTERVAL {
            last_reconnect = Instant::now();
            drop(kds_link.take()); // release UART pins before bit-banging TX

            info!("KDS: fast-init pulse (300ms high, 25ms low, 25ms high)");
            {
                let mut tx = PinDriver::output(unsafe { tx_pin.clone_unchecked() })?;
                tx.set_high()?;
                FreeRtos::delay_ms(300);
                tx.set_low()?;
                FreeRtos::delay_ms(25);
                tx.set_high()?;
                FreeRtos::delay_ms(25);
            } // drop PinDriver so the UART can claim the pin

            let uart = UartDriver::new(
                unsafe { uart1.clone_unchecked() },
                unsafe { tx_pin.clone_unchecked() },
                unsafe { rx_pin.clone_unchecked() },
                Option::<esp_idf_hal::gpio::Gpio0>::None,
                Option::<esp_idf_hal::gpio::Gpio0>::None,
                &UartConfig::new().baudrate(Hertz(10_400)),
            )?;
            let mut link = Kds::new(uart);
            if link.start_communication() {
                kds_link = Some(link);
            } else {
                warn!("KDS: init failed, retrying in {RECONNECT_INTERVAL:?}");
            }
        }

        // Poll live data.
        let (mut rpm, mut speed, mut gear_reg) = (None, None, None);
        let mut link_up = false;
        if let Some(link) = kds_link.as_mut() {
            rpm = link.read_rpm();
            speed = link.read_speed();
            gear_reg = link.read_gear_raw();
            // Losing all three in one cycle = link is dead; force re-init.
            if rpm.is_none() && speed.is_none() && gear_reg.is_none() {
                warn!("KDS: no data — dropping link for re-init");
                kds_link = None;
            } else {
                link_up = true;
            }
        }

        // Neutral switch: LOW = neutral.
        let neutral_active = neutral.is_low();

        let gear = estimator.update(rpm, speed, gear_reg, neutral_active);
        let frame = display::render(gear, link_up, BRIGHTNESS_STEPS[brightness_idx]);
        if let Err(e) = leds.write(frame.into_iter()) {
            warn!("LED write failed: {e}");
        }

        FreeRtos::delay_ms(POLL_INTERVAL.as_millis() as u32);
    }
}
