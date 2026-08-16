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

/// A connected STA interface, kept alive for the lifetime of the caller.
pub struct WifiSta {
    _wifi: EspWifi<'static>,
}

impl WifiSta {
    /// Connects to the given AP as a station, waiting for the link and a
    /// DHCP lease (netif up) before returning.
    pub fn connect(creds: &WifiCreds, sysloop: &EspSystemEventLoop) -> Result<Self> {
        let peripherals = unsafe { Peripherals::steal() };
        let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), None)
            .context("failed to create Wi-Fi driver with netif")?;

        // Match the reference demo: keep the Wi-Fi config in RAM rather than
        // in NVS so a fresh session starts from the configuration we set here.
        esp_idf_svc::sys::esp!(unsafe { esp_idf_svc::sys::esp_wifi_set_storage(
            esp_idf_svc::sys::wifi_storage_t_WIFI_STORAGE_RAM,
        ) })
        .map_err(|e| anyhow!("esp_wifi_set_storage failed: {e:?}"))?;

        let ssid: HeaplessString<32> = HeaplessString::try_from(creds.ssid.as_str())
            .map_err(|_| anyhow!("SSID longer than 32 characters"))?;
        let password: HeaplessString<64> = HeaplessString::try_from(creds.password.as_str())
            .map_err(|_| anyhow!("Wi-Fi password longer than 64 characters"))?;

        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
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

        wifi.start().context("failed to start Wi-Fi")?;

        // Manual scan before connecting. If this finds APs but the internal
        // connect scan does not, the radio path is fine and the problem is in
        // the connect-time scan/filter.
        match wifi.scan() {
            Ok(aps) => {
                log::info!("manual scan: {} APs", aps.len());
                for ap in aps.iter() {
                    log::info!(
                        "  {} ch{} rssi {}",
                        ap.ssid,
                        ap.channel,
                        ap.signal_strength
                    );
                }
            }
            Err(err) => log::warn!("manual scan failed: {err:?}"),
        }
        watchdog::feed();

        wifi.connect()
            .map_err(|e| anyhow!("esp_wifi_connect failed: {e:?}"))?;

        // Poll the driver state directly; if the state machine is stuck we
        // see no `wifi:state` logs below `CONNECT` time and this times out.
        let deadline = (EspSystemTime {}).now() + WIFI_CONNECT_TIMEOUT;
        loop {
            watchdog::feed();
            if let Ok(true) = wifi.is_connected() {
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
            if let Ok(true) = wifi.sta_netif().is_up() {
                log::info!("Wi-Fi netif is up (DHCP done)");
                break;
            }
            if (EspSystemTime {}).now() >= netif_deadline {
                return Err(anyhow!("timed out waiting for DHCP lease"));
            }
            thread::sleep(Duration::from_millis(500));
        }

        Ok(Self { _wifi: wifi })
    }
}

/// Scans for nearby 2.4 GHz access points and returns them sorted by signal
/// strength (strongest first), deduplicated by SSID. Used by the on-device
/// Wi-Fi setup screen so the user picks a network instead of typing an SSID
/// by hand - the exact step that let a case typo (`XiaoMi_ED4E` vs the
/// broadcast `Xiaomi_ED4E`) break `connect()` for a full debugging session
/// (see docs/wifi-connect-issue.md).
pub fn scan_networks(sysloop: &EspSystemEventLoop) -> Result<Vec<AccessPointInfo>> {
    let peripherals = unsafe { Peripherals::steal() };
    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), None)
        .context("failed to create Wi-Fi driver for scan")?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))
        .context("failed to set STA mode for scan")?;
    wifi.start().context("failed to start Wi-Fi for scan")?;
    let mut aps = wifi.scan().context("Wi-Fi scan failed")?;
    watchdog::feed();
    let _ = wifi.stop();
    aps.sort_by_key(|ap| std::cmp::Reverse(ap.signal_strength));
    aps.dedup_by(|a, b| a.ssid == b.ssid);
    Ok(aps)
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
