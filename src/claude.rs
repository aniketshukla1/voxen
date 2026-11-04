//! Claude Messages API client, streaming, written against the raw HTTP API.

use anyhow::{Context, Result, bail};
use futures::{Stream, StreamExt, stream};
use serde_json::{Value, json};

const API_VERSION: &str = "2026-10-21";
/// Retries a request on another model if a safety check declines it.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// One thing Claude produced while streaming a reply.
#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    Tool { name: String, input: Value },
    Done { stop_reason: String },
}

pub struct Claude {
    http: reqwest::Client,
    key: String,
    url: String,
    model: String,
}

impl Claude {
    /// Reads `ANTHROPIC_API_KEY`, and optionally `ANTHROPIC_BASE_URL` and `CLAUDE_MODEL`.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set (copy .env.example to .env)")?;
        let base = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".into());
        let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-opus-5-5".into());
        Ok(Self::new(key, &base, model))
    }

    pub fn new(key: impl Into<String>, base_url: &str, model: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            key: key.into(),
            url: format!("{}/v1/messages", base_url.trim_end_matches('/')),
            model: model.into(),
        }
    }

    /// Streams the reply to one user message. Low effort keeps spoken replies quick.
    pub async fn stream(
        &self,
        system: &str,
        user: &str,
        tools: &[Value],
    ) -> Result<impl Stream<Item = Result<Event>> + use<>> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": 16000,
            "stream": true,
            "system": system,
            "messages": [{"role": "user", "content": user}],
            "output_config": {"effort": "low"},
            "fallbacks": "default",
        });
        if !tools.is_empty() {
            body["tools"] = Value::from(tools.to_vec());
        }
        let resp = self
            .http
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .json(&body)
            .send()
            .await
            .context("calling Claude")?;
        if !resp.status().is_success() {
            let status = resp.status();
            bail!(
                "Claude API {status}: {}",
                resp.text().await.unwrap_or_default()
            );
        }
        let mut sse = Sse::default();
        Ok(resp.bytes_stream().flat_map(move |chunk| {
            let events: Vec<Result<Event>> = match chunk {
                Ok(bytes) => match sse.feed(&bytes) {
                    Ok(events) => events.into_iter().map(Ok).collect(),
                    Err(e) => vec![Err(e)],
                },
                Err(e) => vec![Err(e.into())],
            };
            stream::iter(events)
        }))
    }
}

/// Incremental parser for the Messages API's server-sent events.
#[derive(Default)]
pub struct Sse {
    buf: Vec<u8>,
    tool: Option<(String, String)>,
    stop_reason: String,
}

impl Sse {
    /// Feeds raw bytes from the network and returns every event they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Event>> {
        self.buf.extend(bytes.iter().filter(|&&b| b != b'\r'));
        let mut out = Vec::new();
        while let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = self.buf.drain(..end + 2).collect();
            for line in String::from_utf8_lossy(&block).lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    let event: Value =
                        serde_json::from_str(data.trim()).context("bad event from Claude")?;
                    self.event(&event, &mut out)?;
                }
            }
        }
        Ok(out)
    }

    fn event(&mut self, v: &Value, out: &mut Vec<Event>) -> Result<()> {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match v["type"].as_str().unwrap_or_default() {
            "content_block_start" if v["content_block"]["type"] == "tool_use" => {
                self.tool = Some((text(&v["content_block"]["name"]), String::new()));
            }
            "content_block_delta" => match v["delta"]["type"].as_str() {
                Some("text_delta") => out.push(Event::Text(text(&v["delta"]["text"]))),
                Some("input_json_delta") => {
                    if let Some((_, input)) = &mut self.tool {
                        input.push_str(v["delta"]["partial_json"].as_str().unwrap_or_default());
                    }
                }
                _ => {} // thinking and other deltas aren't spoken
            },
            "content_block_stop" => {
                if let Some((name, input)) = self.tool.take() {
                    // Eager input streaming skips server-side validation: parse strictly, drop bad input.
                    let json = if input.is_empty() {
                        "{}"
                    } else {
                        input.as_str()
                    };
                    if let Ok(input) = serde_json::from_str(json) {
                        out.push(Event::Tool { name, input });
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = reason.to_string();
                }
            }
            "message_stop" => out.push(Event::Done {
                stop_reason: std::mem::take(&mut self.stop_reason),
            }),
            "error" => bail!("Claude stream error: {}", v["error"]["message"]),
            _ => {} // message_start, ping, and text, thinking or fallback block starts
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(chunks: &[&str]) -> Result<Vec<Event>> {
        let mut sse = Sse::default();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(sse.feed(chunk.as_bytes())?);
        }
        Ok(out)
    }

    #[test]
    fn text_split_across_chunks() {
        let events = feed_all(&[
            "event: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel",
            "lo\"}}\r\n\r\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ])
        .unwrap();
        assert_eq!(
            events,
            vec![
                Event::Text("Hello".into()),
                Event::Done {
                    stop_reason: "end_turn".into()
                }
            ]
        );
    }

    #[test]
    fn tool_input_is_collected_and_thinking_is_ignored() {
        let events = feed_all(&[
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}

data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"hand_off","input":{}}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"to\":"}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"Sam\"}"}}

data: {"type":"content_block_stop","index":1}

"#,
        ])
        .unwrap();
        assert_eq!(
            events,
            vec![Event::Tool {
                name: "hand_off".into(),
                input: json!({"to": "Sam"})
            }]
        );
    }

    #[test]
    fn broken_tool_input_is_dropped() {
        let events = feed_all(&[r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"hand_off","input":{}}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"to\": \"Sa"}}

data: {"type":"content_block_stop","index":0}

"#])
        .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs ANTHROPIC_API_KEY"]
    async fn live_reply_streams_text() {
        dotenvy::dotenv().ok();
        let claude = Claude::from_env().unwrap();
        let events: Vec<Event> = claude
            .stream("Reply in at most five words.", "Say hello.", &[])
            .await
            .unwrap()
            .map(|e| e.unwrap())
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Text(t) if !t.is_empty()))
        );
        assert!(matches!(events.last(), Some(Event::Done { .. })));
    }

    #[test]
    fn stream_error_is_an_error() {
        let result = feed_all(&[
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        ]);
        assert!(result.unwrap_err().to_string().contains("Overloaded"));
    }
}
