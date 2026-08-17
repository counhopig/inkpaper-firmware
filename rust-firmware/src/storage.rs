use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};

const NAMESPACE: &str = "inkpaper";
const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";
const KEY_SERVER_URL: &str = "server_url";
const KEY_AUTH_TOKEN: &str = "auth_token";
const KEY_SYNC_ETAG: &str = "sync_etag";
const KEY_TIMEZONE_OFFSET: &str = "timezone_min";

/// Maximum length of the NVS strings used for Wi-Fi credentials.
/// ESP32-S3 NVS limits a single string item to ~4000 bytes; 64 chars is
/// plenty for an SSID and WPA2 passphrase.
const WIFI_CRED_MAX_LEN: usize = 64;

/// Maximum length of the NVS strings used for server URL, auth token, and ETag.
/// Server URLs typically fit in ~256 chars, tokens often 128-256 chars, and
/// ETags vary but are rarely over 128 chars.
const SERVER_CONFIG_MAX_LEN: usize = 256;

#[derive(Clone, Debug)]
pub struct WifiCreds {
    pub ssid: String,
    pub password: String,
}

#[derive(Clone, Debug)]
pub struct DeviceConfig {
    pub server_url: String,
    pub auth_token: String,
}

pub struct PersistedCounters {
    nvs: EspDefaultNvs,
}

impl PersistedCounters {
    /// `partition` is a clone of the one shared `EspDefaultNvsPartition`
    /// handle `main.rs` takes once - `EspDefaultNvsPartition::take()` is a
    /// true singleton (guarded by a global taken-flag, not a ref-counted
    /// "take a new handle" call) and errors with `ESP_ERR_INVALID_STATE` if
    /// called again while an earlier handle is still alive, so every NVS
    /// namespace opener in this codebase (`AlarmStore`, `TodoStore`, this
    /// one) must share a single taken partition rather than each calling
    /// `take()` independently.
    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        let nvs = EspDefaultNvs::new(partition, NAMESPACE, true)
            .map_err(|e| anyhow!("failed to open NVS namespace '{NAMESPACE}': {e}"))?;
        Ok(Self { nvs })
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

    /// Reads the server configuration (URL and auth token) from NVS, if any.
    pub fn device_config(&self) -> Result<Option<DeviceConfig>> {
        let mut url_buf = [0u8; SERVER_CONFIG_MAX_LEN];
        let server_url = match self
            .nvs
            .get_str(KEY_SERVER_URL, &mut url_buf)
            .map_err(|e| anyhow!("NVS get_str({KEY_SERVER_URL}) failed: {e}"))?
        {
            Some(s) => s.to_owned(),
            None => return Ok(None),
        };
        let mut token_buf = [0u8; SERVER_CONFIG_MAX_LEN];
        let auth_token = self
            .nvs
            .get_str(KEY_AUTH_TOKEN, &mut token_buf)
            .map_err(|e| anyhow!("NVS get_str({KEY_AUTH_TOKEN}) failed: {e}"))?
            .unwrap_or("")
            .to_owned();
        Ok(Some(DeviceConfig {
            server_url,
            auth_token,
        }))
    }

    /// Stores the server configuration (URL and auth token) in NVS.
    pub fn save_device_config(&self, cfg: &DeviceConfig) -> Result<()> {
        self.nvs
            .set_str(KEY_SERVER_URL, &cfg.server_url)
            .map_err(|e| anyhow!("NVS set_str({KEY_SERVER_URL}) failed: {e}"))?;
        self.nvs
            .set_str(KEY_AUTH_TOKEN, &cfg.auth_token)
            .map_err(|e| anyhow!("NVS set_str({KEY_AUTH_TOKEN}) failed: {e}"))?;
        Ok(())
    }

    /// Reads the last-seen sync ETag from NVS for conditional requests, if any.
    pub fn sync_etag(&self) -> Result<Option<String>> {
        let mut etag_buf = [0u8; SERVER_CONFIG_MAX_LEN];
        match self
            .nvs
            .get_str(KEY_SYNC_ETAG, &mut etag_buf)
            .map_err(|e| anyhow!("NVS get_str({KEY_SYNC_ETAG}) failed: {e}"))?
        {
            Some(s) => Ok(Some(s.to_owned())),
            None => Ok(None),
        }
    }

    /// Stores the sync ETag in NVS for future conditional requests.
    pub fn save_sync_etag(&self, etag: &str) -> Result<()> {
        self.nvs
            .set_str(KEY_SYNC_ETAG, etag)
            .map_err(|e| anyhow!("NVS set_str({KEY_SYNC_ETAG}) failed: {e}"))
    }

    /// Invalidates conditional-sync state after changing server identity.
    pub fn clear_sync_etag(&self) -> Result<()> {
        self.nvs
            .remove(KEY_SYNC_ETAG)
            .map(|_| ())
            .map_err(|e| anyhow!("NVS remove({KEY_SYNC_ETAG}) failed: {e}"))
    }

    pub fn timezone_offset_minutes(&self) -> Result<i16> {
        let mut buf = [0u8; 8];
        let value = self
            .nvs
            .get_str(KEY_TIMEZONE_OFFSET, &mut buf)
            .map_err(|e| anyhow!("NVS get_str({KEY_TIMEZONE_OFFSET}) failed: {e}"))?
            .unwrap_or("0");
        value
            .parse::<i16>()
            .map_err(|e| anyhow!("invalid stored timezone offset '{value}': {e}"))
    }

    pub fn save_timezone_offset_minutes(&self, offset: i16) -> Result<()> {
        if !(-720..=840).contains(&offset) {
            return Err(anyhow!(
                "timezone offset must be between -720 and 840 minutes"
            ));
        }
        self.nvs
            .set_str(KEY_TIMEZONE_OFFSET, &offset.to_string())
            .map_err(|e| anyhow!("NVS set_str({KEY_TIMEZONE_OFFSET}) failed: {e}"))
    }
}
