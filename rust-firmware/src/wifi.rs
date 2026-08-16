use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::sntp::{EspSntp, SntpConf, SyncStatus};
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::wifi::{AccessPointInfo, AuthMethod, ClientConfiguration, Configuration, EspWifi};
use heapless::String as HeaplessString;

use crate::rtc::{DateTime, Pcf8563};
use crate::storage::WifiCreds;
use crate::watchdog;

/// How long to wait for the STA link to come up before giving up.
const WIFI_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to wait for the first NTP sync to complete.
const NTP_SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// Owns the *one* `EspWifi` driver instance for the whole process. Create
/// this exactly once (`main.rs` does so right after taking `sysloop`) and
/// thread `&mut WifiManager` everywhere Wi-Fi is needed - boot-time sync,
/// the on-device setup wizard, and `sync::sync_now`.
///
/// **A given boot session may only successfully `connect()` once.** This
/// was originally a design where each caller created its own
/// `EspWifi::new(unsafe { Peripherals::steal() }.modem, ...)` and dropped
/// it when done; that crashed on a second connect
/// (`Guru Meditation Error: InstrFetchProhibited`). Switching to this one
/// shared instance (avoiding a second `esp_wifi_init`) still crashed with
/// the same signature on a second `esp_wifi_start()` after
/// `esp_wifi_stop()`. Removing `stop()` from `disconnect()` (so `start()`
/// is only ever called once) *still* crashed, this time with a different
/// signature (`Unhandled debug exception`, `BREAK instr`) partway through
/// the second connection's lifecycle. All three variants reproduced
/// reliably on real hardware, including with over a minute between the
/// first disconnect and the second connect attempt (ruling out an
/// async-teardown timing race). This matches a known, unresolved class of
/// upstream bugs - see e.g. espressif/esp-idf#7579 (netif rx-callback race
/// on STA restart), espressif/esp-idf#11171 (`esp_wifi_stop()` doesn't
/// fully quiesce the driver), and esp-rs/esp-idf-svc#503 (reusing one
/// `EspWifi` for a second `connect()` panics in the driver's own state
/// machine) - none with a confirmed fix in this ESP-IDF/esp-idf-svc
/// version.
///
/// `connect`/`disconnect` below call raw `esp_wifi_connect()`/
/// `esp_wifi_disconnect()` FFI directly rather than going through
/// `EspWifi`'s own `connect()`/`is_connected()`, on the theory that the
/// esp-idf-svc#503 panic meant the crash was specific to the Rust
/// wrapper's own event-driven status tracking. **That theory was tested
/// and disproved**: a real second connect via the raw calls crashes
/// identically (same PC address, `Unhandled debug exception` at
/// `0x4037e864`) with or without the wrapper involved, so this is a
/// lower-level ESP-IDF/hardware issue, not an esp-idf-svc-specific one.
/// (github.com/qiujun8023/slate, a working C++ reference firmware for
/// this exact hardware, *does* reconnect repeatedly via these same raw
/// calls without crashing - but it does so synchronously from inside its
/// `WIFI_EVENT_STA_DISCONNECTED` event handler, immediately on
/// disconnect, not from arbitrary application code potentially much
/// later; that context/timing difference is the leading remaining
/// hypothesis for why theirs works and this doesn't, but reproducing it
/// would mean restructuring this module around the event loop rather
/// than synchronous calls, which hasn't been attempted.) The raw FFI
/// calls are kept anyway since they're no worse than the wrapper and
/// match Slate's proven-working low-level sequence.
///
/// The only way found to reliably get a *working* second Wi-Fi connection
/// is a full software reboot between uses, so every connect attempt is
/// the guaranteed-safe first one of its boot session - see
/// `used`/`restart_for_fresh_wifi_session`.
pub struct WifiManager {
    wifi: EspWifi<'static>,
    used: bool,
}

impl WifiManager {
    /// Takes the modem peripheral (via `Peripherals::steal()` - safe here
    /// because this is called exactly once, immediately after `sysloop`
    /// exists and before anything else touches Wi-Fi) and brings up the
    /// driver in STA mode, disconnected. Call `connect`/`disconnect`/`scan`
    /// afterward as needed; this instance lives for the rest of the
    /// program.
    pub fn new(sysloop: &EspSystemEventLoop) -> Result<Self> {
        let peripherals = unsafe { Peripherals::steal() };
        let wifi = EspWifi::new(peripherals.modem, sysloop.clone(), None)
            .context("failed to create Wi-Fi driver with netif")?;
        Ok(Self { wifi, used: false })
    }

    /// Whether `connect()` has already succeeded once this boot session.
    /// Every caller other than the very first boot-time connect must check
    /// this before calling `connect()` again - see the struct doc comment
    /// - and call `restart_for_fresh_wifi_session()` instead if it's
    /// `true`.
    pub fn used(&self) -> bool {
        self.used
    }

