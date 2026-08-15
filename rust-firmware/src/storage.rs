use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};

use crate::display::ButtonCounts;

const NAMESPACE: &str = "inkpaper";
const KEY_ENTER: &str = "counter_enter";
const KEY_UP: &str = "counter_up";
const KEY_DOWN: &str = "counter_down";
const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";

/// Maximum length of the NVS strings used for Wi-Fi credentials.
/// ESP32-S3 NVS limits a single string item to ~4000 bytes; 64 chars is
/// plenty for an SSID and WPA2 passphrase.
const WIFI_CRED_MAX_LEN: usize = 64;

#[derive(Clone, Debug)]
pub struct WifiCreds {
    pub ssid: String,
    pub password: String,
}

pub struct PersistedCounters {
    nvs: EspDefaultNvs,
}

impl PersistedCounters {
    pub fn open() -> Result<Self> {
        let partition = EspDefaultNvsPartition::take()
            .map_err(|e| anyhow!("failed to initialise default NVS partition: {e}"))?;
        let nvs = EspDefaultNvs::new(partition, NAMESPACE, true)
            .map_err(|e| anyhow!("failed to open NVS namespace '{NAMESPACE}': {e}"))?;
        Ok(Self { nvs })
    }

    pub fn load(&self) -> Result<ButtonCounts> {
        Ok(ButtonCounts {
            enter: self.nvs.get_u32(KEY_ENTER)?.unwrap_or(0),
            up: self.nvs.get_u32(KEY_UP)?.unwrap_or(0),
            down: self.nvs.get_u32(KEY_DOWN)?.unwrap_or(0),
        })
    }

    pub fn save(&self, counts: &ButtonCounts) -> Result<()> {
        self.nvs
            .set_u32(KEY_ENTER, counts.enter)
            .map_err(|e| anyhow!("NVS set_u32({KEY_ENTER}) failed: {e}"))?;
        self.nvs
            .set_u32(KEY_UP, counts.up)
            .map_err(|e| anyhow!("NVS set_u32({KEY_UP}) failed: {e}"))?;
        self.nvs
            .set_u32(KEY_DOWN, counts.down)
            .map_err(|e| anyhow!("NVS set_u32({KEY_DOWN}) failed: {e}"))?;
        Ok(())
    }

    /// Reads the Wi-Fi credentials stored in NVS, if any.
    pub fn wifi_creds(&self) -> Result<Option<WifiCreds>> {
        let mut ssid_buf = [0u8; WIFI_CRED_MAX_LEN];
        let ssid = match self
            .nvs
            .get_str(KEY_WIFI_SSID, &mut ssid_buf)
            .map_err(|e| anyhow!("NVS get_str({KEY_WIFI_SSID}) failed: {e}"))?
        {
            Some(s) => s.to_owned(),
            None => return Ok(None),
        };
        let mut pass_buf = [0u8; WIFI_CRED_MAX_LEN];
        let password = self
            .nvs
            .get_str(KEY_WIFI_PASS, &mut pass_buf)
            .map_err(|e| anyhow!("NVS get_str({KEY_WIFI_PASS}) failed: {e}"))?
            .unwrap_or("")
            .to_owned();
        Ok(Some(WifiCreds { ssid, password }))
    }

    /// Stores the Wi-Fi credentials in NVS for later connections.
    pub fn save_wifi_creds(&self, creds: &WifiCreds) -> Result<()> {
        self.nvs
            .set_str(KEY_WIFI_SSID, &creds.ssid)
            .map_err(|e| anyhow!("NVS set_str({KEY_WIFI_SSID}) failed: {e}"))?;
        self.nvs
            .set_str(KEY_WIFI_PASS, &creds.password)
            .map_err(|e| anyhow!("NVS set_str({KEY_WIFI_PASS}) failed: {e}"))?;
        Ok(())
    }
}
