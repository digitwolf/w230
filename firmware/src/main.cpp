// main.cpp — W230 gear indicator (M5Stack Core / ESP32).
//
// Modes (cycle with button B):
//   RUN   — big gear digit (N,1..5) from KDS + neutral switch, with a shift
//           light (amber -> red -> flashing) driven by RPM.
//   SCAN  — dump KDS registers 0x00..0x3F for W230 register discovery (docs/01).
//   CAL   — guided ratio calibration: hold each gear, press A to capture. Bands
//           are persisted to NVS and reloaded on boot.
//
// Button A: RUN=toggle RPM/speed overlay · CAL=capture band · SCAN=restart scan.
#include <M5Unified.h>
#include <Preferences.h>
#include "pins.h"
#include "kds.h"
#include "kds_registers.h"
#include "gear.h"

KDS kds(Serial2, KDS_RX_PIN, KDS_TX_PIN);
GearEstimator estimator;
Preferences prefs;   // NVS storage for calibrated ratio bands

enum Mode { RUN, SCAN, CAL };
Mode mode = RUN;
bool showOverlay = true;
int  calTargetGear = 1;

// ---- shift light ----
// W230 makes peak power at 7,500 rpm; redline is a little above. Warn amber
// approaching, red at the shift point, and flash the border past it. Tune to
// taste (and lower for a short-shifting economy style).
static const float SHIFT_WARN_RPM  = 6800.0f;   // amber bar
static const float SHIFT_RPM       = 7800.0f;   // red — time to shift
static const float SHIFT_FLASH_RPM = 8600.0f;   // flashing red border
static bool  flashOn = false;
static uint32_t lastFlash = 0;

// Default ratio bands (unitless rpm/speed). Placeholders until CAL overrides
// them; also the fallback when NVS is empty.
static const float DEFAULT_BANDS[NUM_GEARS] = {240.0f, 165.0f, 125.0f, 100.0f, 82.0f};

// ---- calibration persistence (NVS) ----
static void loadBands() {
  prefs.begin("gear", /*readOnly=*/true);
  for (int g = 1; g <= NUM_GEARS; g++) {
    char key[4]; snprintf(key, sizeof(key), "b%d", g);
    estimator.setBand(g, prefs.getFloat(key, DEFAULT_BANDS[g - 1]));
  }
  prefs.end();
}

static void saveBands() {
  prefs.begin("gear", /*readOnly=*/false);
  for (int g = 1; g <= NUM_GEARS; g++) {
    char key[4]; snprintf(key, sizeof(key), "b%d", g);
    float v = estimator.band(g);
    if (!isnan(v)) prefs.putFloat(key, v);
  }
  prefs.end();
}

// ---- optional analog taps (interrupt pulse counting) ----
volatile uint32_t rpmPulses = 0, vssPulses = 0;
void IRAM_ATTR onRpmPulse() { rpmPulses++; }
void IRAM_ATTR onVssPulse() { vssPulses++; }

static bool neutralActive() {
  // LOW = neutral (switch grounds the line).
  return digitalRead(NEUTRAL_PIN) == LOW;
}

// ---------------- display ----------------
static void drawGear(int g, float rpm, float speed, bool connected) {
  auto& d = M5.Display;
  d.startWrite();
  d.fillScreen(TFT_BLACK);

  // big centred gear glyph
  const char* label = (g == GearEstimator::NEUTRAL) ? "N"
                    : (g >= 1) ? nullptr : "-";
  char buf[2];
  if (label == nullptr) { buf[0] = '0' + g; buf[1] = 0; label = buf; }

  d.setTextDatum(middle_center);
  d.setTextColor((g == GearEstimator::NEUTRAL) ? TFT_GREEN : TFT_WHITE, TFT_BLACK);
  d.setTextSize(1);
  d.setFont(&fonts::Font7);            // 7-seg style
  d.setTextSize(2);
  d.drawString(label, d.width() / 2, d.height() / 2 - 6);

  // status + overlay
  d.setFont(&fonts::Font2);
  d.setTextSize(1);
  d.setTextDatum(top_left);
  d.setTextColor(connected ? TFT_GREEN : TFT_RED, TFT_BLACK);
  d.drawString(connected ? "KDS" : "NO-LINK", 4, 4);

  if (showOverlay) {
    d.setTextColor(TFT_CYAN, TFT_BLACK);
    d.setTextDatum(bottom_left);
    char o[48];
    snprintf(o, sizeof(o), "%.0f rpm  %.0f",
             isnan(rpm) ? 0.f : rpm, isnan(speed) ? 0.f : speed);
    d.drawString(o, 4, d.height() - 4);
  }

  // shift light: coloured border once RPM enters the warn/shift/flash zones.
  if (!isnan(rpm) && rpm >= SHIFT_WARN_RPM) {
    uint16_t col;
    bool draw = true;
    if (rpm >= SHIFT_FLASH_RPM) {
      if (millis() - lastFlash > 80) { flashOn = !flashOn; lastFlash = millis(); }
      col = TFT_RED; draw = flashOn;                 // urgent: flashing red
    } else if (rpm >= SHIFT_RPM) {
      col = TFT_RED; flashOn = false;                // shift now: solid red
    } else {
      col = TFT_ORANGE; flashOn = false;             // approaching: amber
    }
    if (draw)
      for (int t = 0; t < 6; t++)
        d.drawRect(t, t, d.width() - 2 * t, d.height() - 2 * t, col);
  } else {
    flashOn = false;
  }
  d.endWrite();
}

