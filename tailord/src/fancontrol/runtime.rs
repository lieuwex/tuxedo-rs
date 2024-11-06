use crate::suspend::process_suspend;

use super::{buffer::TemperatureBuffer, FanRuntimeData};

use std::time::Duration;
use std::path::Path;
use tokio::io;
use tokio_uring::fs;

async fn rw_file<P>(path: P) -> Result<fs::File, io::Error>
where
    P: AsRef<Path>,
{
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
}

async fn write_buffer<V>(file: &mut fs::File, value: V) -> Result<(), io::Error>
where
    V: tokio_uring::buf::IoBuf,
{
    file.write_at(value, 0).submit().await.0?;
    Ok(())
}

async fn write_string(file: &mut fs::File, string: String) -> Result<(), io::Error> {
    write_buffer(file, string.into_bytes()).await
}
async fn write_int(file: &mut fs::File, int: u32) -> Result<(), io::Error> {
    write_string(file, format!("{}", int)).await
}

impl FanRuntimeData {
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn fan_control_loop(&mut self) {
        let mut powerclamp_file = rw_file("/sys/class/thermal/cooling_device16/cur_state").await.unwrap();
        let mut previous_powerclamp: Option<u8> = None;

        loop {
            // Add the current temperature to history
            let act_current_temp = self.update_temp();
            let current_temp = *self.temp_history.temp_history.iter().min().unwrap();

            let target_fan_speed = self.profile.calc_target_fan_speed(current_temp);
            let fan_diff = self.fan_speed.abs_diff(target_fan_speed);

            // Make small steps to decrease or increase fan speed.
            // If the target fan speed is below 50%, don't increase the speed at all
            // unless the difference is higher than 3% to avoid frequent speed changes
            // at low temperatures.
            let mut fan_increment = fan_diff / 4 + (target_fan_speed / 50);
            if target_fan_speed > self.fan_speed {
                fan_increment = fan_increment.min(3).max(1);
            }

            // Update fan speed
            self.set_speed(if target_fan_speed > self.fan_speed {
                self.fan_speed.saturating_add(fan_increment).min(100)
            } else {
                self.fan_speed.saturating_sub(fan_increment)
            });

            // update intel_powerclamp
            let target_power_limit = self.profile.calc_target_power_limit(act_current_temp);
            if previous_powerclamp.map_or(true, |prev| prev != target_power_limit) {
                if let Err(err) = write_int(&mut powerclamp_file, target_power_limit as u32).await{
                    tracing::error!("Failed setting new power limit: `{err}`");
                }
                previous_powerclamp = Some(target_power_limit);
            }

            //let delay = suitable_delay(&self.temp_history, fan_diff);
            let delay = Duration::from_millis(100);

            tracing::debug!(
                "Fan {}: Current temperature is {act_current_temp}°C, pretending it is {current_temp}°C, fan speed: {}%, target fan speed: {target_fan_speed} \
                fan diff: {fan_diff}, fan increment {fan_increment}, target power_limit: {target_power_limit}, delay: {delay:?}", self.fan_idx, self.fan_speed
            );

            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = process_suspend(&mut self.suspend_receiver) => {
                    self.fan_speed = self.io.get_fan_speed_percent(0).unwrap();
                }
            }
        }
    }
}

/// Calculate a suitable delay to reduce CPU usage.
fn suitable_delay(temp_buffer: &TemperatureBuffer, fan_diff: u8) -> Duration {
    // How much is the temperature changing?
    let temperature_pressure = temp_buffer.diff_to_min_in_history();

    // How much is the fan speed off from the ideal value?
    let fan_diff_pressure = fan_diff / 2;

    // Calculate an overall pressure value from 0 to 15.
    let pressure = temperature_pressure
        .saturating_add(fan_diff_pressure)
        .min(15);

    // Define a falling exponential function with time constant -1/7.
    // This should yield decent results but the formula might be tuned
    // to perform better.
    // 0  -> 2000ms
    // 15 -> ~230ms
    const TAU: f64 = -1.0 / 7.0;
    let delay = 2000.0 * (pressure as f64 * TAU).exp();
    Duration::from_millis(delay as u64)
}

#[cfg(test)]
mod test {
    use crate::fancontrol::buffer::TemperatureBuffer;

    use super::suitable_delay;

    #[test]
    fn test_suitable_delay() {
        let mut temp_buffer = TemperatureBuffer::new(20);

        // Test with no pressure.
        assert_eq!(suitable_delay(&temp_buffer, 0).as_millis(), 2000);

        // Test with max pressure.
        assert_eq!(suitable_delay(&temp_buffer, 255).as_millis(), 234);

        // Test with pressure 1.
        assert_eq!(suitable_delay(&temp_buffer, 2).as_millis(), 1733);

        // Test with pressure 1 but this time through temperature diff.
        temp_buffer.update(21);
        assert_eq!(suitable_delay(&temp_buffer, 0).as_millis(), 1733);
    }
}
