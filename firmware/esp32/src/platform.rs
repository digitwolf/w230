//! Safe wrappers around the handful of ESP-IDF calls the firmware needs
//! outside the driver crates. Every `unsafe` block in the application lives
//! here, next to the reason it is sound, so the rest of the code stays
//! `unsafe`-free and reviewable.

use std::ffi::CStr;
use std::sync::OnceLock;

use esp_idf_svc::sys;

/// Free heap in bytes.
pub fn free_heap() -> u32 {
    // SAFETY: pure query with no preconditions; safe from any task.
    unsafe { sys::esp_get_free_heap_size() }
}

/// Seconds since boot (from the high-resolution timer).
pub fn uptime_s() -> u32 {
    // SAFETY: esp_timer is initialised before app_main; the call has no
    // preconditions and never fails.
    (unsafe { sys::esp_timer_get_time() } / 1_000_000) as u32
}

/// Hardware reset reason of this boot (`esp_reset_reason_t` as a number).
pub fn reset_reason() -> u32 {
    // SAFETY: no preconditions; returns an enum value.
    unsafe { sys::esp_reset_reason() as u32 }
}

/// Linked ESP-IDF version, e.g. `v5.3.3`.
pub fn idf_version() -> &'static str {
    static VER: OnceLock<String> = OnceLock::new();
    VER.get_or_init(|| {
        // SAFETY: esp_get_idf_version returns a pointer to a static,
        // NUL-terminated string that lives for the whole program.
        let p = unsafe { sys::esp_get_idf_version() };
        if p.is_null() {
            return "unknown".into();
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    })
}

/// Bluetooth MAC address as `AA:BB:CC:DD:EE:FF`.
pub fn bt_mac() -> String {
    let mut mac = [0u8; 6];
    // SAFETY: the buffer is exactly the 6 bytes esp_read_mac writes for a
    // MAC type; the call only fails for invalid types, and ESP_MAC_BT is valid.
    unsafe {
        sys::esp_read_mac(mac.as_mut_ptr(), sys::esp_mac_type_t_ESP_MAC_BT);
    }
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Project name stamped into the running image's app descriptor
/// (`esp_app_desc_t`). Read as a bounded byte array, never as a C string:
/// the field is 32 bytes and a full-width name would have no terminator.
pub fn app_project_name() -> String {
    // SAFETY: esp_app_get_description returns a pointer to the descriptor
    // in the running image's read-only data, valid for the whole program
    // (or null, which is checked).
    let d = unsafe { sys::esp_app_get_description() };
    if d.is_null() {
        return String::new();
    }
    // c_char is u8 on Xtensa but i8 elsewhere; the cast keeps this portable.
    #[allow(clippy::unnecessary_cast)]
    let raw: [u8; 32] = unsafe { (*d).project_name }.map(|c| c as u8);
    let len = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..len]).into_owned()
}

/// Lock a mutex, recovering the data if another thread panicked while
/// holding it. The state behind every mutex here is plain data that stays
/// consistent per-field, so continuing is safer than cascading the panic
/// into the Bluetooth task or the gear loop.
pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
