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
use esp_idf_hal::units::Hertz;
use log::{info, warn};
use std::time::Duration;

// KWP2000 addressing
pub const ECU_ADDR: u8 = 0x11; // target (ECU)
pub const TESTER_ADDR: u8 = 0xF2; // source (this module)

// Service IDs
pub const SVC_START: u8 = 0x81; // startCommunication
pub const SVC_START_OK: u8 = 0xC1; // positive response
pub const SVC_SESSION: u8 = 0x10; // startDiagnosticSession
pub const SVC_SESSION_KDS: u8 = 0x80; // session type used by the KDS tool
pub const SVC_SESSION_OK: u8 = 0x50; // positive response
pub const SVC_READ: u8 = 0x21; // readDataByLocalIdentifier
pub const SVC_READ_OK: u8 = 0x61; // positive response
pub const SVC_READ_COMMON: u8 = 0x22; // readDataByCommonIdentifier (16-bit id)
pub const SVC_READ_COMMON_OK: u8 = 0x62; // positive response
pub const SVC_READ_IDENT: u8 = 0x1A; // readEcuIdentification
pub const SVC_READ_IDENT_OK: u8 = 0x5A; // positive response

// Local identifiers (registers) — verified live on a 2024 W230 (2026-08-02):
// a full 0x00-0xFF scan found no gear-number and no neutral register (0x0B
// answers 7F/12; neutral must come from the switch wire on GPIO19). 0x03
// initially looked like a neutral flag but a clutch-hold test proved it is
// the CLUTCH switch. 0x0A is battery volts, 0x04-0x08 look like sensor temps.
pub const REG_RPM: u8 = 0x09; // 2 bytes: hi*100 + lo
pub const REG_SPEED: u8 = 0x0C; // 1 byte on the W230 (2 on other models)
pub const REG_CLUTCH: u8 = 0x03; // 2 bytes: 00 00 = lever pulled, FF FF = released

const RSP_TIMEOUT: Duration = Duration::from_millis(250);
const TX_BYTE_GAP: Duration = Duration::from_millis(5);
/// ISO 14230 P3min: quiet time the ECU needs between its response and our next
/// request. Requests sent sooner are silently ignored (seen on the W230).
const REQUEST_GAP: Duration = Duration::from_millis(60);

