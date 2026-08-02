// pins.h — hardware pin assignments (M5Stack Core / ESP32).
// Adjust for M5StickC Plus2 or a bare ESP32 as noted in hardware/wiring.md.
#pragma once

// --- KDS K-line via L9637D on UART2 ---
static const int KDS_RX_PIN = 16;   // ESP32 RX  <- L9637D RX (through 5V->3.3V divider)
static const int KDS_TX_PIN = 17;   // ESP32 TX  -> L9637D TX
static const uint32_t KDS_BAUD = 10400;

// --- Neutral switch: grounds when in neutral -> reads LOW ---
static const int NEUTRAL_PIN = 26;  // INPUT_PULLUP

// --- Optional analog tap inputs (fallback path; leave unused if KDS is trusted) ---
static const int RPM_TAP_PIN = 36;  // opto-isolated coil-primary pulse (input-only pin)
static const int VSS_TAP_PIN = 25;  // opto-isolated speed-sensor pulse

// Set to true only if you have physically wired the analog taps above.
static const bool USE_ANALOG_TAPS = false;
