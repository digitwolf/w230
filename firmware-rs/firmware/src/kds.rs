//! KDS K-line transport: UART I/O, ISO-14230 fast init, request/response
//! timing. Framing, parsing and value decoding live in `w230_core::kds_proto`.
//!
//! Physical layer: single-wire K-line through a TJA1021 LIN transceiver module
//! on UART1 @ 10400 8N1. Single wire means every transmitted byte is echoed
//! back on RX; this driver reads and discards those echoes.
//!
//! Every frame sent and received (including echoes) is hex-dumped to the log —
//! watch with `espflash monitor` / `cargo run`.

use esp_idf_hal::delay::TickType;
use esp_idf_hal::uart::UartDriver;
use esp_idf_hal::units::Hertz;
use log::{info, warn};
use std::time::Duration;
use w230_core::kds_proto as proto;
use w230_core::kds_proto::{
    ParseError, REG_CLUTCH, REG_RPM, REG_SPEED, SVC_READ, SVC_READ_COMMON, SVC_READ_COMMON_OK,
    SVC_READ_IDENT, SVC_READ_IDENT_OK, SVC_READ_OK, SVC_SESSION, SVC_SESSION_KDS, SVC_SESSION_OK,
    SVC_START, SVC_START_OK,
};

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
    /// Wrap an already-opened 10400-baud UART.
    pub fn new(uart: UartDriver<'d>) -> Self {
        Self {
            uart,
            connected: false,
        }
    }

    /// ISO 14230 fast init + startCommunication + startDiagnosticSession.
    /// Returns true when the ECU completes both handshakes.
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
        self.uart
            .wait_tx_done(TickType::from(RSP_TIMEOUT).ticks())
            .ok();
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
                warn!(
                    "KDS: unexpected startDiagnosticSession reply: {}",
                    hex(&payload)
                );
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
        match proto::positive_data(&payload, SVC_READ_OK, &[reg]) {
            Some(data) => Some(data.to_vec()),
            None => {
                if !quiet {
                    warn!("KDS: reg {reg:02X} negative/unknown reply: {}", hex(&payload));
                }
                None
            }
        }
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
        match proto::positive_data(&payload, SVC_READ_COMMON_OK, &[(id >> 8) as u8, id as u8]) {
            Some(data) => Some(data.to_vec()),
            None => {
                if !quiet {
                    warn!("KDS: common {id:04X} negative/unknown reply: {}", hex(&payload));
                }
                None
            }
        }
    }

    /// Read one ECU identification record via service 0x1A. Quiet on negatives.
    pub fn read_ident(&mut self, id: u8) -> Option<Vec<u8>> {
        if !self.connected {
            return None;
        }
        std::thread::sleep(REQUEST_GAP);
        self.send_request(&[SVC_READ_IDENT, id]).ok()?;
        let payload = self.read_response()?;
        proto::positive_data(&payload, SVC_READ_IDENT_OK, &[id]).map(|d| d.to_vec())
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
        let rpm = proto::decode_rpm(&self.read_register(REG_RPM)?)?;
        info!("KDS: RPM = {rpm:.0}");
        Some(rpm)
    }

    pub fn read_speed(&mut self) -> Option<f32> {
        let speed = proto::decode_speed(&self.read_register(REG_SPEED)?)?;
        info!("KDS: speed = {speed:.1}");
        Some(speed)
    }

    /// Clutch switch from reg 0x03: `Some(true)` = lever pulled, `Some(false)`
    /// = released, `None` = no/odd reply.
    pub fn read_clutch(&mut self) -> Option<bool> {
        let data = self.read_register(REG_CLUTCH)?;
        let clutch = proto::decode_clutch(&data);
        if clutch.is_none() {
            warn!("KDS: unexpected clutch reg value {data:02X?}");
        }
        clutch
    }

    // ---- transport ----

    /// Frame the payload, send byte-by-byte, drain the single-wire echo.
    fn send_request(&mut self, payload: &[u8]) -> Result<(), ()> {
        let frame = proto::build_request(payload);

        // Log AFTER transmitting: a blocking console write here would delay the
        // first byte past the ECU's post-fast-init window.
        for b in &frame {
            self.uart.write(&[*b]).map_err(|e| {
                warn!("KDS: UART write error: {e}");
            })?;
            self.uart
                .wait_tx_done(TickType::from(RSP_TIMEOUT).ticks())
                .ok();
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
        let mut read = || self.read_byte_inner();
        match proto::parse_response(&mut read) {
            Ok(frame) => {
                if !frame.checksum_ok {
                    warn!("KDS: checksum mismatch in {}", hex(&frame.raw));
                }
                info!("KDS RX << {}", hex(&frame.raw));
                Some(frame.payload)
            }
            Err(ParseError::Timeout) => None,
            Err(ParseError::Truncated(raw)) => {
                warn!("KDS RX << (truncated) {}", hex(&raw));
                None
            }
            Err(ParseError::HeldLow(_)) => {
                warn!("KDS: RX held low (all 0x00) — transceiver unpowered or K-line shorted to ground?");
                None
            }
        }
    }

    /// Discard anything sitting in the RX FIFO (stale echoes, noise).
    fn drain_rx(&mut self) {
        let mut b = [0u8; 16];
        while matches!(self.uart.read(&mut b, 0), Ok(n) if n > 0) {}
    }

    fn read_byte_inner(&self) -> Option<u8> {
        let mut b = [0u8; 1];
        match self.uart.read(&mut b, TickType::from(RSP_TIMEOUT).ticks()) {
            Ok(1) => Some(b[0]),
            _ => None,
        }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> usize {
        let mut n = 0;
        while n < buf.len() {
            match self.read_byte_inner() {
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
