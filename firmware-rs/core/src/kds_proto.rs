//! Kawasaki KDS (ISO-14230 / KWP2000) protocol: framing, parsing, decoding.
//!
//! Pure logic only — no UART, no timing. The firmware layers transport
//! (byte I/O, echoes, P3min gaps) on top. Register meanings and response
//! shapes below were verified live on a 2024 W230.

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
// answers 7F/12; neutral comes from the switch wire on GPIO23). 0x03 initially
// looked like a neutral flag but a clutch-hold test proved it is the CLUTCH
// switch. 0x0A is battery volts, 0x04-0x08 look like sensor temps.
pub const REG_RPM: u8 = 0x09; // 2 bytes: hi*100 + lo
pub const REG_SPEED: u8 = 0x0C; // 1 byte on the W230 (2 on other models)
pub const REG_CLUTCH: u8 = 0x03; // 2 bytes: 00 00 = lever pulled, FF FF = released

/// Longest payload a single 0x80-format frame can carry (6-bit length field).
pub const MAX_PAYLOAD: usize = 63;

/// Sum-mod-256 checksum over a frame's bytes.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Build a tester→ECU request: `[0x80|len][tgt][src][payload...][cs]`.
///
/// # Panics
/// If `payload` is empty or longer than [`MAX_PAYLOAD`].
pub fn build_request(payload: &[u8]) -> Vec<u8> {
    assert!(!payload.is_empty() && payload.len() <= MAX_PAYLOAD);
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.push(0x80 | payload.len() as u8);
    frame.push(ECU_ADDR);
    frame.push(TESTER_ADDR);
    frame.extend_from_slice(payload);
    frame.push(checksum(&frame));
    frame
}

/// One parsed response frame.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    /// Every byte consumed, for logging.
    pub raw: Vec<u8>,
    /// Service byte + data (after fmt/tgt/src, before checksum).
    pub payload: Vec<u8>,
    /// False when a checksum byte arrived and didn't match. A missing
    /// checksum byte is tolerated (true), matching real-tool behaviour.
    pub checksum_ok: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing arrived at all (first byte timed out).
    Timeout,
    /// Frame started but ended early; bytes so far for logging.
    Truncated(Vec<u8>),
    /// Every byte read as 0x00 — the UART is sampling a line held low
    /// (transceiver unpowered or K-line shorted to ground).
    HeldLow(Vec<u8>),
}

/// Parse one response frame from a byte source (`None` = timeout).
///
/// Accepts both KWP2000 header shapes seen in the wild: length in the low 6
/// bits of the format byte, or a separate length byte after tgt/src.
pub fn parse_response(read_byte: &mut dyn FnMut() -> Option<u8>) -> Result<Frame, ParseError> {
    let Some(fmt) = read_byte() else {
        return Err(ParseError::Timeout);
    };
    let mut raw = vec![fmt];
    let mut next = |raw: &mut Vec<u8>| -> Result<u8, ParseError> {
        match read_byte() {
            Some(b) => {
                raw.push(b);
                Ok(b)
            }
            None => Err(ParseError::Truncated(raw.clone())),
        }
    };

    let payload_len = if fmt & 0xC0 == 0x80 && fmt & 0x3F != 0 {
        // length in low 6 bits of format byte; consume tgt + src
        next(&mut raw)?;
        next(&mut raw)?;
        (fmt & 0x3F) as usize
    } else {
        // 0x80 variant with a separate length byte after tgt/src
        next(&mut raw)?;
        next(&mut raw)?;
        next(&mut raw)? as usize
    };

    let payload_start = raw.len();
    for _ in 0..payload_len {
        next(&mut raw)?;
    }
    let payload = raw[payload_start..].to_vec();

    // Checksum byte: verify when present, tolerate its absence.
    let checksum_ok = match read_byte() {
        Some(cs) => {
            let want = checksum(&raw);
            raw.push(cs);
            cs == want
        }
        None => true,
    };

    // A frame of nothing but 0x00 is not data: a real frame always has a
    // non-zero format byte. (Observed on the bench with the module unpowered.)
    if raw.iter().all(|&b| b == 0) {
        return Err(ParseError::HeldLow(raw));
    }

    Ok(Frame {
        raw,
        payload,
        checksum_ok,
    })
}

