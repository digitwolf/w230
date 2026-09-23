//! Hardware-independent logic for the W230 gear indicator.
//!
//! Everything in this crate is pure computation — KDS/KWP2000 protocol
//! framing and decoding, gear estimation, ratio-histogram learning, LED frame
//! rendering, diagnostic wire formats — so it builds and unit-tests on the
//! host (`cargo test-host` from the workspace root). The `firmware` crate
//! layers UART, NVS, WiFi, and the poll loop on top.

pub mod diag;
pub mod display;
pub mod gear;
pub mod kds_proto;
pub mod learn;
