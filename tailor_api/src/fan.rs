#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct FanProfilePoint {
    pub temp: u8,
    pub fan: u8,
    #[serde(default)]
    /// value to write to `/sys/class/thermal/cooling_device16/cur_state`
    pub power_limit: u8,
}
