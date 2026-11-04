//! ElevenLabs streaming text-to-speech client, written against the raw HTTP API.

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use serde_json::json;

/// ElevenLabs streams raw 16-bit mono PCM at this rate (24 kHz works on the free tier).
pub const RATE: u32 = 24_000;

pub struct Voices {
    http: reqwest::Client,
    key: String,
    model: String,
}

impl Voices {
    /// Reads `ELEVENLABS_API_KEY`, and optionally `ELEVENLABS_MODEL`.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::new(),
            key: std::env::var("ELEVENLABS_API_KEY")
                .context("ELEVENLABS_API_KEY is not set (copy .env.example to .env)")?,
            model: std::env::var("ELEVENLABS_MODEL").unwrap_or_else(|_| "eleven_flash_v2_5".into()),
        })
    }

    /// The voices this account can use, as (id, name).
    pub async fn list(&self) -> Result<Vec<(String, String)>> {
        let resp = self
            .http
            .get("https://api.elevenlabs.io/v1/voices")
            .header("xi-api-key", &self.key)
            .send()
            .await
            .context("calling ElevenLabs")?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!(
                "ElevenLabs {status}: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(body["voices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some((
                    v["voice_id"].as_str()?.to_string(),
                    v["name"].as_str()?.to_string(),
                ))
            })
            .collect())
    }

    /// Speaks one sentence, handing each chunk of samples to `play` as it arrives.
    /// `previous` is the sentence before it, so the voice keeps its flow.
    pub async fn speak(
        &self,
        voice: &str,
        speed: f32,
        text: &str,
        previous: &str,
        mut play: impl FnMut(&[i16]),
    ) -> Result<()> {
        let mut body = json!({
            "text": text,
            "model_id": self.model,
            "voice_settings": {"stability": 0.5, "similarity_boost": 0.75, "speed": speed},
        });
        if !previous.is_empty() {
            body["previous_text"] = previous.into();
        }
        let resp = self
            .http
            .post(format!(
                "https://api.elevenlabs.io/v1/text-to-speech/{voice}/stream?output_format=pcm_{RATE}"
            ))
            .header("xi-api-key", &self.key)
            .json(&body)
            .send()
            .await
            .context("calling ElevenLabs")?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!(
                "ElevenLabs {status}: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        let mut carry = None;
        let mut audio = resp.bytes_stream();
        while let Some(chunk) = audio.next().await {
            play(&pcm16(&mut carry, &chunk?));
        }
        Ok(())
    }
}

/// Turns little-endian bytes into samples. A chunk can end halfway through a sample,
/// so the odd byte is carried into the next call.
pub fn pcm16(carry: &mut Option<u8>, bytes: &[u8]) -> Vec<i16> {
    let mut joined: Vec<u8> = carry.take().into_iter().collect();
    joined.extend_from_slice(bytes);
    if joined.len() % 2 == 1 {
        *carry = joined.pop();
    }
    joined
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| i16::from_le_bytes(pair))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_split_across_chunks_survive() {
        let mut carry = None;
        // 1000 = 0x03E8 and -2 = 0xFFFE, little-endian, split mid-sample.
        assert_eq!(pcm16(&mut carry, &[0xE8, 0x03, 0xFE]), vec![1000]);
        assert_eq!(pcm16(&mut carry, &[0xFF]), vec![-2]);
        assert_eq!(carry, None);
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs ELEVENLABS_API_KEY"]
    async fn live_sentence_comes_back_as_audio() {
        dotenvy::dotenv().ok();
        let voices = Voices::from_env().unwrap();
        let mut samples = 0;
        voices
            .speak("JBFqnCBsd6RMkjVDRZzb", 1.0, "Hello there.", "", |s| {
                samples += s.len()
            })
            .await
            .unwrap();
        assert!(
            samples > RATE as usize / 4,
            "only {samples} samples came back"
        );
    }
}
