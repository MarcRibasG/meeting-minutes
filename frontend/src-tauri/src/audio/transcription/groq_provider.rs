// audio/transcription/groq_provider.rs
//
// Groq cloud transcription provider using Whisper API

use async_trait::async_trait;
use log::{info, warn};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::io::Cursor;

use super::provider::{TranscriptionError, TranscriptionProvider, TranscriptResult};

#[derive(Deserialize)]
struct GroqResponse {
    text: String,
}

pub struct GroqProvider {
    api_key: String,
    model: String,
}

impl GroqProvider {
    pub fn new(api_key: String, model: String) -> Self {
        info!("🌐 Groq provider initialized with model: {}", model);
        Self { api_key, model }
    }
}

#[async_trait]
impl TranscriptionProvider for GroqProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> Result<TranscriptResult, TranscriptionError> {
        // Convert f32 samples to WAV bytes
        let wav_bytes = samples_to_wav(&audio, 16000)
            .map_err(|e| TranscriptionError::EngineFailed(format!("WAV conversion failed: {}", e)))?;

        // Create multipart form
        let audio_part = Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| TranscriptionError::EngineFailed(format!("Failed to create audio part: {}", e)))?;

        let mut form = Form::new()
            .part("file", audio_part)
            .text("model", self.model.clone());

        if let Some(lang) = language {
            form = form.text("language", lang);
        }

        // Send request to Groq API
        let client = reqwest::Client::new();
        let response = client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(format!("Groq API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(TranscriptionError::EngineFailed(format!(
                "Groq API error {}: {}",
                status, error_text
            )));
        }

        let groq_response: GroqResponse = response
            .json()
            .await
            .map_err(|e| TranscriptionError::EngineFailed(format!("Failed to parse Groq response: {}", e)))?;

        Ok(TranscriptResult {
            text: groq_response.text,
            confidence: None, // Groq doesn't provide confidence scores
            is_partial: false,
        })
    }

    async fn is_model_loaded(&self) -> bool {
        true // Cloud service, always "loaded"
    }

    async fn get_current_model(&self) -> Option<String> {
        Some(self.model.clone())
    }

    fn provider_name(&self) -> &'static str {
        "groq"
    }
}

/// Convert f32 audio samples to WAV format bytes
fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec)
        .map_err(|e| format!("Failed to create WAV writer: {}", e))?;

    for &sample in samples {
        let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer
            .write_sample(sample_i16)
            .map_err(|e| format!("Failed to write sample: {}", e))?;
    }

    writer
        .finalize()
        .map_err(|e| format!("Failed to finalize WAV: {}", e))?;

    Ok(cursor.into_inner())
}