const KDS_BAUD: u32 = 10_400;
/// Sending 0x00 at this baud holds the line low for 9 bit times = 25 ms —
/// the ISO 14230 fast-init low pulse, without giving up the pin to a GPIO
/// driver (which cost ~15 ms of setup before the request could start).
const BREAK_BAUD: u32 = 360;
const FAST_INIT_IDLE: Duration = Duration::from_millis(300);

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

    /// ISO 14230 fast init + startCommunication handshake.
    /// Returns true when the ECU ACKs (0xC1).
    pub fn start_communication(&mut self) -> bool {
        self.connected = false;
        info!("KDS: fast init (300ms idle, 25ms low via {BREAK_BAUD}-baud break, 25ms high)");
        self.drain_rx();
        std::thread::sleep(FAST_INIT_IDLE);

        // 25 ms low pulse: one 0x00 frame at the break baud rate.
        if self.uart.change_baudrate(Hertz(BREAK_BAUD)).is_err()
            || self.uart.write(&[0x00]).is_err()
        {
            warn!("KDS: fast-init break failed");
            let _ = self.uart.change_baudrate(Hertz(KDS_BAUD));
            return false;
        }
        self.uart.wait_tx_done(TickType::from(RSP_TIMEOUT).ticks()).ok();
        if self.uart.change_baudrate(Hertz(KDS_BAUD)).is_err() {
            warn!("KDS: could not restore {KDS_BAUD} baud");
            return false;
        }
        // Stop bit of the break frame already gave ~2.8 ms high; top up to 25 ms,
        // then the request must follow immediately. The echoed break byte only
        // reaches the driver's ring buffer after the UART idle timeout (~10 ms
        // at 10400 baud), so drain it at the END of this window — draining
        // right after wait_tx_done misses it and misaligns the echo read.
        std::thread::sleep(Duration::from_millis(22));
        self.drain_rx();

        if self.send_request(&[SVC_START]).is_err() {
            return false;
        }
        match self.read_response() {
            Some(payload) if payload.contains(&SVC_START_OK) => {
                info!("KDS: ECU acknowledged startCommunication (0xC1)");
            }
            Some(payload) => {
                warn!("KDS: unexpected startCommunication reply: {}", hex(&payload));
                return false;
            }
            None => {
                warn!("KDS: no reply to startCommunication");
                return false;
            }
        }

        // The KDS tool follows up with startDiagnosticSession (10 80); without
        // it every readDataByLocalIdentifier gets 7F 21 22 (conditionsNotCorrect).
        std::thread::sleep(REQUEST_GAP);
        if self.send_request(&[SVC_SESSION, SVC_SESSION_KDS]).is_err() {
            return false;
        }
        match self.read_response() {
            Some(payload) if payload.contains(&SVC_SESSION_OK) => {
                info!("KDS: diagnostic session started (0x50)");
                self.connected = true;
                true
            }
            Some(payload) => {
                warn!("KDS: unexpected startDiagnosticSession reply: {}", hex(&payload));
                false
            }
            None => {
                warn!("KDS: no reply to startDiagnosticSession");
                false
            }
        }
    }

    /// Read one register via service 0x21. Returns the data bytes.
    pub fn read_register(&mut self, reg: u8) -> Option<Vec<u8>> {
        self.read_register_impl(reg, false)
    }

    fn read_register_impl(&mut self, reg: u8, quiet: bool) -> Option<Vec<u8>> {
        if !self.connected {
            return None;
        }
        std::thread::sleep(REQUEST_GAP); // respect P3min or the ECU ignores us
        self.send_request(&[SVC_READ, reg]).ok()?;
        let payload = self.read_response()?;
        // Expect [0x61][reg][data...] (some ECUs omit the echoed reg)
        if payload.first() != Some(&SVC_READ_OK) {
            if !quiet {
                warn!("KDS: reg {reg:02X} negative/unknown reply: {}", hex(&payload));
            }
            return None;
        }
        let data_off = if payload.get(1) == Some(&reg) { 2 } else { 1 };
        Some(payload[data_off..].to_vec())
    }

    /// Read one 16-bit common identifier via service 0x22. Quiet on negatives.
    pub fn read_common(&mut self, id: u16, quiet: bool) -> Option<Vec<u8>> {
        if !self.connected {
            return None;
        }
        std::thread::sleep(REQUEST_GAP);
        self.send_request(&[SVC_READ_COMMON, (id >> 8) as u8, id as u8])
            .ok()?;
        let payload = self.read_response()?;
        if payload.first() != Some(&SVC_READ_COMMON_OK) {
            if !quiet {
                warn!("KDS: common {id:04X} negative/unknown reply: {}", hex(&payload));
            }
            return None;
        }
        let off = if payload.len() >= 3 && payload[1] == (id >> 8) as u8 && payload[2] == id as u8
        {
            3
        } else {
            1
        };
        Some(payload[off..].to_vec())
    }

    /// Read one ECU identification record via service 0x1A. Quiet on negatives.
    pub fn read_ident(&mut self, id: u8) -> Option<Vec<u8>> {
        if !self.connected {
            return None;
        }
        std::thread::sleep(REQUEST_GAP);
        self.send_request(&[SVC_READ_IDENT, id]).ok()?;
        let payload = self.read_response()?;
        if payload.first() != Some(&SVC_READ_IDENT_OK) {
            return None;
        }
        let off = if payload.get(1) == Some(&id) { 2 } else { 1 };
        Some(payload[off..].to_vec())
    }

    /// One-shot probe of every local identifier (read-only service 0x21).
    /// Returns the registers that answered positively, with their data.
    pub fn scan_registers(&mut self) -> Vec<(u8, Vec<u8>)> {
        let mut found = Vec::new();
        for reg in 0x00..=0xFFu8 {
            if let Some(d) = self.read_register_impl(reg, true) {
                info!("KDS scan: reg {reg:02X} supported = {d:02X?}");
                found.push((reg, d));
            }
            if reg & 0x1F == 0x1F {
                info!("KDS scan: ... {:02X}/FF", reg);
            }
        }
        found
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
        // The W230 answers with a single byte; other Kawasakis use two ((hi<<8|lo)/2).
        let speed = match d.len() {
            0 => return None,
            1 => d[0] as f32,
            _ => ((d[0] as u16) << 8 | d[1] as u16) as f32 / 2.0,
        };
        info!("KDS: speed = {speed:.1}");
        Some(speed)
    }

    /// Clutch switch from reg 0x03: `Some(true)` = lever pulled, `Some(false)`
    /// = released, `None` = no/odd reply.
    pub fn read_clutch(&mut self) -> Option<bool> {
        let d = self.read_register(REG_CLUTCH)?;
        match d.as_slice() {
            [0x00, 0x00] => Some(true),
            [0xFF, 0xFF] => Some(false),
            other => {
                warn!("KDS: unexpected clutch reg value {other:02X?}");
                None
            }
        }
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

        // Log AFTER transmitting: a blocking console write here would delay the
        // first byte past the ECU's post-fast-init window.
        for b in &frame {
            self.uart.write(&[*b]).map_err(|e| {
                warn!("KDS: UART write error: {e}");
            })?;
            self.uart.wait_tx_done(TickType::from(RSP_TIMEOUT).ticks()).ok();
            std::thread::sleep(TX_BYTE_GAP);
        }
        info!("KDS TX >> {}", hex(&frame));

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
        // A frame of nothing but 0x00 means the UART is sampling a line that
        // is stuck dominant/low: transceiver unpowered, or K-line shorted to
        // ground. A real ECU frame always has a non-zero format byte.
        if raw.iter().all(|&b| b == 0) {
            warn!("KDS: RX held low (all 0x00) — transceiver unpowered or K-line shorted to ground?");
            return None;
        }
        Some(payload)
    }

    /// Discard anything sitting in the RX FIFO (stale echoes, noise).
    fn drain_rx(&mut self) {
        let mut b = [0u8; 16];
        while matches!(self.uart.read(&mut b, 0), Ok(n) if n > 0) {}
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
