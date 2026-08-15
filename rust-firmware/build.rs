fn main() {
    embuild::espidf::sysenv::output();

    // Capture build-time epoch seconds so the firmware can seed PCF8563 on
    // first boot (when the coin cell is missing or drained and the VL bit is
    // asserted). The build script reruns whenever source files change, so a
    // rebuild will refresh this value automatically.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=BUILD_EPOCH_SECS={now}");
    println!("cargo:rerun-if-changed=build.rs");
}
