use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};

use crate::display::ButtonCounts;

const NAMESPACE: &str = "inkpaper";
const KEY_ENTER: &str = "counter_enter";
const KEY_UP: &str = "counter_up";
const KEY_DOWN: &str = "counter_down";

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
}
