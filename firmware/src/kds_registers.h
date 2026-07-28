// kds_registers.h — KDS/KWP2000 local identifiers (registers) for live data.
//
// !!! VERIFY ON THE W230 !!!
// These defaults are derived from other Kawasaki ECUs (Kawaduino / Z1000SX).
// They are a *starting point*. Use SCAN mode (see main.cpp) to confirm which
// register tracks RPM, speed, and — if present — gear on the actual W230, then
// update the values below.
#pragma once

#include <stdint.h>

// KWP2000 addressing
static const uint8_t KDS_ECU_ADDR    = 0x11;  // target (ECU)
static const uint8_t KDS_TESTER_ADDR = 0xF2;  // source (this module)

// Service IDs
static const uint8_t KDS_SVC_READ    = 0x21;  // readDataByLocalIdentifier
static const uint8_t KDS_SVC_READ_OK = 0x61;  // positive response to 0x21
static const uint8_t KDS_SVC_START   = 0x81;  // startCommunication
static const uint8_t KDS_SVC_START_OK= 0xC1;  // positive response to 0x81

// Local identifiers (registers)  -- VERIFY --
static const uint8_t REG_RPM   = 0x09;  // 2 bytes: hi*100 + lo
static const uint8_t REG_SPEED = 0x0C;  // 2 bytes: (hi<<8 | lo) / 2
static const uint8_t REG_TEMP  = 0x06;  // 1 byte: (raw - 48)/1.6  [degC]
static const uint8_t REG_TPS   = 0x04;  // 2 bytes: 0x00D8..0x037F -> 0..100%
static const uint8_t REG_GEAR  = 0x0B;  // 1 byte gear -- MAY NOT EXIST on W230 --

// Set false if the W230 has no working gear register; the module will then
// always compute gear from RPM/speed ratio.
static const bool KDS_HAS_GEAR_REGISTER = true;

// Number of forward gears (W230 = 5-speed).
static const int NUM_GEARS = 5;
