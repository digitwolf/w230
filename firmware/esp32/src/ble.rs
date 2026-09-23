//! BLE GATT server: the "W230 Gear" service the iOS app talks to.
//!
//! Attribute layout, byte formats and opcodes are defined once in
//! `w230_core::ble_proto` (host-tested); this module is only the Bluedroid
//! plumbing: build the service, keep attribute values current, notify
//! subscribers, and hand writes (commands, WiFi credentials) to the main
//! loop through channels. Nothing here touches the K-line or the display.
//!
//! Threading: Bluedroid delivers events on its own BTC task; handlers only
//! take the state mutex briefly and push into bounded channels, so a slow
//! main loop can never stall the stack. The main loop calls
//! [`Ble::publish`] / [`Ble::notify`] which are plain attribute writes.
//!
//! Security: commands and WiFi credentials sit behind `WriteEncrypted`, so
//! the first write from a new phone triggers "Just Works" LE pairing
//! (iOS shows its pairing sheet, then retries the write transparently).
//! Telemetry reads stay open — nothing secret in them.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use enumset::{enum_set, EnumSet};
use esp_idf_hal::modem::BluetoothModem;
use esp_idf_svc::bt::ble::gap::{
    AdvConfiguration, AuthenticationRequest, BleGapEvent, EspBleGap, IOCapabilities, KeyMask,
    SecurityConfiguration,
};
use esp_idf_svc::bt::ble::gatt::server::{ConnectionId, EspGatts, GattsEvent};
use esp_idf_svc::bt::ble::gatt::{
    AutoResponse, GattCharacteristic, GattDescriptor, GattId, GattInterface, GattServiceId,
    GattStatus, Handle, Permission, Property,
};
use esp_idf_svc::bt::{BdAddr, Ble as BleMode, BtDriver, BtStatus, BtUuid};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{EspError, ESP_FAIL};
use log::{info, warn};

use w230_core::ble_proto::{self as proto, Command, WifiCredentials, MAX_ATTR_LEN};

const APP_ID: u16 = 0;
const MAX_CONNECTIONS: usize = 2;
/// Service handle budget: 1 (service) + 2 per characteristic + 1 per CCCD.
const SERVICE_HANDLES: u16 = 48;

/// Which attribute the main loop is addressing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Attr {
    Live,
    DeviceInfo,
    BlackBox,
    Calibration,
    Hist0,
    Hist1,
    Events,
    Command,
    WifiConfig,
    WifiStatus,
    OtaStatus,
    Settings,
}

struct CharDef {
    attr: Attr,
    slot: u16,
    read: bool,
    write: bool,
    notify: bool,
    encrypted: bool,
    max_len: usize,
}

