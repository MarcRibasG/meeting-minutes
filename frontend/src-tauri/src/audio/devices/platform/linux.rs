use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};

use crate::audio::devices::configuration::{AudioDevice, DeviceType};

/// Configure Linux audio devices using ALSA/PulseAudio
pub fn configure_linux_audio(host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();
    let mut monitor_devices = Vec::new();

    // Add input devices (but filter out monitors from regular inputs)
    if let Ok(input_devs) = host.input_devices() {
        for device in input_devs {
            if let Ok(name) = device.name() {
                let name_lower = name.to_lowercase();
                // Separate monitor devices from regular inputs
                if name_lower.contains("monitor") {
                    // Create a nicer display name for monitors
                    let display_name = if name_lower.contains("analog") {
                        "Built-in Audio (System Audio)".to_string()
                    } else {
                        format!("{} (System Audio)", name)
                    };
                    monitor_devices.push(AudioDevice::new(display_name, DeviceType::Output));
                } else {
                    devices.push(AudioDevice::new(name, DeviceType::Input));
                }
            }
        }
    }

    // Add all regular input devices first
    // Then add monitor devices as system audio sources
    devices.extend(monitor_devices);

    Ok(devices)
}