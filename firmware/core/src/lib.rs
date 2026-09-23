//! Hardware-independent logic for the W230 gear indicator.
//!
//! Everything in this crate is pure computation — KDS/KWP2000 protocol
//! framing and decoding, gear estimation, ratio-histogram learning, LED frame
//! rendering, the BLE GATT wire formats and the OTA manifest rules — so it
//! builds and unit-tests on the host (`cargo test-host` from the workspace
//! root). The `esp32` crate layers UART, NVS, Bluetooth, WiFi, OTA and the
//! poll loop on top.

pub mod ble_proto;
pub mod diag;
pub mod display;
pub mod gear;
pub mod kds_proto;
pub mod learn;
pub mod ota_manifest;
