#include "gear.h"
#include "kds_registers.h"
#include <math.h>

// Tuning
static const float MIN_SPEED   = 3.0f;   // below this, ratio is meaningless
static const float MIN_RPM     = 600.0f; // below this, engine effectively off/idle-clutch
static const int   DEBOUNCE_N  = 3;      // consecutive samples before committing
static const float BAND_TOL    = 0.14f;  // +/-14% window around a learned band

int GearEstimator::update(float rpm, float speed, int gearRegRaw,
                          bool neutralActive) {
  int raw;

  // 1) Neutral switch wins outright.
  if (neutralActive) {
    raw = NEUTRAL;
  }
  // 2) Direct KDS gear register, if plausible (0..NUM_GEARS).
  else if (gearRegRaw >= 0 && gearRegRaw <= NUM_GEARS) {
    raw = gearRegRaw;                    // 0 here would be neutral per ECU map
  }
  // 3) Ratio classifier fallback.
  else {
    if (learning_) learnSample_(rpm, speed);
    raw = classifyByRatio_(rpm, speed);
  }

  // Debounce so a transient sample doesn't flicker the display.
  if (raw == candidate_) {
    if (stableCount_ < DEBOUNCE_N) stableCount_++;
  } else {
    candidate_ = raw;
    stableCount_ = 1;
  }
  if (stableCount_ >= DEBOUNCE_N && candidate_ != UNKNOWN) {
    gear_ = candidate_;
  }
  return gear_;
}

int GearEstimator::classifyByRatio_(float rpm, float speed) {
  if (isnan(rpm) || isnan(speed)) return UNKNOWN;
  if (rpm < MIN_RPM) return UNKNOWN;
  if (speed < MIN_SPEED) return UNKNOWN;      // stopped: rely on neutral switch

  float ratio = rpm / speed;
  int best = UNKNOWN;
  float bestErr = 1e9f;
  for (int g = 1; g <= NUM_GEARS; g++) {
    float c = band_[g - 1];
    if (isnan(c)) continue;
    float err = fabsf(ratio - c) / c;
    if (err < bestErr) { bestErr = err; best = g; }
  }
  if (best == UNKNOWN) return UNKNOWN;
  return (bestErr <= BAND_TOL) ? best : UNKNOWN;
}

// Learn: assume the rider holds one steady gear at a time. We watch the ratio;
// when it settles (low variance), we assign the next empty band. A simpler and
// very robust scheme in practice is to call setBand() during a guided
// calibration ride (hold each gear, press a button). This auto path is a bonus.
void GearEstimator::learnSample_(float rpm, float speed) {
  if (rpm < MIN_RPM || speed < MIN_SPEED) return;
  float ratio = rpm / speed;

  // reset the accumulator if the ratio jumped (a shift happened)
  if (learnN_ > 0 && fabsf(ratio - learnLast_) / learnLast_ > 0.10f) {
    learnSum_ = 0; learnN_ = 0;
  }
  learnLast_ = ratio;
  learnSum_ += ratio; learnN_++;

  if (learnN_ >= 25) {                   // ~stable for 25 samples
    float center = learnSum_ / learnN_;
    // place into the nearest empty slot preserving monotonic order later
    for (int g = 1; g <= NUM_GEARS; g++) {
      if (isnan(band_[g - 1])) {
        // avoid duplicating an already-learned band
        bool dup = false;
        for (int h = 1; h <= NUM_GEARS; h++)
          if (!isnan(band_[h - 1]) &&
              fabsf(center - band_[h - 1]) / band_[h - 1] < 0.08f) dup = true;
        if (!dup) band_[g - 1] = center;
        break;
      }
    }
    learnSum_ = 0; learnN_ = 0;
  }
}

void GearEstimator::setBand(int g, float ratioCenter) {
  if (g >= 1 && g <= NUM_GEARS) band_[g - 1] = ratioCenter;
}

// Sort learned bands descending (1st gear = highest rpm/speed ratio) so index
// order matches gear order.
void GearEstimator::sortBands_() {
  for (int i = 0; i < NUM_GEARS; i++)
    for (int j = i + 1; j < NUM_GEARS; j++) {
      float a = band_[i], b = band_[j];
      if (isnan(a) || (!isnan(b) && b > a)) { band_[i] = b; band_[j] = a; }
    }
}
