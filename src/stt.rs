//! Deepgram live speech-to-text client, written against the raw WebSocket API.

use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

/// How long a pause ends your turn. Too short cuts you off mid-thought; too long feels slow.
pub const ENDPOINTING_MS: u32 = 400;

/// What the listener heard. Times are seconds into the audio stream.
#[derive(Debug, PartialEq)]
pub enum Heard {
    SpeechStarted {
        at: f64,
    },
    /// Words so far in the current phrase; they may still change.
    Partial(String),
    /// Settled words. `end_of_turn` means you paused long enough to be done.
    Final {
        text: String,
        end_of_turn: bool,
        end: f64,
    },
    /// Backup end-of-turn signal after a longer gap between words.
    UtteranceEnd,
}

/// The listen URL. Bot names go in as key terms so they're recognized reliably.
pub fn url(rate: u32, names: &[String]) -> String {
    let mut url = format!(
        "wss://api.deepgram.com/v1/listen?model=nova-3&encoding=linear16&sample_rate={rate}\
         &channels=1&interim_results=true&smart_format=true&vad_events=true\
         &endpointing={ENDPOINTING_MS}&utterance_end_ms=1000"
    );
    for name in names {
        url.push_str("&keyterm=");
        url.push_str(&name.replace(' ', "%20"));
    }
    url
}

/// Reads one message from Deepgram; `None` for messages that don't matter here.
pub fn parse(msg: &str) -> Option<Heard> {
    let v: Value = serde_json::from_str(msg).ok()?;
    match v["type"].as_str()? {
        "SpeechStarted" => Some(Heard::SpeechStarted {
            at: v["timestamp"].as_f64()?,
        }),
        "UtteranceEnd" => Some(Heard::UtteranceEnd),
        "Results" => {
            let best = &v["channel"]["alternatives"][0];
            let text = best["transcript"].as_str()?.trim().to_string();
            if v["is_final"] == true {
                let segment_end =
                    v["start"].as_f64().unwrap_or(0.0) + v["duration"].as_f64().unwrap_or(0.0);
                let end = best["words"]
                    .as_array()
                    .and_then(|words| words.last())
                    .and_then(|word| word["end"].as_f64())
                    .unwrap_or(segment_end);
                Some(Heard::Final {
                    text,
                    end_of_turn: v["speech_final"] == true,
                    end,
                })
            } else if text.is_empty() {
                None
            } else {
                Some(Heard::Partial(text))
            }
        }
        _ => None,
    }
}

/// Streams mic audio to Deepgram and forwards what it hears. Returns `Ok` when the
/// audio channel closes and an error if the connection drops. `sent` counts samples
/// sent on this connection, so the caller knows where "now" is in stream time.
pub async fn listen(
    key: &str,
    rate: u32,
    names: &[String],
    audio: &mut mpsc::UnboundedReceiver<Vec<i16>>,
    heard: &mpsc::UnboundedSender<Heard>,
    sent: &AtomicU64,
) -> Result<()> {
    let mut request = url(rate, names).into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", format!("Token {key}").parse()?);
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("connecting to Deepgram")?;
    sent.store(0, Ordering::Relaxed);
    let (mut tx, mut rx) = ws.split();
    let send = async {
        while let Some(frame) = audio.recv().await {
            let bytes: Vec<u8> = frame.iter().flat_map(|s| s.to_le_bytes()).collect();
            sent.fetch_add(frame.len() as u64, Ordering::Relaxed);
            tx.send(Message::Binary(bytes.into())).await?;
        }
        tx.send(Message::Text(r#"{"type":"CloseStream"}"#.into()))
            .await?;
        anyhow::Ok(())
    };
    let receive = async {
        while let Some(message) = rx.next().await {
            if let Message::Text(text) = message?
                && let Some(h) = parse(text.as_str())
            {
                heard.send(h).ok();
            }
        }
        anyhow::Ok(())
    };
    tokio::select! {
        done = send => done,
        done = receive => {
            done?;
            bail!("Deepgram closed the connection")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_result_with_pause_ends_the_turn() {
        let msg = r#"{"type":"Results","start":1.0,"duration":2.0,"is_final":true,"speech_final":true,
            "channel":{"alternatives":[{"transcript":"Sam, what's 15% of 80?","confidence":0.99,
            "words":[{"word":"sam","start":1.1,"end":1.4},{"word":"80","start":2.4,"end":2.7}]}]}}"#;
        assert_eq!(
            parse(msg),
            Some(Heard::Final {
                text: "Sam, what's 15% of 80?".into(),
                end_of_turn: true,
                end: 2.7
            })
        );
    }

    #[test]
    fn interim_results_are_partial_and_empty_ones_are_skipped() {
        let partial = r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":"what's"}]}}"#;
        let empty =
            r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":""}]}}"#;
        assert_eq!(parse(partial), Some(Heard::Partial("what's".into())));
        assert_eq!(parse(empty), None);
    }

    #[test]
    fn speech_events_and_noise() {
        assert_eq!(
            parse(r#"{"type":"SpeechStarted","channel":[0],"timestamp":3.25}"#),
            Some(Heard::SpeechStarted { at: 3.25 })
        );
        assert_eq!(
            parse(r#"{"type":"UtteranceEnd","channel":[0],"last_word_end":2.7}"#),
            Some(Heard::UtteranceEnd)
        );
        assert_eq!(parse(r#"{"type":"Metadata","request_id":"x"}"#), None);
        assert_eq!(parse("not json"), None);
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs DEEPGRAM_API_KEY"]
    async fn live_connection_accepts_audio() {
        dotenvy::dotenv().ok();
        let key = std::env::var("DEEPGRAM_API_KEY").unwrap();
        let (tx, mut audio) = mpsc::unbounded_channel();
        let (heard, _) = mpsc::unbounded_channel();
        tx.send(vec![0i16; 16_000]).unwrap(); // one second of silence
        drop(tx);
        let sent = AtomicU64::new(0);
        listen(&key, 16_000, &[], &mut audio, &heard, &sent)
            .await
            .unwrap();
        assert_eq!(sent.load(Ordering::Relaxed), 16_000);
    }

    #[test]
    fn bot_names_become_key_terms() {
        let url = url(48_000, &["Maya".into(), "Mary Ann".into()]);
        assert!(url.contains("sample_rate=48000&channels=1"));
        assert!(url.ends_with("&keyterm=Maya&keyterm=Mary%20Ann"));
    }
}