const CHARS: [CharDef; 12] = [
    CharDef {
        attr: Attr::Live,
        slot: proto::SLOT_LIVE,
        read: true,
        write: false,
        notify: true,
        encrypted: false,
        max_len: proto::LiveStatus::LEN,
    },
    CharDef {
        attr: Attr::DeviceInfo,
        slot: proto::SLOT_DEVICE_INFO,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::BlackBox,
        slot: proto::SLOT_BLACK_BOX,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::Calibration,
        slot: proto::SLOT_CALIBRATION,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::Hist0,
        slot: proto::SLOT_HIST_0,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::Hist1,
        slot: proto::SLOT_HIST_1,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::Events,
        slot: proto::SLOT_EVENTS,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::Command,
        slot: proto::SLOT_COMMAND,
        read: false,
        write: true,
        notify: false,
        encrypted: true,
        max_len: 256,
    },
    CharDef {
        attr: Attr::WifiConfig,
        slot: proto::SLOT_WIFI_CONFIG,
        read: false,
        write: true,
        notify: false,
        encrypted: true,
        max_len: 128,
    },
    CharDef {
        attr: Attr::WifiStatus,
        slot: proto::SLOT_WIFI_STATUS,
        read: true,
        write: false,
        notify: true,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::OtaStatus,
        slot: proto::SLOT_OTA_STATUS,
        read: true,
        write: false,
        notify: true,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
    CharDef {
        attr: Attr::Settings,
        slot: proto::SLOT_SETTINGS,
        read: true,
        write: false,
        notify: false,
        encrypted: false,
        max_len: MAX_ATTR_LEN,
    },
];

fn char_index(attr: Attr) -> usize {
    CHARS.iter().position(|c| c.attr == attr).unwrap()
}

type Driver = BtDriver<'static, BleMode>;
type Gap = Arc<EspBleGap<'static, BleMode, Arc<Driver>>>;
type Gatts = Arc<EspGatts<'static, BleMode, Arc<Driver>>>;

#[derive(Debug, Clone)]
struct Connection {
    peer: BdAddr,
    conn_id: ConnectionId,
    /// Bit i set = subscribed to notifications of CHARS[i].
    subscribed: u16,
    mtu: u16,
}

#[derive(Default)]
struct State {
    gatt_if: Option<GattInterface>,
    service_handle: Option<Handle>,
    handles: [Option<Handle>; CHARS.len()],
    cccd: [Option<Handle>; CHARS.len()],
    /// Index of the characteristic whose add is in flight (they must be
    /// added strictly one at a time so each CCCD lands under its own char).
    pending: usize,
    ready: bool,
    adv_parts_configured: u8,
    connections: Vec<Connection>,
}

struct Inner {
    gap: Gap,
    gatts: Gatts,
    state: Mutex<State>,
    cmd_tx: SyncSender<Command>,
    wifi_tx: SyncSender<WifiCredentials>,
    _driver: Arc<Driver>,
}

/// Handle owned by the main loop.
pub struct Ble {
    inner: Arc<Inner>,
    pub commands: Receiver<Command>,
    pub wifi_credentials: Receiver<WifiCredentials>,
}

impl Ble {
    /// Bring up the Bluedroid stack and register the service. Advertising
    /// starts as soon as the stack reports the service built.
    pub fn start(modem: BluetoothModem, nvs: EspDefaultNvsPartition) -> anyhow::Result<Ble> {
        let driver = Arc::new(BtDriver::<BleMode>::new(modem, Some(nvs))?);
        let gap: Gap = Arc::new(EspBleGap::new(driver.clone())?);
        let gatts: Gatts = Arc::new(EspGatts::new(driver.clone())?);
        let (cmd_tx, commands) = sync_channel(8);
        let (wifi_tx, wifi_credentials) = sync_channel(2);
        let inner = Arc::new(Inner {
            gap,
            gatts,
            state: Mutex::new(State::default()),
            cmd_tx,
            wifi_tx,
            _driver: driver,
        });

        // Just-Works bonding with encryption: enough to keep casual writers
        // out of the command/WiFi characteristics, no passkey UI needed.
        inner.gap.set_security_conf(&SecurityConfiguration {
            auth_req_mode: AuthenticationRequest::SecureBonding,
            io_capabilities: IOCapabilities::NoInputNoOutput,
            initiator_key: Some(KeyMask::EncryptionKey | KeyMask::IdentityResolvingKey),
            responder_key: Some(KeyMask::EncryptionKey | KeyMask::IdentityResolvingKey),
            max_key_size: Some(16),
            min_key_size: None,
            static_passkey: None,
            only_accept_specified_auth: false,
            enable_oob: false,
        })?;
        // esp-idf-svc 0.51 skips the auth-req parameter in set_security_conf;
        // without it the stack pairs without bonding and re-pairs every time.
        {
            let auth_req: u8 = AuthenticationRequest::SecureBonding as u8;
            esp_idf_svc::sys::esp!(unsafe {
                esp_idf_svc::sys::esp_ble_gap_set_security_param(
                    esp_idf_svc::sys::esp_ble_sm_param_t_ESP_BLE_SM_AUTHEN_REQ_MODE,
                    &auth_req as *const u8 as *mut core::ffi::c_void,
                    1,
                )
            })?;
        }
        // Long attribute values (JSON up to 512 B) in as few ATT round trips
        // as the phone allows; iOS negotiates 185.
        esp_idf_svc::sys::esp!(unsafe { esp_idf_svc::sys::esp_ble_gatt_set_local_mtu(500) })?;

        let gap_inner = inner.clone();
        inner.gap.subscribe(move |event| {
            if let Err(e) = gap_inner.on_gap_event(event) {
                warn!("BLE gap: {e:?}");
            }
        })?;
        let gatts_inner = inner.clone();
        inner.gatts.subscribe(move |(gatt_if, event)| {
            if let Err(e) = gatts_inner.on_gatts_event(gatt_if, event) {
                warn!("BLE gatts: {e:?}");
            }
        })?;
        inner.gatts.register_app(APP_ID)?;
        info!("BLE: stack up, registering service");

        Ok(Ble {
            inner,
            commands,
            wifi_credentials,
        })
    }

    /// Replace an attribute's value (what the next read returns). Silently
    /// a no-op until the service is built.
    pub fn publish(&self, attr: Attr, value: &[u8]) {
        let handle = {
            let st = self.inner.state.lock().unwrap();
            if !st.ready {
                return;
            }
            st.handles[char_index(attr)]
        };
        if let Some(h) = handle {
            let value = &value[..value.len().min(CHARS[char_index(attr)].max_len)];
            if let Err(e) = self.inner.gatts.set_attr(h, value) {
                warn!("BLE: set_attr {attr:?} failed: {e:?}");
            }
        }
    }

    /// Publish and push to every subscriber. Payloads beyond MTU-3 are
    /// truncated by the stack, so the app treats notifications of the JSON
    /// attributes as "changed, re-read" and only trusts the live packet.
    pub fn notify(&self, attr: Attr, value: &[u8]) {
        self.publish(attr, value);
        let idx = char_index(attr);
        let targets: Vec<(GattInterface, ConnectionId, Handle, usize)> = {
            let st = self.inner.state.lock().unwrap();
            let (Some(gatt_if), Some(h)) = (st.gatt_if, st.handles[idx]) else {
                return;
            };
            st.connections
                .iter()
                .filter(|c| c.subscribed & (1 << idx) != 0)
                .map(|c| (gatt_if, c.conn_id, h, c.mtu.saturating_sub(3) as usize))
                .collect()
        };
        for (gatt_if, conn_id, h, max) in targets {
            let v = &value[..value.len().min(max.max(20))];
            if let Err(e) = self.inner.gatts.notify(gatt_if, conn_id, h, v) {
                warn!("BLE: notify {attr:?} failed: {e:?}");
            }
        }
    }

    pub fn connections(&self) -> u8 {
        self.inner.state.lock().unwrap().connections.len() as u8
    }

    /// True when at least one phone subscribed to the live packet — the
    /// main loop skips encoding telemetry nobody is listening to.
    pub fn live_subscribed(&self) -> bool {
        let idx = char_index(Attr::Live);
        self.inner
            .state
            .lock()
            .unwrap()
            .connections
            .iter()
            .any(|c| c.subscribed & (1 << idx) != 0)
    }
}

impl Inner {
    fn on_gap_event(&self, event: BleGapEvent) -> Result<(), EspError> {
        match event {
            BleGapEvent::AdvertisingConfigured(status)
            | BleGapEvent::ScanResponseConfigured(status) => {
                check_bt(status)?;
                let mut st = self.state.lock().unwrap();
                st.adv_parts_configured += 1;
                if st.adv_parts_configured == 2 {
                    drop(st);
                    self.gap.start_advertising()?;
                    info!("BLE: advertising as {}", proto::DEVICE_NAME);
                }
            }
            BleGapEvent::AuthenticationComplete { bd_addr, status } => {
                info!("BLE: pairing with {bd_addr}: {status:?}");
            }
            BleGapEvent::AdvertisingStopped(_) => {
                // Bluedroid stops advertising on connect; keep the door open
                // for a second phone while under the connection cap.
                let n = self.state.lock().unwrap().connections.len();
                if n < MAX_CONNECTIONS && n > 0 {
                    let _ = self.gap.start_advertising();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_gatts_event(&self, gatt_if: GattInterface, event: GattsEvent) -> Result<(), EspError> {
        match event {
            GattsEvent::ServiceRegistered { status, app_id } => {
                check_gatt(status)?;
                if app_id == APP_ID {
                    self.create_service(gatt_if)?;
                }
            }
            GattsEvent::ServiceCreated {
                status,
                service_handle,
                ..
            } => {
                check_gatt(status)?;
                self.state.lock().unwrap().service_handle = Some(service_handle);
                self.gatts.start_service(service_handle)?;
                self.add_char(service_handle, 0)?;
            }
            GattsEvent::CharacteristicAdded {
                status,
                attr_handle,
                service_handle,
                char_uuid,
            } => {
                check_gatt(status)?;
                let (idx, needs_cccd) = {
                    let mut st = self.state.lock().unwrap();
                    let idx = st.pending;
                    if char_uuid != BtUuid::uuid128(proto::uuid(CHARS[idx].slot)) {
                        warn!("BLE: unexpected characteristic added at {idx}");
                    }
                    st.handles[idx] = Some(attr_handle);
                    (idx, CHARS[idx].notify)
                };
                if needs_cccd {
                    self.gatts.add_descriptor(
                        service_handle,
                        &GattDescriptor {
                            uuid: BtUuid::uuid16(0x2902),
                            permissions: enum_set!(Permission::Read | Permission::Write),
                        },
                    )?;
                } else {
                    self.advance(service_handle, idx)?;
                }
            }
            GattsEvent::DescriptorAdded {
                status,
                attr_handle,
                service_handle,
                ..
            } => {
                check_gatt(status)?;
                let idx = {
                    let mut st = self.state.lock().unwrap();
                    let idx = st.pending;
                    st.cccd[idx] = Some(attr_handle);
                    idx
                };
                self.advance(service_handle, idx)?;
            }
            GattsEvent::Mtu { conn_id, mtu } => {
                let mut st = self.state.lock().unwrap();
                if let Some(c) = st.connections.iter_mut().find(|c| c.conn_id == conn_id) {
                    c.mtu = mtu;
                }
            }
            GattsEvent::PeerConnected { conn_id, addr, .. } => {
                let accepted = {
                    let mut st = self.state.lock().unwrap();
                    if st.connections.len() < MAX_CONNECTIONS {
                        st.connections.push(Connection {
                            peer: addr,
                            conn_id,
                            subscribed: 0,
                            mtu: 23,
                        });
                        true
                    } else {
                        false
                    }
                };
                info!("BLE: {addr} connected (accepted={accepted})");
                if accepted {
                    // 30–50 ms interval: 4 Hz telemetry with margin, and inside
                    // Apple's accessory guidelines (≥15 ms, multiple of 15).
                    self.gap.set_conn_params_conf(addr, 24, 40, 0, 400)?;
                }
            }
            GattsEvent::PeerDisconnected { addr, reason, .. } => {
                {
                    let mut st = self.state.lock().unwrap();
                    st.connections.retain(|c| c.peer != addr);
                }
                info!("BLE: {addr} disconnected ({reason:?})");
                self.gap.start_advertising()?;
            }
            // Prepared (long) writes are assembled by the stack into the
            // attribute value; ExecWrite below reads the result.
            GattsEvent::Write {
                conn_id,
                handle,
                offset,
                is_prep: false,
                value,
                ..
            } => self.on_write(conn_id, handle, offset, value),
            GattsEvent::ExecWrite {
                canceled: false, ..
            } => {
                for attr in [Attr::Command, Attr::WifiConfig] {
                    let h = self.state.lock().unwrap().handles[char_index(attr)];
                    if let Some(h) = h {
                        let mut buf = [0u8; 256];
                        if let Ok(n) = self.gatts.get_attr(h, &mut buf) {
                            if n > 0 {
                                self.dispatch_write(attr, &buf[..n]);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn create_service(&self, gatt_if: GattInterface) -> Result<(), EspError> {
        self.state.lock().unwrap().gatt_if = Some(gatt_if);
        self.gap.set_device_name(proto::DEVICE_NAME)?;
        // Flags + 128-bit service UUID already fill 21 of the 31 advertising
        // bytes, so the name rides in the scan response.
        self.gap.set_adv_conf(&AdvConfiguration {
            include_name: false,
            include_txpower: false,
            flag: 6, // LE general discoverable + BR/EDR not supported
            service_uuid: Some(BtUuid::uuid128(proto::uuid(proto::SLOT_SERVICE))),
            min_interval: 0x00A0, // 100 ms
            max_interval: 0x0140, // 200 ms
            ..Default::default()
        })?;
        self.gap.set_adv_conf(&AdvConfiguration {
            set_scan_rsp: true,
            include_name: true,
            ..Default::default()
        })?;
        self.gatts.create_service(
            gatt_if,
            &GattServiceId {
                id: GattId {
                    uuid: BtUuid::uuid128(proto::uuid(proto::SLOT_SERVICE)),
                    inst_id: 0,
                },
                is_primary: true,
            },
            SERVICE_HANDLES,
        )?;
        Ok(())
    }

    fn add_char(&self, service_handle: Handle, idx: usize) -> Result<(), EspError> {
        let def = &CHARS[idx];
        let mut permissions: EnumSet<Permission> = EnumSet::empty();
        let mut properties: EnumSet<Property> = EnumSet::empty();
        if def.read {
            permissions |= Permission::Read;
            properties |= Property::Read;
        }
        if def.write {
            permissions |= if def.encrypted {
                Permission::WriteEncrypted
            } else {
                Permission::Write
            };
            properties |= Property::Write;
        }
        if def.notify {
            properties |= Property::Notify;
        }
        self.state.lock().unwrap().pending = idx;
        self.gatts.add_characteristic(
            service_handle,
            &GattCharacteristic {
                uuid: BtUuid::uuid128(proto::uuid(def.slot)),
                permissions,
                properties,
                max_len: def.max_len,
                auto_rsp: AutoResponse::ByGatt,
            },
            &[],
        )
    }

    fn advance(&self, service_handle: Handle, done_idx: usize) -> Result<(), EspError> {
        let next = done_idx + 1;
        if next < CHARS.len() {
            self.add_char(service_handle, next)
        } else {
            self.state.lock().unwrap().ready = true;
            info!("BLE: service built ({} characteristics)", CHARS.len());
            Ok(())
        }
    }

    fn on_write(&self, conn_id: ConnectionId, handle: Handle, offset: u16, value: &[u8]) {
        let mut st = self.state.lock().unwrap();
        if let Some(i) = st.cccd.iter().position(|h| *h == Some(handle)) {
            if offset == 0 && value.len() == 2 {
                let on = value[0] & 0x01 != 0;
                if let Some(c) = st.connections.iter_mut().find(|c| c.conn_id == conn_id) {
                    if on {
                        c.subscribed |= 1 << i;
                    } else {
                        c.subscribed &= !(1 << i);
                    }
                    info!(
                        "BLE: {} notifications {}",
                        CHARS[i].attr_name(),
                        if on { "on" } else { "off" }
                    );
                }
            }
            return;
        }
        let attr = CHARS
            .iter()
            .enumerate()
            .find(|(i, _)| st.handles[*i] == Some(handle))
            .map(|(_, d)| d.attr);
        drop(st);
        match attr {
            Some(a @ (Attr::Command | Attr::WifiConfig)) if offset == 0 => {
                self.dispatch_write(a, value)
            }
            _ => {}
        }
    }

    fn dispatch_write(&self, attr: Attr, value: &[u8]) {
        match attr {
            Attr::Command => match Command::decode(value) {
                Some(cmd) => {
                    info!("BLE: command {cmd:?}");
                    if let Err(TrySendError::Full(_)) = self.cmd_tx.try_send(cmd) {
                        warn!("BLE: command queue full, dropped");
                    }
                }
                None => warn!("BLE: undecodable command {value:02X?}"),
            },
            Attr::WifiConfig => match WifiCredentials::decode(value) {
                Some(c) => {
                    info!("BLE: WiFi credentials for '{}' received", c.ssid);
                    let _ = self.wifi_tx.try_send(c);
                }
                None => warn!("BLE: undecodable WiFi config ({} bytes)", value.len()),
            },
            _ => {}
        }
        // Clear so a later ExecWrite can't replay a stale value.
        if let Some(h) = self.state.lock().unwrap().handles[char_index(attr)] {
            let _ = self.gatts.set_attr(h, &[]);
        }
    }
}

impl CharDef {
    fn attr_name(&self) -> &'static str {
        match self.attr {
            Attr::Live => "live",
            Attr::DeviceInfo => "device-info",
            Attr::BlackBox => "black-box",
            Attr::Calibration => "calibration",
            Attr::Hist0 => "hist-0",
            Attr::Hist1 => "hist-1",
            Attr::Events => "events",
            Attr::Command => "command",
            Attr::WifiConfig => "wifi-config",
            Attr::WifiStatus => "wifi-status",
            Attr::OtaStatus => "ota-status",
            Attr::Settings => "settings",
        }
    }
}

fn check_bt(status: BtStatus) -> Result<(), EspError> {
    if matches!(status, BtStatus::Success) {
        Ok(())
    } else {
        warn!("BLE: bt status {status:?}");
        Err(EspError::from_infallible::<ESP_FAIL>())
    }
}

fn check_gatt(status: GattStatus) -> Result<(), EspError> {
    if matches!(status, GattStatus::Ok) {
        Ok(())
    } else {
        warn!("BLE: gatt status {status:?}");
        Err(EspError::from_infallible::<ESP_FAIL>())
    }
}
