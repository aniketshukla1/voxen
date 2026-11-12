//! Text to speech, written against raw HTTP APIs: ElevenLabs in the cloud, or a speech model
//! pulled into Ollama on this Mac (like Phonon). Both stream 16-bit, 24 kHz mono samples.

use crate::llm;
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;

/// Both stream raw 16-bit mono PCM at this rate: 24 kHz works on ElevenLabs' free tier, and
/// it's what Ollama's speech models make.
pub const RATE: u32 = 24_000;

/// A voice from a speech model in Ollama is called `ollama/<model>/<voice>`, like
/// `ollama/phonon/Zoey`. Any other voice is an ElevenLabs voice id.
const LOCAL: &str = "ollama/";

#[derive(Clone)]
pub struct Voices {
    http: reqwest::Client,
    /// ElevenLabs' key and model, when the key is set.
    eleven: Option<(String, String)>,
    /// Where Ollama listens.
    ollama: String,
}

impl Voices {
    /// ElevenLabs from `ELEVENLABS_API_KEY` (and `ELEVENLABS_MODEL`) when the key is set, and
    /// Ollama at its usual address (`OLLAMA_BASE_URL` changes it).
    pub fn from_env() -> Self {
        let key = std::env::var("ELEVENLABS_API_KEY").ok().filter(|k| !k.trim().is_empty());
        let model = std::env::var("ELEVENLABS_MODEL").unwrap_or_else(|_| "eleven_flash_v2_5".into());
        Self {
            http: reqwest::Client::new(),
            eleven: key.map(|key| (key, model)),
            ollama: llm::ollama_root(),
        }
    }

    /// ElevenLabs is set up.
    pub fn eleven(&self) -> bool {
        self.eleven.is_some()
    }

    /// The speech models pulled into Ollama, like "phonon"; none when Ollama isn't running.
    pub async fn local_models(&self) -> Vec<String> {
        let asked = self
            .http
            .get(format!("{}/api/tags", self.ollama))
            .timeout(Duration::from_secs(2))
            .send()
            .await;
        match asked {
            Ok(reply) => reply.json::<Value>().await.map(|tags| llm::ollama_models(&tags, "speech")).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Every voice a bot can have, as (id, name): ElevenLabs' when its key is set, then each
    /// speech model's voices in Ollama.
    pub async fn list(&self) -> Result<Vec<(String, String)>> {
        let mut all = Vec::new();
        let mut problem = None;
        if let Some((key, _)) = &self.eleven {
            match self.eleven_voices(key).await {
                Ok(voices) => all.extend(voices),
                Err(e) => problem = Some(e),
            }
        }
        for model in self.local_models().await {
            let show: Value = self
                .http
                .post(format!("{}/api/show", self.ollama))
                .json(&json!({"model": model}))
                .send()
                .await
                .context("asking Ollama for its voices")?
                .json()
                .await?;
            all.extend(voice_names(&show).into_iter().map(|name| (format!("{LOCAL}{model}/{name}"), name)));
        }
        match (all.is_empty(), problem) {
            (true, Some(e)) => Err(e),
            (true, None) => bail!("there are no voices yet: add your ElevenLabs key, or pull a voice with Ollama (ollama pull phonon)"),
            _ => Ok(all),
        }
    }

    async fn eleven_voices(&self, key: &str) -> Result<Vec<(String, String)>> {
        let resp = self
            .http
            .get("https://api.elevenlabs.io/v1/voices")
            .header("xi-api-key", key)
            .send()
            .await
            .context("calling ElevenLabs")?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!("ElevenLabs {status}: {}", resp.text().await.unwrap_or_default());
        }
        let body: Value = resp.json().await?;
        Ok(body["voices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["voice_id"].as_str()?.to_string(), v["name"].as_str()?.to_string())))
            .collect())
    }

    /// Speaks one sentence, handing each chunk of samples to `play` as it arrives.
    /// `previous` is the sentence before it, so an ElevenLabs voice keeps its flow; Ollama's
    /// voices don't take it, or a speed.
    pub async fn speak(
        &self,
        voice: &str,
        speed: f32,
        text: &str,
        previous: &str,
        play: impl FnMut(&[i16]),
    ) -> Result<()> {
        if let Some((model, name)) = local_voice(voice) {
            return self.speak_local(model, name, text, play).await;
        }
        let Some((key, model)) = &self.eleven else {
            // Without ElevenLabs, a bot whose voice came from there still speaks, on this Mac.
            return self.speak_local("phonon", "default", text, play).await;
        };
        let mut body = json!({
            "text": text,
            "model_id": model,
            "voice_settings": {"stability": 0.5, "similarity_boost": 0.75, "speed": speed},
        });
        if !previous.is_empty() {
            body["previous_text"] = previous.into();
        }
        let resp = self
            .http
            .post(format!("https://api.elevenlabs.io/v1/text-to-speech/{voice}/stream?output_format=pcm_{RATE}"))
            .header("xi-api-key", key)
            .json(&body)
            .send()
            .await
            .context("calling ElevenLabs")?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!("ElevenLabs {status}: {}", resp.text().await.unwrap_or_default());
        }
        stream(resp, play).await
    }

    async fn speak_local(&self, model: &str, voice: &str, text: &str, play: impl FnMut(&[i16])) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/v1/audio/speech", self.ollama))
            .json(&json!({"model": model, "voice": voice, "input": text, "response_format": "pcm"}))
            .send()
            .await
            .context("calling Ollama on this Mac")?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!("Ollama {status}: {}", resp.text().await.unwrap_or_default());
        }
        stream(resp, play).await
    }
}