/// From a positive-response payload, return the data bytes: expects
/// `[ok_service][echoed id...?][data...]`; the id echo is optional (some ECUs
/// omit it). `None` when the service byte is not the expected positive code.
pub fn positive_data<'p>(payload: &'p [u8], ok_service: u8, echoed_id: &[u8]) -> Option<&'p [u8]> {
    if payload.first() != Some(&ok_service) {
        return None;
    }
    let off = if payload.len() > echoed_id.len() && &payload[1..1 + echoed_id.len()] == echoed_id {
        1 + echoed_id.len()
    } else {
        1
    };
    Some(&payload[off..])
}

/// RPM from register 0x09 data: `hi*100 + lo`.
pub fn decode_rpm(data: &[u8]) -> Option<f32> {
    match data {
        [hi, lo, ..] => Some(*hi as f32 * 100.0 + *lo as f32),
        _ => None,
    }
}

/// Speed from register 0x0C data. The W230 sends one byte; other Kawasakis
/// two (`(hi<<8|lo)/2`).
pub fn decode_speed(data: &[u8]) -> Option<f32> {
    match data {
        [] => None,
        [v] => Some(*v as f32),
        [hi, lo, ..] => Some(((*hi as u16) << 8 | *lo as u16) as f32 / 2.0),
    }
}

