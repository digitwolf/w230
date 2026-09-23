import Foundation

/// Sample device state for the in-app demo (Connect sheet → "Try the demo",
/// or launch argument `-demo`). Used for App Store screenshots and lets a
/// reviewer see every screen without the indicator hardware.
enum DemoData {
    static let deviceInfo = DeviceInfo(
        proto: 1, fw: "0.2.4", project: "w230-gear-indicator", idf: "v5.3.3",
        built: "Sep 22 2026 19:15:55", hw: "atom-matrix", slot: "ota_1",
        otaCapable: true, pendingVerify: false, bootPolicy: 1, uptime: 1_842,
        freeHeap: 116_804, minFreeHeap: 44_032, resetReason: "power-on",
        mac: "64:B7:08:81:AE:6E", bleConns: 1)

    static let settings = DeviceSettings(
        brightness: 1, brightnessSteps: [40, 120, 255], bootPolicy: 1,
        manifestUrl: "https://d394jgrm9p9yqj.cloudfront.net/w230/manifest.json",
        manifestDefault: true, wifiSsid: "Garage")

    static let blackBox = BlackBox(
        boots: 37, abnormalResets: 0, lastReset: "power-on", linkDrops: 4,
        minFreeHeap: 44_032, interlockOdd: 0, interlockLastOdd: 0,
        maxRpm: 8_120, maxSpeed: 118,
        gates: .init(accepted: 6_412, clutch: 0, neutral: 388, rpmLow: 141,
                     speedLow: 903, outOfRange: 12, binFull: 0))

    static let factory: [Double] = [196.9, 135.7, 102.1, 82.8, 68.3, 55.9]
    static let learned: [Double] = [203.4, 139.8, 104.6, 84.9, 69.7, 57.1]

    static let calibration = Calibration(
        factory: factory, bands: learned, learned: 6, samples: 6_412,
        peaks: [[104.6, 1_930], [84.9, 1_512], [69.7, 1_204], [139.8, 880], [57.1, 611], [203.4, 275]])

    /// Histogram with one peak per learned band, widths like a real ride.
    static var histogram: Histogram {
        var h = Histogram()
        h.samples = 6_412
        let masses = [275.0, 880, 1_930, 1_512, 1_204, 611]
        for (i, centre) in learned.enumerated() {
            let sigma = centre * 0.035
            for bin in 0..<W230Protocol.histogramBins {
                let r = W230Protocol.binRatio(bin)
                let z = (r - centre) / sigma
                h.bins[bin] += Int((masses[i] / (sigma * 2.5)) * exp(-0.5 * z * z))
            }
        }
        h.bins[0] = 5 // the firmware's boot marker
        h.pagesLoaded = [0, 1]
        return h
    }

    static let events: [FirmwareEvent] = [
        .init(t: 0, e: "boot fw 0.2.4"), .init(t: 0, e: "slot ota_1"),
        .init(t: 9, e: "ota checking"), .init(t: 17, e: "wifi connected"),
        .init(t: 18, e: "ota upToDate"), .init(t: 21, e: "self-test passed"),
        .init(t: 24, e: "k-line up"), .init(t: 108, e: "wifi off"),
        .init(t: 1_790, e: "k-line down"), .init(t: 1_793, e: "k-line up"),
    ]

    static let wifi = WifiStatus(configured: true, ssid: "Garage", state: .off, ip: nil, rssi: nil, error: nil)

    static let ota = OtaStatus(
        state: .upToDate, progress: 0, current: "0.2.4", available: nil,
        notes: nil, size: nil, error: nil, lastCheck: 18, lastCheckOk: true)

    /// One simulated ride: a gentle pull through the gears, 4 samples/s.
    static func live(tick: Int, brightness: Int, samples: UInt32) -> LiveStatus? {
        let t = Double(tick) / 4.0
        let phase = t.truncatingRemainder(dividingBy: 48)
        let gearIndex = min(5, Int(phase / 8))            // 8 s per gear
        let within = (phase - Double(gearIndex) * 8) / 8   // 0..1 inside the gear
        let rpm = 2_800 + within * 3_600
        let ratio = learned[gearIndex]
        let speed = rpm / ratio
        var flags: UInt8 = 0x01 // link up
        var gear = UInt8(gearIndex + 1)
        if phase < 1.5 { flags |= 0x02; gear = 7 } // start in neutral
        var b = [UInt8](repeating: 0, count: 20)
        b[0] = 1; b[1] = flags; b[2] = gear; b[3] = UInt8(brightness)
        let r = UInt16(rpm.rounded()), s = UInt16(speed.rounded())
        b[4] = UInt8(r & 0xff); b[5] = UInt8(r >> 8)
        b[6] = UInt8(s & 0xff); b[7] = UInt8(s >> 8)
        let f = Float(rpm / speed).bitPattern
        for i in 0..<4 { b[8 + i] = UInt8((f >> (8 * UInt32(i))) & 0xff) }
        for i in 0..<4 { b[12 + i] = UInt8((samples >> (8 * UInt32(i))) & 0xff) }
        let up = UInt32(1_842 + tick / 4)
        for i in 0..<4 { b[16 + i] = UInt8((up >> (8 * UInt32(i))) & 0xff) }
        return LiveStatus(data: Data(b))
    }
}
