//! Minimal, READ-ONLY Kawasaki KDS (ISO-14230 / KWP2000) K-line client.
//!
//! Physical layer: single-wire K-line through a TJA1021 LIN transceiver module
//! on UART1 @ 10400 8N1. Single wire means every transmitted byte is echoed
//! back on RX; the driver reads and discards those echoes.
//!
//! Every frame sent and received (including echoes) is hex-dumped to the log —
//! watch with `espflash monitor` / `cargo run`.

use esp_idf_hal::delay::TickType;
use esp_idf_hal::uart::UartDriver;
use log::{info, warn};
use std::time::Duration;

// KWP2000 addressing
pub const ECU_ADDR: u8 = 0x11; // target (ECU)
pub const TESTER_ADDR: u8 = 0xF2; // source (this module)

// Service IDs
pub const SVC_START: u8 = 0x81; // startCommunication
pub const SVC_START_OK: u8 = 0xC1; // positive response
pub const SVC_READ: u8 = 0x21; // readDataByLocalIdentifier
pub const SVC_READ_OK: u8 = 0x61; // positive response

// Local identifiers (registers) — !!! VERIFY ON THE W230 (see docs/01 §4) !!!
pub const REG_RPM: u8 = 0x09; // 2 bytes: hi*100 + lo
pub const REG_SPEED: u8 = 0x0C; // 2 bytes: (hi<<8|lo)/2
pub const REG_GEAR: u8 = 0x0B; // 1 byte — may not exist on the W230

const RSP_TIMEOUT: Duration = Duration::from_millis(250);
const TX_BYTE_GAP: Duration = Duration::from_millis(5);

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct Kds<'d> {
    uart: UartDriver<'d>,
    pub connected: bool,
}

impl<'d> Kds<'d> {
    /// Wrap an already-opened 10400-baud UART (fast-init pulse must be done
    /// beforehand on the TX pin, as raw GPIO — see `main.rs`).
    pub fn new(uart: UartDriver<'d>) -> Self {
        Self {
            uart,
            connected: false,
        }
    }

    /// startCommunication handshake. Returns true when the ECU ACKs (0xC1).
    pub fn start_communication(&mut self) -> bool {
        info!("KDS: startCommunication (fast-init pulse already sent)");
        self.connected = false;
        if self.send_request(&[SVC_START]).is_err() {
            return false;
        }
        match self.read_response() {
            Some(payload) if payload.contains(&SVC_START_OK) => {
                info!("KDS: ECU acknowledged startCommunication (0xC1)");
                self.connected = true;
                true
            }
            Some(payload) => {
                warn!("KDS: unexpected startCommunication reply: {}", hex(&payload));
                false
            }
            None => {
                warn!("KDS: no reply to startCommunication");
                false
            }
        }
    }

    /// Read one register via service 0x21. Returns the data bytes.
    pub fn read_register(&mut self, reg: u8) -> Option<Vec<u8>> {
        if !self.connected {
            return None;
        }
        self.send_request(&[SVC_READ, reg]).ok()?;
        let payload = self.read_response()?;
        // Expect [0x61][reg][data...] (some ECUs omit the echoed reg)
        if payload.first() != Some(&SVC_READ_OK) {
            warn!("KDS: reg {reg:02X} negative/unknown reply: {}", hex(&payload));
            return None;
        }
        let data_off = if payload.get(1) == Some(&reg) { 2 } else { 1 };
        Some(payload[data_off..].to_vec())
    }

    pub fn read_rpm(&mut self) -> Option<f32> {
        let d = self.read_register(REG_RPM)?;
        if d.len() < 2 {
            return None;
        }
        let rpm = d[0] as f32 * 100.0 + d[1] as f32;
        info!("KDS: RPM = {rpm:.0}");
        Some(rpm)
    }

    pub fn read_speed(&mut self) -> Option<f32> {
        let d = self.read_register(REG_SPEED)?;
        if d.len() < 2 {
            return None;
        }
        let speed = ((d[0] as u16) << 8 | d[1] as u16) as f32 / 2.0;
        info!("KDS: speed = {speed:.1}");
        Some(speed)
    }

    pub fn read_gear_raw(&mut self) -> Option<u8> {
        let d = self.read_register(REG_GEAR)?;
        let g = *d.first()?;
        info!("KDS: gear register raw = {g}");
        Some(g)
    }

    // ---- framing ----

    /// Build [fmt][tgt][src][payload...][cs], send byte-by-byte, drain echo.
    fn send_request(&mut self, payload: &[u8]) -> Result<(), ()> {
        assert!(!payload.is_empty() && payload.len() <= 63);
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.push(0x80 | payload.len() as u8);
        frame.push(ECU_ADDR);
        frame.push(TESTER_ADDR);
        frame.extend_from_slice(payload);
        let cs = frame.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        frame.push(cs);

        info!("KDS TX >> {}", hex(&frame));

        for b in &frame {
            self.uart.write(&[*b]).map_err(|e| {
                warn!("KDS: UART write error: {e}");
            })?;
            self.uart.wait_tx_done(TickType::from(RSP_TIMEOUT).ticks()).ok();
            std::thread::sleep(TX_BYTE_GAP);
        }

        // Single-wire bus: our own bytes come back on RX. Read and log them.
        let mut echo = vec![0u8; frame.len()];
        let n = self.read_exact(&mut echo);
        info!("KDS RX << (echo) {}", hex(&echo[..n]));
        if n < frame.len() {
            warn!("KDS: short echo ({n}/{} bytes) — check wiring/pull-up", frame.len());
        }
        Ok(())
    }

    /// Read one response frame; log it raw; return the payload
    /// (bytes after fmt/tgt/src, before checksum).
    fn read_response(&mut self) -> Option<Vec<u8>> {
        let fmt = self.read_byte()?;
        let mut raw = vec![fmt];

        let payload_len = if fmt & 0xC0 == 0x80 && fmt & 0x3F != 0 {
            // length in low 6 bits of format byte; consume tgt + src
            raw.push(self.read_byte()?);
            raw.push(self.read_byte()?);
            (fmt & 0x3F) as usize
        } else {
            // 0x80 variant with a separate length byte after tgt/src
            raw.push(self.read_byte()?);
            raw.push(self.read_byte()?);
            let len = self.read_byte()?;
            raw.push(len);
            len as usize
        };

        let payload_start = raw.len();
        for _ in 0..payload_len {
            match self.read_byte() {
                Some(b) => raw.push(b),
                None => {
                    warn!("KDS RX << (truncated) {}", hex(&raw));
                    return None;
                }
            }
        }
        let payload = raw[payload_start..].to_vec();

        // checksum byte (verify + log, tolerate mismatch with a warning)
        if let Some(cs) = self.read_byte() {
            raw.push(cs);
            let want = raw[..raw.len() - 1]
                .iter()
                .fold(0u8, |a, b| a.wrapping_add(*b));
            if cs != want {
                warn!("KDS: checksum mismatch (got {cs:02X}, want {want:02X})");
            }
        }
        info!("KDS RX << {}", hex(&raw));
        Some(payload)
    }

    fn read_byte(&mut self) -> Option<u8> {
        let mut b = [0u8; 1];
        match self.uart.read(&mut b, TickType::from(RSP_TIMEOUT).ticks()) {
            Ok(1) => Some(b[0]),
            _ => None,
        }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> usize {
        let mut n = 0;
        while n < buf.len() {
            match self.read_byte() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        n
    }
}