static void drawScan(uint8_t reg, const uint8_t* data, int n) {
  auto& d = M5.Display;
  d.setFont(&fonts::Font2);
  d.setTextSize(1);
  d.setTextColor(TFT_WHITE, TFT_BLACK);
  d.setTextDatum(top_left);
  char line[64];
  int y = 4 + (reg % 16) * 14;
  if (reg % 16 == 0) d.fillScreen(TFT_BLACK);
  if (n <= 0) {
    snprintf(line, sizeof(line), "%02X: --", reg);
  } else {
    int p = snprintf(line, sizeof(line), "%02X:", reg);
    for (int i = 0; i < n && i < 6; i++)
      p += snprintf(line + p, sizeof(line) - p, " %02X", data[i]);
  }
  d.drawString(line, 4, y);
}

// ---------------- setup ----------------
void setup() {
  auto cfg = M5.config();
  M5.begin(cfg);
  M5.Display.setRotation(1);
  M5.Display.fillScreen(TFT_BLACK);

  pinMode(NEUTRAL_PIN, INPUT_PULLUP);
  if (USE_ANALOG_TAPS) {
    pinMode(RPM_TAP_PIN, INPUT);
    pinMode(VSS_TAP_PIN, INPUT);
    attachInterrupt(RPM_TAP_PIN, onRpmPulse, RISING);
    attachInterrupt(VSS_TAP_PIN, onVssPulse, RISING);
  }

  // Load calibrated ratio bands from NVS (falls back to DEFAULT_BANDS if unset).
  // Replace them any time via CAL mode; captures are persisted automatically.
  loadBands();

  M5.Display.setFont(&fonts::Font2);
  M5.Display.drawString("Connecting KDS...", 10, 10);
  kds.begin();
}

// ---------------- loop ----------------
static uint32_t lastPoll = 0;

void loop() {
  M5.update();

  // Button B: cycle mode.
  if (M5.BtnB.wasPressed()) {
    mode = (Mode)((mode + 1) % 3);
    M5.Display.fillScreen(TFT_BLACK);
  }

  if (mode == RUN) {
    if (millis() - lastPoll >= 100) {
      lastPoll = millis();
      if (!kds.connected()) kds.begin();

      float rpm = kds.connected() ? kds.readRpm() : NAN;
      float spd = kds.connected() ? kds.readSpeed() : NAN;
      int   gRaw = kds.connected() ? kds.readGearRaw() : -1;

      int g = estimator.update(rpm, spd, gRaw, neutralActive());
      drawGear(g, rpm, spd, kds.connected());
    }
    if (M5.BtnA.wasPressed()) showOverlay = !showOverlay;
  }

  else if (mode == SCAN) {
    // Dump 0x00..0x3F once; press A to rescan. See docs/01 §4 for how to read it.
    for (uint8_t reg = 0; reg < 0x40; reg++) {
      if (!kds.connected()) kds.begin();
      uint8_t d[8];
      int n = kds.connected() ? kds.readRegister(reg, d, sizeof(d)) : -1;
      drawScan(reg, d, n);
      delay(30);
      M5.update();
      if (M5.BtnB.wasPressed()) { mode = RUN; M5.Display.fillScreen(TFT_BLACK); return; }
    }
    while (mode == SCAN && !M5.BtnA.wasPressed() && !M5.BtnB.wasPressed()) {
      M5.update(); delay(20);
    }
    if (M5.BtnB.wasPressed()) mode = RUN;
    M5.Display.fillScreen(TFT_BLACK);
  }

  else if (mode == CAL) {
    // Guided: ride steadily in `calTargetGear`, press A to capture its ratio.
    float rpm = kds.connected() ? kds.readRpm() : NAN;
    float spd = kds.connected() ? kds.readSpeed() : NAN;
    auto& d = M5.Display;
    d.fillScreen(TFT_BLACK);
    d.setFont(&fonts::Font2); d.setTextDatum(top_left);
    d.setTextColor(TFT_YELLOW, TFT_BLACK);
    char l[64];
    snprintf(l, sizeof(l), "CAL gear %d", calTargetGear); d.drawString(l, 6, 8);
    snprintf(l, sizeof(l), "%.0f rpm  %.0f spd", isnan(rpm)?0.f:rpm, isnan(spd)?0.f:spd);
    d.drawString(l, 6, 30);
    float ratio = (!isnan(rpm) && !isnan(spd) && spd > 3) ? rpm / spd : NAN;
    snprintf(l, sizeof(l), "ratio %.1f", isnan(ratio)?0.f:ratio); d.drawString(l, 6, 52);
    d.drawString("A=capture  B=exit", 6, 80);

    if (M5.BtnA.wasPressed() && !isnan(ratio)) {
      estimator.setBand(calTargetGear, ratio);
      saveBands();                       // persist to NVS immediately
      calTargetGear++;
      if (calTargetGear > NUM_GEARS) { calTargetGear = 1; mode = RUN; }
    }
    delay(120);
  }
}
