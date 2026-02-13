use std::pin::Pin;
use std::task::{Context, Poll};
use futures_util::{Stream, StreamExt};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};


#[cfg(target_os = "macos")]
use futures_channel::mpsc;
#[cfg(target_os = "macos")]
use super::core_audio::CoreAudioCapture;
#[cfg(target_os = "macos")]
use log::info;

/// System audio capture using Core Audio tap (macOS) or CPAL (other platforms)
pub struct SystemAudioCapture {
    _host: cpal::Host,
}

impl SystemAudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        Ok(Self { _host: host })
    }

    pub fn list_system_devices() -> Result<Vec<String>> {
        let host = cpal::default_host();
        let devices = host.output_devices()
            .map_err(|e| anyhow::anyhow!("Failed to enumerate output devices: {}", e))?;

        let mut device_names = Vec::new();
        for device in devices {
            if let Ok(name) = device.name() {
                device_names.push(name);
            }
        }

        Ok(device_names)
    }

    pub fn start_system_audio_capture(&self) -> Result<SystemAudioStream> {
        #[cfg(target_os = "macos")]
        {
            info!("Starting Core Audio system capture (macOS)");
            // Use Core Audio tap for system audio capture
            let core_audio = CoreAudioCapture::new()?;
            let core_audio_stream = core_audio.stream()?;
            let sample_rate = core_audio_stream.sample_rate();

            // Convert CoreAudioStream to SystemAudioStream
            let (tx, rx) = mpsc::unbounded::<Vec<f32>>();
            let (drop_tx, drop_rx) = std::sync::mpsc::channel::<()>();

            // Spawn task to forward Core Audio samples
            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut stream = core_audio_stream;
                let mut buffer = Vec::new();
                let chunk_size = 1024;

                loop {
                    // Check if we should stop
                    if drop_rx.try_recv().is_ok() {
                        break;
                    }

                    // Poll the Core Audio stream
                    match stream.next().await {
                        Some(sample) => {
                            buffer.push(sample);
                            if buffer.len() >= chunk_size {
                                if tx.unbounded_send(buffer.clone()).is_err() {
                                    break;
                                }
                                buffer.clear();
                            }
                        }
                        None => break,
                    }
                }

                // Send any remaining samples
                if !buffer.is_empty() {
                    let _ = tx.unbounded_send(buffer);
                }
            });

            let receiver = rx.map(futures_util::stream::iter).flatten();

            info!("Core Audio system capture started successfully");

            Ok(SystemAudioStream {
                drop_tx,
                sample_rate,
                receiver: Box::pin(receiver),
                #[cfg(target_os = "linux")]
                _stream: None,
            })
        }

        #[cfg(target_os = "linux")]
        {
            use log::info;
            
            info!("Starting PulseAudio/PipeWire system audio capture (Linux)");
            
            // Get ALSA host which works with PulseAudio/PipeWire
            let host = cpal::default_host();
            
            // Look for monitor devices
            let mut monitor_device = None;
            if let Ok(devices) = host.input_devices() {
                for device in devices {
                    if let Ok(name) = device.name() {
                        // Monitor devices typically have "monitor" in their name
                        // PulseAudio: "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
                        // PipeWire: similar patterns
                        if name.to_lowercase().contains("monitor") {
                            info!("Found monitor device: {}", name);
                            monitor_device = Some(device);
                            break;
                        }
                    }
                }
            }
            
            let device = monitor_device
                .ok_or_else(|| anyhow::anyhow!("No system audio monitor device found. Make sure PulseAudio/PipeWire is running."))?;
            
            let config = device.default_input_config()
                .map_err(|e| anyhow::anyhow!("Failed to get default config: {}", e))?;
            
            let sample_rate = config.sample_rate().0;
            info!("Monitor device config: {:?}", config);
            
            let (tx, rx) = futures_channel::mpsc::unbounded::<Vec<f32>>();
            let (drop_tx, drop_rx) = std::sync::mpsc::channel::<()>();
            
            // Build the input stream based on sample format
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => {
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if drop_rx.try_recv().is_ok() {
                                return;
                            }
                            let _ = tx.unbounded_send(data.to_vec());
                        },
                        |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                },
                cpal::SampleFormat::I16 => {
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if drop_rx.try_recv().is_ok() {
                                return;
                            }
                            let samples: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                            let _ = tx.unbounded_send(samples);
                        },
                        |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                },
                cpal::SampleFormat::U16 => {
                    device.build_input_stream(
                        &config.into(),
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            if drop_rx.try_recv().is_ok() {
                                return;
                            }
                            let samples: Vec<f32> = data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect();
                            let _ = tx.unbounded_send(samples);
                        },
                        |err| eprintln!("Stream error: {}", err),
                        None,
                    )
                },
                _ => {
                    return Err(anyhow::anyhow!("Unsupported sample format: {:?}", config.sample_format()));
                }
            }.map_err(|e| anyhow::anyhow!("Failed to build input stream: {}", e))?;
            
            // Start playing the stream
            use cpal::traits::StreamTrait;
            stream.play().map_err(|e| anyhow::anyhow!("Failed to start stream: {}", e))?;
            
            info!("PulseAudio/PipeWire system audio capture started successfully");
            
            let receiver = rx.map(futures_util::stream::iter).flatten();
            
            Ok(SystemAudioStream {
                drop_tx,
                sample_rate,
                receiver: Box::pin(receiver),
                _stream: Some(stream),
            })
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            // For Windows, we would implement WASAPI loopback here
            anyhow::bail!("System audio capture not yet implemented for this platform")
        }
    }

    pub fn check_system_audio_permissions() -> bool {
        // Check if we can enumerate audio devices
        match cpal::default_host().output_devices() {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

pub struct SystemAudioStream {
    drop_tx: std::sync::mpsc::Sender<()>,
    sample_rate: u32,
    receiver: Pin<Box<dyn Stream<Item = f32> + Send + Sync>>,
    #[cfg(target_os = "linux")]
    _stream: Option<cpal::Stream>, // Keep stream alive on Linux
}

impl Drop for SystemAudioStream {
    fn drop(&mut self) {
        let _ = self.drop_tx.send(());
    }
}

impl Stream for SystemAudioStream {
    type Item = f32;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.as_mut().poll_next_unpin(cx)
    }
}

impl SystemAudioStream {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Public interface for system audio capture
pub async fn start_system_audio_capture() -> Result<SystemAudioStream> {
    let capture = SystemAudioCapture::new()?;
    capture.start_system_audio_capture()
}

pub fn list_system_audio_devices() -> Result<Vec<String>> {
    SystemAudioCapture::list_system_devices()
}

pub fn check_system_audio_permissions() -> bool {
    SystemAudioCapture::check_system_audio_permissions()
}