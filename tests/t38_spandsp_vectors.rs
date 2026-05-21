#![cfg(feature = "t38")]

//! Cross-validate T.38 IFP encoding against spandsp (reference implementation).
//!
//! KEY FINDING: spandsp uses a SIMPLIFIED binary format, NOT standard T.38 PER.
//!
//! spandsp indicator format (1 byte):
//!   byte = (value << 1)    where value = T.38 indicator number
//!   Example: CNG(1) → 0x02, CED(2) → 0x04, V21(3) → 0x06
//!
//! Our `IfpPacket::encode()` uses standard T.38 ASN.1 PER (Annex A).
//! Our `IfpPacket::encode_spandsp()` produces the simplified format.
//!
//! For spandsp-based systems (FreeSWITCH/Asterisk), use encode_spandsp().

use std::ffi::{c_int, c_void};

use rustrtc::t38::{IfpPacket, T30Indicator};

// ──────────────────────────────────────────────
// FFI state (one-time init, all tests share)
// ──────────────────────────────────────────────

static mut RX_INDICATOR: Option<c_int> = None;

extern "C" fn rx_indicator_handler(
    _s: *mut spandsp_sys::t38_core_state_t,
    _user_data: *mut c_void,
    indicator: c_int,
) -> c_int {
    unsafe { RX_INDICATOR = Some(indicator) };
    0
}

/// A thin wrapper around a spandsp T.38 core context used for decoding.
/// Each instance gets its own context, freed on drop.
struct SpandspSession {
    ptr: *mut spandsp_sys::t38_core_state_t,
}

unsafe impl Send for SpandspSession {}

impl SpandspSession {
    fn new() -> Self {
        let ptr = unsafe {
            spandsp_sys::t38_core_init(
                std::ptr::null_mut(),
                Some(rx_indicator_handler as extern "C" fn(_, _, _) -> c_int),
                None,
                None,
                std::ptr::null_mut(),
                None,
                std::ptr::null_mut(),
            )
        };
        assert!(!ptr.is_null(), "t38_core_init failed");
        Self { ptr }
    }

    fn decode(&self, bytes: &[u8], seq: u16) -> Result<(), String> {
        unsafe { RX_INDICATOR = None };
        let ret = unsafe {
            spandsp_sys::t38_core_rx_ifp_packet(self.ptr, bytes.as_ptr(), bytes.len() as c_int, seq)
        };
        if ret < 0 {
            Err(format!(
                "rx_ifp_packet returned {} (bytes={:02x?})",
                ret, bytes
            ))
        } else {
            Ok(())
        }
    }

    fn decoded_indicator(&self) -> Option<c_int> {
        unsafe { RX_INDICATOR }
    }
}

impl Drop for SpandspSession {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { spandsp_sys::t38_core_free(self.ptr) };
        }
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

/// Spandsp indicators that work reliably. Higher values (V34, V8) are
/// not supported by spandsp's decoder.
const SPANDSP_INDICATORS: &[(T30Indicator, u8)] = &[
    (T30Indicator::NoSignal, 0),
    (T30Indicator::Cng, 1),
    (T30Indicator::Ced, 2),
    (T30Indicator::V21Preamble, 3),
    (T30Indicator::V27Ter2400Preamble, 4),
    (T30Indicator::V27Ter4800Preamble, 5),
    (T30Indicator::V297200Preamble, 6),
    (T30Indicator::V299600Preamble, 7),
    (T30Indicator::V177200ShortPreamble, 8),
    (T30Indicator::V177200LongPreamble, 9),
    (T30Indicator::V179600ShortPreamble, 10),
    (T30Indicator::V179600LongPreamble, 11),
    (T30Indicator::V1712000ShortPreamble, 12),
    (T30Indicator::V1712000LongPreamble, 13),
    (T30Indicator::V1714400ShortPreamble, 14),
    (T30Indicator::V1714400LongPreamble, 15),
];

#[test]
fn test_spandsp_roundtrip_self_format() {
    let ss = SpandspSession::new();
    for &(_, val) in SPANDSP_INDICATORS {
        let encoded = vec![val << 1];
        ss.decode(&encoded, val as u16 + 1)
            .unwrap_or_else(|e| panic!("IND {}: {}", val, e));
        let decoded = ss.decoded_indicator();
        assert_eq!(decoded, Some(val as c_int), "IND {} roundtrip", val);
    }
}

#[test]
fn test_our_spandsp_encoder_produces_correct_format() {
    let ss = SpandspSession::new();
    for &(ind, val) in SPANDSP_INDICATORS {
        let packet = IfpPacket::T30Indicator(vec![ind]);
        let encoded = packet.encode_spandsp().unwrap();
        ss.decode(&encoded, val as u16 + 1)
            .unwrap_or_else(|e| panic!("IND {} encode_spandsp: {}", val, e));
        let decoded = ss.decoded_indicator();
        assert_eq!(
            decoded,
            Some(val as c_int),
            "IND {} via encode_spandsp",
            val
        );
    }
}

#[test]
fn test_our_per_encoder_cannot_decode_by_spandsp() {
    let ss = SpandspSession::new();
    let packet = IfpPacket::T30Indicator(vec![T30Indicator::Cng]);
    let encoded = packet.encode().unwrap(); // standard PER
    let result = ss.decode(&encoded, 1);
    assert!(
        result.is_err(),
        "spandsp should NOT decode our PER format — bytes={:02x?}",
        encoded
    );
}

#[test]
fn test_spandsp_format_documentation() {
    // This is a known-answer test that documents the spandsp format
    let cases = [
        (T30Indicator::Cng, 0x02),
        (T30Indicator::Ced, 0x04),
        (T30Indicator::V21Preamble, 0x06),
        (T30Indicator::NoSignal, 0x00),
        (T30Indicator::V27Ter2400Preamble, 0x08),
        (T30Indicator::V27Ter4800Preamble, 0x0a),
    ];
    for (ind, expected_byte) in &cases {
        let computed = (*ind as u8) << 1;
        assert_eq!(computed, *expected_byte, "mismatch for {:?}", ind);
    }
}