    /// Connects to `creds`, waiting for the STA link and a DHCP lease
    /// (netif up) before returning. Leaves the driver connected - call
    /// `disconnect` when done, mirroring the previous per-call
    /// connect-then-drop lifecycle but without tearing down the driver
    /// itself.
    pub fn connect(&mut self, creds: &WifiCreds) -> Result<()> {
        // Match the reference demo: keep the Wi-Fi config in RAM rather than
        // in NVS so a fresh session starts from the configuration we set here.
        esp_idf_svc::sys::esp!(unsafe {
            esp_idf_svc::sys::esp_wifi_set_storage(
                esp_idf_svc::sys::wifi_storage_t_WIFI_STORAGE_RAM,
            )
        })
        .map_err(|e| anyhow!("esp_wifi_set_storage failed: {e:?}"))?;

        let ssid: HeaplessString<32> = HeaplessString::try_from(creds.ssid.as_str())
            .map_err(|_| anyhow!("SSID longer than 32 characters"))?;
        let password: HeaplessString<64> = HeaplessString::try_from(creds.password.as_str())
            .map_err(|_| anyhow!("Wi-Fi password longer than 64 characters"))?;

        self.wifi
            .set_configuration(&Configuration::Client(ClientConfiguration {
                ssid,
                password,
                auth_method: if creds.password.is_empty() {
                    AuthMethod::None
                } else {
                    AuthMethod::WPA2Personal
                },
                ..Default::default()
            }))
            .context("failed to set Wi-Fi station configuration")?;

        if !self.wifi.is_started().unwrap_or(false) {
            self.wifi.start().context("failed to start Wi-Fi")?;
        }

        // Manual scan before connecting. If this finds APs but the internal
        // connect scan does not, the radio path is fine and the problem is in
        // the connect-time scan/filter.
        match self.wifi.scan() {
            Ok(aps) => {
                log::info!("manual scan: {} APs", aps.len());
                for ap in aps.iter() {
                    log::info!("  {} ch{} rssi {}", ap.ssid, ap.channel, ap.signal_strength);
                }
            }
            Err(err) => log::warn!("manual scan failed: {err:?}"),
        }
        watchdog::feed();

        // Raw FFI from here, bypassing `EspWifi`'s own `connect()`/
        // `is_connected()` - the Rust wrapper tracks STA state via an
        // `Arc<Mutex<WifiDriverStatus>>` fed by its own event-loop
        // subscription, and esp-rs/esp-idf-svc#503 reports that state
        // machine panicking (`Result::unwrap() on Err('Invalid')`) on a
        // second `connect()` on one `EspWifi` - i.e. a bug in the
        // *wrapper's* reuse tracking, not necessarily the underlying
        // ESP-IDF C API. Slate (github.com/qiujun8023/slate, a working
        // reference firmware for this exact hardware) reconnects Wi-Fi
        // repeatedly in one process by calling raw `esp_wifi_connect()`/
        // `esp_wifi_disconnect()` directly - no deinit, no stop/start
        // between reconnects, matching what's left of this function after
        // switching away from the safe wrapper's connect/status calls.
        esp_idf_svc::sys::esp!(unsafe { esp_idf_svc::sys::esp_wifi_connect() })
            .map_err(|e| anyhow!("esp_wifi_connect failed: {e:?}"))?;

        // Raw `esp_wifi_sta_get_ap_info` instead of `self.wifi.is_connected()`
        // - returns ESP_OK once associated, ESP_ERR_WIFI_NOT_CONNECT until
        // then, so this polls the driver's actual state directly rather
        // than the wrapper's derived-from-events status.
        let deadline = (EspSystemTime {}).now() + WIFI_CONNECT_TIMEOUT;
        loop {
            watchdog::feed();
            let mut ap_info: esp_idf_svc::sys::wifi_ap_record_t = unsafe { std::mem::zeroed() };
            if unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap_info) } == 0 {
                log::info!("Wi-Fi connected to '{}'", creds.ssid);
                break;
            }
            if (EspSystemTime {}).now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for Wi-Fi connection to '{}'",
                    creds.ssid
                ));
            }
            thread::sleep(Duration::from_millis(500));
        }

        let netif_deadline = (EspSystemTime {}).now() + Duration::from_secs(10);
        loop {
            watchdog::feed();
            if let Ok(true) = self.wifi.sta_netif().is_up() {
                log::info!("Wi-Fi netif is up (DHCP done)");
                break;
            }
            if (EspSystemTime {}).now() >= netif_deadline {
                return Err(anyhow!("timed out waiting for DHCP lease"));
            }
            thread::sleep(Duration::from_millis(500));
        }

        self.used = true;
        Ok(())
    }

    /// Disconnects from the AP but leaves the STA interface *started*.
    /// Calling `esp_wifi_stop()` here (paired with `connect`'s `start()`
    /// call, matching the natural "undo what connect did" instinct) was
    /// the actual crash trigger, not driver re-init as first suspected:
    /// confirmed by reproducing the exact same
    /// `Guru Meditation Error: InstrFetchProhibited` at `wifi:enable tsf`
    /// (inside `esp_wifi_start()`) on a *second* start-after-stop cycle,
    /// even after switching to this one-shared-instance design. `start()`
    /// is now only ever called once per process (`connect`'s
    /// `is_started()` guard skips it on every later call, since this
    /// method no longer un-starts the interface) - a Wi-Fi radio idling
    /// disconnected costs a bit more power than a stopped one, which is an
    /// acceptable trade against a reliable crash. Safe to call even if not
    /// currently connected.
    ///
    /// Raw `esp_wifi_disconnect()` rather than `self.wifi.disconnect()`,
    /// for the same reason `connect()` bypasses the wrapper - see its
    /// comment.
    pub fn disconnect(&mut self) {
        let ret = unsafe { esp_idf_svc::sys::esp_wifi_disconnect() };
        if ret != 0 {
            log::warn!("esp_wifi_disconnect failed: 0x{ret:x}");
        }
    }

    /// Scans for nearby 2.4 GHz access points and returns them sorted by
    /// signal strength (strongest first), deduplicated by SSID. Used by the
    /// on-device Wi-Fi setup screen so the user picks a network instead of
    /// typing an SSID by hand - the exact step that let a case typo
    /// (`XiaoMi_ED4E` vs the broadcast `Xiaomi_ED4E`) break `connect()` for
    /// a full debugging session (see docs/wifi-connect-issue.md).
    pub fn scan_networks(&mut self) -> Result<Vec<AccessPointInfo>> {
        self.wifi
            .set_configuration(&Configuration::Client(ClientConfiguration::default()))
            .context("failed to set STA mode for scan")?;
        if !self.wifi.is_started().unwrap_or(false) {
            self.wifi
                .start()
                .context("failed to start Wi-Fi for scan")?;
        }
        let mut aps = self.wifi.scan().context("Wi-Fi scan failed")?;
        watchdog::feed();
        aps.sort_by_key(|ap| std::cmp::Reverse(ap.signal_strength));
        aps.dedup_by(|a, b| a.ssid == b.ssid);
        Ok(aps)
    }
}

