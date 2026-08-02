#include "kds.h"
#include "kds_registers.h"
#include "pins.h"

// --- timing constants (ISO-14230 fast init) ---
static const uint32_t WUP_IDLE_MS  = 300;  // K-line idle high before wake-up
static const uint32_t WUP_LOW_MS   = 25;   // low pulse
static const uint32_t WUP_HIGH_MS  = 25;   // high pulse
static const uint32_t P4_TX_GAP_MS = 5;    // inter-byte gap on TX
static const uint32_t RSP_TIMEOUT  = 250;  // per-response timeout

static uint8_t checksum(const uint8_t* b, int n) {
  uint16_t s = 0;
  for (int i = 0; i < n; i++) s += b[i];
  return (uint8_t)(s & 0xFF);
}

bool KDS::begin() {
  connected_ = false;
  if (!fastInit_()) return false;

  // startCommunication: fmt=0x81, target, source, service, checksum
  uint8_t req[3] = {KDS_SVC_START, 0, 0};   // only the service byte is payload
  // Build full frame with header inside sendRequest_ below expects payload only.
  if (!sendRequest_(req, 1)) return false;

  uint8_t resp[16];
  int n = readResponse_(resp, sizeof(resp), RSP_TIMEOUT);
  // Positive response to startCommunication is service 0xC1 in the payload.
  for (int i = 0; i < n; i++) {
    if (resp[i] == KDS_SVC_START_OK) { connected_ = true; break; }
  }
  return connected_;
}

bool KDS::fastInit_() {
  ser_.end();
  // Manually toggle the TX line for the wake-up pattern.
  pinMode(txPin_, OUTPUT);
  digitalWrite(txPin_, HIGH);
  delay(WUP_IDLE_MS);
  digitalWrite(txPin_, LOW);
  delay(WUP_LOW_MS);
  digitalWrite(txPin_, HIGH);
  delay(WUP_HIGH_MS);

  ser_.begin(KDS_BAUD, SERIAL_8N1, rxPin_, txPin_);
  ser_.setTimeout(RSP_TIMEOUT);
  // flush any noise from the line transition
  while (ser_.available()) ser_.read();
  return true;
}

// data = payload (service + params). We prepend fmt/target/source and append cs.
bool KDS::sendRequest_(const uint8_t* data, int len) {
  if (len < 1 || len > 63) return false;
  uint8_t frame[70];
  int i = 0;
  frame[i++] = 0x80 | (uint8_t)len;   // format byte: length in low 6 bits
  frame[i++] = KDS_ECU_ADDR;          // target
  frame[i++] = KDS_TESTER_ADDR;       // source
  for (int k = 0; k < len; k++) frame[i++] = data[k];
  frame[i] = checksum(frame, i);
  i++;

  for (int k = 0; k < i; k++) {
    ser_.write(frame[k]);
    ser_.flush();          // ensure byte is on the wire before next
    delay(P4_TX_GAP_MS);
  }
  drainEcho_(i);           // discard our own echo (single-wire bus)
  return true;
}

void KDS::drainEcho_(int nBytes) {
  for (int k = 0; k < nBytes; k++) readByte_(RSP_TIMEOUT);
}

int KDS::readByte_(uint32_t timeoutMs) {
  uint32_t start = millis();
  while (!ser_.available()) {
    if (millis() - start > timeoutMs) return -1;
    delay(1);
  }
  return ser_.read();
}

// Reads one response frame, returns the payload bytes (after fmt/tgt/src, before
// checksum) into out. Returns payload length or -1.
int KDS::readResponse_(uint8_t* out, int maxLen, uint32_t timeoutMs) {
  int fmt = readByte_(timeoutMs);
  if (fmt < 0) return -1;

  int payloadLen;
  int hdr = 1;
  if ((fmt & 0xC0) == 0x80) {            // length embedded in format byte
    payloadLen = fmt & 0x3F;
    // consume target + source
    if (readByte_(timeoutMs) < 0) return -1;
    if (readByte_(timeoutMs) < 0) return -1;
    hdr = 3;
  } else if (fmt == 0x80) {              // separate length byte variant
    if (readByte_(timeoutMs) < 0) return -1;  // target
    if (readByte_(timeoutMs) < 0) return -1;  // source
    int lb = readByte_(timeoutMs);
    if (lb < 0) return -1;
    payloadLen = lb;
    hdr = 4;
  } else {
    payloadLen = fmt & 0x3F;             // best-effort fallback
  }

  int n = 0;
  for (int k = 0; k < payloadLen; k++) {
    int b = readByte_(timeoutMs);
    if (b < 0) return (n > 0) ? n : -1;
    if (n < maxLen) out[n++] = (uint8_t)b;
  }
  readByte_(timeoutMs);                  // consume/ignore checksum
  return n;
}

int KDS::readRegister(uint8_t reg, uint8_t* out, int maxLen) {
  if (!connected_) return -1;
  uint8_t req[2] = {KDS_SVC_READ, reg};
  if (!sendRequest_(req, 2)) return -1;

  uint8_t resp[32];
  int n = readResponse_(resp, sizeof(resp), RSP_TIMEOUT);
  if (n < 2) return -1;
  // Expect: [0x61][reg][data...]
  if (resp[0] != KDS_SVC_READ_OK) return -1;
  int dataOff = (resp[1] == reg) ? 2 : 1;   // some ECUs omit the echoed reg
  int dataLen = n - dataOff;
  if (dataLen < 0) return -1;
  int copy = min(dataLen, maxLen);
  for (int i = 0; i < copy; i++) out[i] = resp[dataOff + i];
  return copy;
}

float KDS::readRpm() {
  uint8_t d[4];
  int n = readRegister(REG_RPM, d, sizeof(d));
  if (n < 2) return NAN;
  return (float)d[0] * 100.0f + (float)d[1];
}

float KDS::readSpeed() {
  uint8_t d[4];
  int n = readRegister(REG_SPEED, d, sizeof(d));
  if (n < 2) return NAN;
  return (float)(((uint16_t)d[0] << 8) | d[1]) / 2.0f;
}

int KDS::readGearRaw() {
  if (!KDS_HAS_GEAR_REGISTER) return -1;
  uint8_t d[2];
  int n = readRegister(REG_GEAR, d, sizeof(d));
  if (n < 1) return -1;
  return d[0];
}
