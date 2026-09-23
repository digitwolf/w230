fn main() {
    embuild::espidf::sysenv::output();

    // The OTA path checks the downloaded image's embedded app descriptor
    // against the manifest version, and that descriptor is stamped from
    // CONFIG_APP_PROJECT_VER in sdkconfig.defaults. Keep it in lock-step with
    // Cargo's package version so a release can't advertise one number and
    // carry another.
    let defaults = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sdkconfig.defaults");
    println!("cargo:rerun-if-changed={}", defaults.display());
    let text = std::fs::read_to_string(&defaults).expect("read sdkconfig.defaults");
    let configured = text
        .lines()
        .find_map(|l| l.strip_prefix("CONFIG_APP_PROJECT_VER="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .expect("CONFIG_APP_PROJECT_VER missing from sdkconfig.defaults");
    let cargo = env!("CARGO_PKG_VERSION");
    assert_eq!(
        configured, cargo,
        "sdkconfig.defaults CONFIG_APP_PROJECT_VER ({configured}) must equal esp32/Cargo.toml version ({cargo})"
    );
}