/// Cleanly restarts the device so the next boot's Wi-Fi connect is a fresh
/// session's first (and therefore safe - see `WifiManager`'s doc comment).
/// Call this instead of `WifiManager::connect()` whenever `used()` is
/// already `true`. Never returns.
///
/// Goes through `power::enter_deep_sleep_with_wakeups` (a near-immediate
/// timer wake) rather than `esp_idf_svc::sys::esp_restart()`. The obvious
/// choice - `esp_restart()` - turned out to hit the *same* crash class
/// this function exists to avoid: even with the Wi-Fi driver never
/// reconnected, calling it right after a Wi-Fi session had been used
/// still crashed (`Guru Meditation Error: Cache error` at
/// `_UserExceptionVector`), reproduced on real hardware after ruling out
/// several other suspects (thread core affinity included). `esp_restart()`
/// evidently performs its own internal Wi-Fi-aware teardown that hits the
/// same unresolved upstream instability - see `WifiManager`'s doc comment
/// for the full saga. Deep sleep, by contrast, has been exercised
/// repeatedly and reliably in this project specifically *after* an
/// already-used Wi-Fi session (every DOWN-hold-3s sleep following a
/// boot-time NTP sync), so it's the proven path here even though a
/// power-cycle is a heavier hammer than a soft reset.
pub fn restart_for_fresh_wifi_session() -> ! {
    log::warn!(
        "Wi-Fi already used this boot session; power-cycling for a fresh (safe) session instead of reconnecting in-process"
    );
    // Best-effort: give any in-flight log/serial output a moment to flush
    // before sleep. Not load-bearing for correctness, just politeness.
    thread::sleep(Duration::from_millis(300));
    crate::power::enter_deep_sleep_with_wakeups(Some(Duration::from_millis(100)))
}

/// Starts the SNTP client, waits for the first sync and pushes the obtained
/// time into the PCF8563 RTC.
pub fn ntp_sync_and_set_rtc(rtc: &mut Pcf8563) -> Result<()> {
    let sntp = EspSntp::new(&SntpConf {
        servers: ["pool.ntp.org", "ntp.aliyun.com"],
        ..Default::default()
    })
    .context("failed to start SNTP client")?;

    let deadline = EspSystemTime {}.now() + NTP_SYNC_TIMEOUT;
    loop {
        watchdog::feed();
        if sntp.get_sync_status() == SyncStatus::Completed {
            break;
        }
        if (EspSystemTime {}).now() >= deadline {
            return Err(anyhow!("timed out waiting for NTP sync"));
        }
        thread::sleep(Duration::from_millis(500));
    }

    let epoch_secs = EspSystemTime {}.now().as_secs();
    let dt = DateTime::from_unix(epoch_secs);
    rtc.write_time(&dt)
        .context("failed to write NTP time to PCF8563")?;
    log::info!(
        "NTP sync OK; RTC set to {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year,
        dt.month,
        dt.day,
        dt.hour,
        dt.minute,
        dt.second
    );
    Ok(())
}
