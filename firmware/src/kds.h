// kds.h — minimal, READ-ONLY Kawasaki KDS (ISO-14230 / KWP2000) K-line client.
//
// Physical layer: single bidirectional K-line through an L9637D transceiver on
// UART2 @ 10400 8N1. Because it is one wire, every TX byte is echoed back on RX;
// this driver reads and discards those echoes.
//
// Scope is deliberately limited to startCommunication + readDataByLocalIdentifier
// (service 0x21). No ECU writes / actuator tests — see docs/02 safety notes.
#pragma once

#include <Arduino.h>

class KDS {
 public:
  KDS(HardwareSerial& serial, int rxPin, int txPin)
      : ser_(serial), rxPin_(rxPin), txPin_(txPin) {}

  // Perform ISO-14230 fast-init + startCommunication. Returns true on ECU ACK.
  bool begin();

  bool connected() const { return connected_; }

  // Read a register (local identifier). Copies up to `maxLen` payload bytes into
  // `out`, returns the number of payload bytes, or -1 on error/timeout.
  int readRegister(uint8_t reg, uint8_t* out, int maxLen);

  // Convenience decoders (return NAN / -1 on failure). Units per kds_registers.h.
  float readRpm();
  float readSpeed();
  int   readGearRaw();   // raw gear register byte, or -1

 private:
  bool fastInit_();
  bool sendRequest_(const uint8_t* data, int len);
  int  readResponse_(uint8_t* out, int maxLen, uint32_t timeoutMs);
  int  readByte_(uint32_t timeoutMs);          // -1 on timeout
  void drainEcho_(int nBytes);                 // swallow n echoed TX bytes

  HardwareSerial& ser_;
  int rxPin_, txPin_;
  bool connected_ = false;
};