/// Hands the samples of a streamed audio reply to `play` as they arrive.
async fn stream(resp: reqwest::Response, mut play: impl FnMut(&[i16])) -> Result<()> {
    let mut carry = None;
    let mut audio = resp.bytes_stream();
    while let Some(chunk) = audio.next().await {
        play(&pcm16(&mut carry, &chunk?));
    }
    Ok(())
}

/// The model and voice in a voice id from Ollama, like ("phonon", "Zoey").
pub fn local_voice(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(LOCAL)?.rsplit_once('/')
}

/// The voices in Ollama's details for a speech model: a list of names, or of objects with one.
fn voice_names(show: &Value) -> Vec<String> {
    show["voices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().or_else(|| v["name"].as_str()).map(String::from))
        .collect()
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

    #[test]
    fn voices_on_this_mac_name_their_model() {
        assert_eq!(local_voice("ollama/phonon/Zoey"), Some(("phonon", "Zoey")));
        assert_eq!(local_voice("ollama/phonon:40m-fr/Captain Silas Blackwater"), Some(("phonon:40m-fr", "Captain Silas Blackwater")));
        assert_eq!(local_voice("JBFqnCBsd6RMkjVDRZzb"), None);
        assert_eq!(voice_names(&json!({"voices": ["Zoey", {"name": "Hadley"}]})), ["Zoey", "Hadley"]);
        assert!(voice_names(&json!({})).is_empty());
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs ELEVENLABS_API_KEY"]
    async fn live_sentence_comes_back_as_audio() {
        dotenvy::dotenv().ok();
        let voices = Voices::from_env();
        assert!(voices.eleven(), "put ELEVENLABS_API_KEY in .env");
        let mut samples = 0;
        voices
            .speak("JBFqnCBsd6RMkjVDRZzb", 1.0, "Hello there.", "", |s| samples += s.len())
            .await
            .unwrap();
        assert!(samples > RATE as usize / 4, "only {samples} samples came back");
    }

    #[tokio::test]
    #[ignore = "needs Ollama running with a voice model (ollama pull phonon)"]
    async fn live_voice_on_this_mac_comes_back_as_audio() {
        let voices = Voices::from_env();
        assert!(voices.local_models().await.iter().any(|m| m.starts_with("phonon")), "pull Phonon: ollama pull phonon");
        let listed = voices.list().await.unwrap();
        assert!(listed.iter().any(|(id, name)| id == "ollama/phonon/Zoey" && name == "Zoey"), "{listed:?}");
        let mut samples = 0;
        voices
            .speak("ollama/phonon/Zoey", 1.0, "Hello from your Mac.", "", |s| samples += s.len())
            .await
            .unwrap();
        assert!(samples > RATE as usize / 2, "only {samples} samples came back");
    }
}
