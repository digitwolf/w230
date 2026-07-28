// gear.h — gear determination with three prioritised sources:
//   1. Neutral switch  -> definitive "N".
//   2. Direct KDS gear register (if the W230 exposes one).
//   3. RPM/speed ratio classifier with auto-learned per-gear bands.
//
// Result: -1 = unknown, 0 = Neutral, 1..NUM_GEARS = gear.
#pragma once

#include <Arduino.h>

class GearEstimator {
 public:
  static const int UNKNOWN = -1;
  static const int NEUTRAL = 0;

  // Feed the latest sample. `gearRegRaw` < 0 means "no register value".
  // `neutralActive` true means the neutral switch reports neutral.
  // Returns the current debounced gear.
  int update(float rpm, float speed, int gearRegRaw, bool neutralActive);

  int gear() const { return gear_; }

  // --- ratio-band calibration (fallback path) ---
  // Enter learn mode: while riding steadily in each gear, call learnSample().
  void beginLearn() { learning_ = true; }
  void endLearn()   { learning_ = false; sortBands_(); }
  bool learning() const { return learning_; }
  // Manually inject a known band (ratio center) for gear g (1..NUM_GEARS).
  void setBand(int g, float ratioCenter);
  float band(int g) const { return (g >= 1 && g <= 5) ? band_[g - 1] : NAN; }

 private:
  int classifyByRatio_(float rpm, float speed);
  void learnSample_(float rpm, float speed);
  void sortBands_();

  int gear_ = UNKNOWN;
  int candidate_ = UNKNOWN;
  int stableCount_ = 0;

  bool  learning_ = false;
  float band_[5]  = {NAN, NAN, NAN, NAN, NAN};   // ratio center per gear (index 0 = 1st)

  // running stats while learning the currently-held gear
  float learnSum_ = 0; int learnN_ = 0; float learnLast_ = 0;
};