/// Clutch switch from register 0x03 data: `Some(true)` = lever pulled.
pub fn decode_clutch(data: &[u8]) -> Option<bool> {
    match data {
        [0x00, 0x00] => Some(true),
        [0xFF, 0xFF] => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a fixed byte sequence, then timeouts.
    fn reader(bytes: &[u8]) -> impl FnMut() -> Option<u8> + '_ {
        let mut it = bytes.iter().copied();
        move || it.next()
    }

    // --- requests: exact frames captured on the wire from this firmware ---

    #[test]
    fn builds_start_communication_frame() {
        assert_eq!(build_request(&[SVC_START]), [0x81, 0x11, 0xF2, 0x81, 0x05]);
    }

    #[test]
    fn builds_start_session_frame() {
        assert_eq!(
            build_request(&[SVC_SESSION, SVC_SESSION_KDS]),
            [0x82, 0x11, 0xF2, 0x10, 0x80, 0x15]
        );
    }

    #[test]
    fn builds_read_rpm_frame() {
        assert_eq!(
            build_request(&[SVC_READ, REG_RPM]),
            [0x82, 0x11, 0xF2, 0x21, 0x09, 0xAF]
        );
    }

    // --- responses: exact frames captured from the 2024 W230 ECU ---

    #[test]
    fn parses_start_communication_ack() {
        // 80 F2 11 03 C1 EA 8F C0 — separate length byte variant
        let mut r = reader(&[0x80, 0xF2, 0x11, 0x03, 0xC1, 0xEA, 0x8F, 0xC0]);
        let f = parse_response(&mut r).unwrap();
        assert_eq!(f.payload, [0xC1, 0xEA, 0x8F]);
        assert!(f.checksum_ok);
        assert!(f.payload.contains(&SVC_START_OK));
    }

    #[test]
    fn parses_rpm_response() {
        // 80 F2 11 04 61 09 00 00 F1
        let mut r = reader(&[0x80, 0xF2, 0x11, 0x04, 0x61, 0x09, 0x00, 0x00, 0xF1]);
        let f = parse_response(&mut r).unwrap();
        assert_eq!(f.payload, [0x61, 0x09, 0x00, 0x00]);
        assert!(f.checksum_ok);
    }

    #[test]
    fn parses_negative_response() {
        // 80 F2 11 03 7F 21 12 38 — reg 0x0B rejection seen on the W230
        let mut r = reader(&[0x80, 0xF2, 0x11, 0x03, 0x7F, 0x21, 0x12, 0x38]);
        let f = parse_response(&mut r).unwrap();
        assert_eq!(f.payload, [0x7F, 0x21, 0x12]);
        assert_eq!(positive_data(&f.payload, SVC_READ_OK, &[0x0B]), None);
    }

    #[test]
    fn parses_length_in_format_byte() {
        // 83 F2 11 C1 EA 8F: length 3 in the fmt low bits, no length byte
        let mut r = reader(&[0x83, 0xF2, 0x11, 0xC1, 0xEA, 0x8F, 0xC0]);
        let f = parse_response(&mut r).unwrap();
        assert_eq!(f.payload, [0xC1, 0xEA, 0x8F]);
    }

    #[test]
    fn empty_line_is_timeout() {
        assert_eq!(parse_response(&mut reader(&[])), Err(ParseError::Timeout));
    }

    #[test]
    fn short_frame_is_truncated() {
        let mut r = reader(&[0x80, 0xF2, 0x11, 0x04, 0x61]);
        assert!(matches!(
            parse_response(&mut r),
            Err(ParseError::Truncated(raw)) if raw == [0x80, 0xF2, 0x11, 0x04, 0x61]
        ));
    }

    #[test]
    fn all_zeros_is_held_low() {
        // Exactly what an unpowered transceiver produced on the bench.
        let mut r = reader(&[0x00, 0x00, 0x00, 0x00, 0x00]);
        assert!(matches!(parse_response(&mut r), Err(ParseError::HeldLow(_))));
    }

    #[test]
    fn bad_checksum_is_flagged_not_fatal() {
        let mut r = reader(&[0x80, 0xF2, 0x11, 0x04, 0x61, 0x09, 0x00, 0x00, 0xFF]);
        let f = parse_response(&mut r).unwrap();
        assert!(!f.checksum_ok);
        assert_eq!(f.payload, [0x61, 0x09, 0x00, 0x00]);
    }

    #[test]
    fn missing_checksum_is_tolerated() {
        let mut r = reader(&[0x80, 0xF2, 0x11, 0x04, 0x61, 0x09, 0x00, 0x00]);
        assert!(parse_response(&mut r).unwrap().checksum_ok);
    }

    // --- payload extraction & decoding ---

    #[test]
    fn positive_data_strips_echoed_register() {
        assert_eq!(
            positive_data(&[0x61, 0x09, 0x12, 0x22], SVC_READ_OK, &[0x09]),
            Some(&[0x12, 0x22][..])
        );
    }

    #[test]
    fn positive_data_handles_missing_echo() {
        // Some ECUs omit the echoed register.
        assert_eq!(
            positive_data(&[0x61, 0x12, 0x22], SVC_READ_OK, &[0x09]),
            Some(&[0x12, 0x22][..])
        );
    }

    #[test]
    fn positive_data_strips_16bit_id() {
        assert_eq!(
            positive_data(&[0x62, 0x01, 0x0B, 0x55], SVC_READ_COMMON_OK, &[0x01, 0x0B]),
            Some(&[0x55][..])
        );
    }

    #[test]
    fn decodes_rpm() {
        assert_eq!(decode_rpm(&[0x12, 0x22]), Some(18.0 * 100.0 + 34.0));
        assert_eq!(decode_rpm(&[0x00, 0x00]), Some(0.0));
        assert_eq!(decode_rpm(&[0x12]), None);
    }

    #[test]
    fn decodes_speed_both_widths() {
        assert_eq!(decode_speed(&[40]), Some(40.0)); // W230 single byte
        assert_eq!(decode_speed(&[0x00, 0x50]), Some(40.0)); // 2-byte /2 variant
        assert_eq!(decode_speed(&[]), None);
    }

    #[test]
    fn decodes_clutch() {
        assert_eq!(decode_clutch(&[0x00, 0x00]), Some(true)); // pulled
        assert_eq!(decode_clutch(&[0xFF, 0xFF]), Some(false)); // released
        assert_eq!(decode_clutch(&[0x00, 0xFF]), None);
    }
}
