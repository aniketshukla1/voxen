//! Claude Messages API client, streaming, written against the raw HTTP API.

use anyhow::{Context, Result, bail};
use futures::{Stream, StreamExt, stream};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;

const API_VERSION: &str = "2026-10-21";
/// Retries a request on another model if a safety check declines it.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// A web page Claude found or quoted.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub url: String,
    pub title: String,
}

/// One thing Claude produced while streaming a reply.
#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    /// Claude started one of Anthropic's own tools, like web search; `input` holds its query.
    Using { tool: String, input: Value },
    /// The pages a web search found.
    Found(Vec<Source>),
    /// A page Claude quoted in its answer.
    Cited(Source),
    /// Claude asks the app to run one of the app's tools. `input` is null when it came
    /// through broken.
    Tool {
        id: String,
        name: String,
        input: Value,
    },
    /// The reply ended. `content` is all of it, ready to send back to continue the turn.
    Done {
        stop_reason: String,
        content: Vec<Value>,
    },
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

    /// Streams Claude's next reply to `messages`. `effort` trades speed for depth: "low"
    /// keeps spoken replies quick.
    pub async fn stream(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[Value],
        effort: &str,
    ) -> Result<impl Stream<Item = Result<Event>> + use<>> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": 16000,
            "stream": true,
            "system": system,
            "messages": messages,
            "output_config": {"effort": effort},
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
    /// The reply's content blocks so far, by index.
    blocks: Vec<Value>,
    /// Tool input JSON still arriving, by block index.
    inputs: HashMap<usize, String>,
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
        let at = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str().unwrap_or_default() {
            "content_block_start" => {
                let block = v["content_block"].clone();
                match block["type"].as_str() {
                    Some("tool_use" | "server_tool_use") => {
                        self.inputs.insert(at, String::new());
                    }
                    // A failed search's content is an error object, not a list.
                    Some("web_search_tool_result") => {
                        if let Some(results) = block["content"].as_array() {
                            out.push(Event::Found(results.iter().filter_map(source).collect()));
                        }
                    }
                    _ => {}
                }
                if self.blocks.len() <= at {
                    self.blocks.resize(at + 1, Value::Null);
                }
                self.blocks[at] = block;
            }
            "content_block_delta" => {
                let delta = &v["delta"];
                let block = self.blocks.get_mut(at);
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        if let Some(block) = block {
                            append(block, "text", text);
                        }
                        out.push(Event::Text(text.to_string()));
                    }
                    "thinking_delta" => {
                        if let Some(block) = block {
                            append(block, "thinking", delta["thinking"].as_str().unwrap_or_default());
                        }
                    }
                    "signature_delta" => {
                        if let Some(block) = block {
                            block["signature"] = delta["signature"].clone();
                        }
                    }
                    "citations_delta" => {
                        let citation = &delta["citation"];
                        if let Some(found) = source(citation) {
                            out.push(Event::Cited(found));
                        }
                        if let Some(block) = block {
                            match block["citations"].as_array_mut() {
                                Some(list) => list.push(citation.clone()),
                                None => block["citations"] = json!([citation]),
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(input) = self.inputs.get_mut(&at) {
                            input.push_str(delta["partial_json"].as_str().unwrap_or_default());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(json) = self.inputs.remove(&at) {
                    self.tool_ready(at, &json, out);
                }
            }
            "message_delta" => {
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = reason.to_string();
                }
            }
            "message_stop" => {
                let blocks = std::mem::take(&mut self.blocks);
                out.push(Event::Done {
                    stop_reason: std::mem::take(&mut self.stop_reason),
                    content: echo(blocks.into_iter().filter(|b| !b.is_null()).collect()),
                });
            }
            "error" => bail!("Claude stream error: {}", v["error"]["message"]),
            _ => {} // message_start, ping
        }
        Ok(())
    }

    /// A tool call's input is complete: keep it in its block and say which tool is used.
    fn tool_ready(&mut self, at: usize, json: &str, out: &mut Vec<Event>) {
        let block = &mut self.blocks[at];
        // Eager input streaming skips the API's validation, so parse strictly.
        let input: Option<Value> =
            serde_json::from_str(if json.is_empty() { "{}" } else { json }).ok();
        if let Some(input) = &input {
            block["input"] = input.clone();
        }
        let name = block["name"].as_str().unwrap_or_default().to_string();
        if block["type"] == "tool_use" {
            let id = block["id"].as_str().unwrap_or_default().to_string();
            let input = input.unwrap_or(Value::Null);
            out.push(Event::Tool { id, name, input });
        } else if let Some(input) = input {
            out.push(Event::Using { tool: name, input });
        }
    }
}

fn append(block: &mut Value, field: &str, text: &str) {
    match block.get_mut(field) {
        Some(Value::String(s)) => s.push_str(text),
        _ => block[field] = Value::from(text),
    }
}

/// A search result or citation as a source, if it has a link.
fn source(v: &Value) -> Option<Source> {
    let url = v["url"].as_str()?.to_string();
    let title = v["title"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&url)
        .to_string();
    Some(Source { url, title })
}

/// The reply's blocks as they can be sent back. After a mid-reply switch to a fallback
/// model, only text and finished server-tool pairs from before the switch go back.
fn echo(content: Vec<Value>) -> Vec<Value> {
    let Some(switch) = content.iter().rposition(|b| b["type"] == "fallback") else {
        return content;
    };
    let finished: Vec<String> = content[..switch]
        .iter()
        .filter_map(|b| b["tool_use_id"].as_str().map(String::from))
        .collect();
    content
        .into_iter()
        .enumerate()
        .filter(|(i, b)| {
            let kind = b["type"].as_str().unwrap_or_default();
            *i > switch
                || (*i < switch
                    && (kind == "text"
                        || kind.ends_with("_tool_result")
                        || (kind == "server_tool_use"
                            && finished.iter().any(|id| b["id"] == id.as_str()))))
        })
        .map(|(_, b)| b)
        .collect()
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
                    stop_reason: "end_turn".into(),
                    content: vec![]
                }
            ]
        );
    }

    #[test]
    fn tool_input_is_collected_and_thinking_is_kept_quietly() {
        let events = feed_all(&[
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}

data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"remember","input":{}}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"fact\":"}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"Likes tea\"}"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}

data: {"type":"message_stop"}

"#,
        ])
        .unwrap();
        assert_eq!(
            events,
            vec![
                Event::Tool {
                    id: "t1".into(),
                    name: "remember".into(),
                    input: json!({"fact": "Likes tea"})
                },
                Event::Done {
                    stop_reason: "tool_use".into(),
                    content: vec![
                        json!({"type": "thinking", "thinking": "", "signature": "sig"}),
                        json!({"type": "tool_use", "id": "t1", "name": "remember", "input": {"fact": "Likes tea"}}),
                    ]
                }
            ]
        );
    }

    #[test]
    fn broken_tool_input_comes_through_as_null() {
        let events = feed_all(&[r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"remember","input":{}}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"fact\": \"Li"}}

data: {"type":"content_block_stop","index":0}

"#])
        .unwrap();
        assert_eq!(
            events,
            vec![Event::Tool {
                id: "t1".into(),
                name: "remember".into(),
                input: Value::Null
            }]
        );
    }

    #[test]
    fn web_search_shows_query_results_and_quotes_and_keeps_the_reply() {
        let events = feed_all(&[r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"s1","name":"web_search","input":{}}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\": \"lisbon weather\"}"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"s1","content":[{"type":"web_search_result","url":"https://w.example/lisbon","title":"Lisbon forecast","encrypted_content":"x"}]}}

data: {"type":"content_block_stop","index":1}

data: {"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}

data: {"type":"content_block_delta","index":2,"delta":{"type":"citations_delta","citation":{"type":"web_search_result_location","url":"https://w.example/lisbon","title":"Lisbon forecast","cited_text":"Sunny, 21","encrypted_index":"e"}}}

data: {"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Sunny."}}

data: {"type":"content_block_stop","index":2}

data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

data: {"type":"message_stop"}

"#])
        .unwrap();
        let lisbon = Source {
            url: "https://w.example/lisbon".into(),
            title: "Lisbon forecast".into(),
        };
        assert_eq!(
            events[..4],
            [
                Event::Using {
                    tool: "web_search".into(),
                    input: json!({"query": "lisbon weather"})
                },
                Event::Found(vec![lisbon.clone()]),
                Event::Cited(lisbon),
                Event::Text("Sunny.".into()),
            ]
        );
        let Event::Done { content, .. } = &events[4] else {
            panic!("no Done: {events:?}");
        };
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["input"], json!({"query": "lisbon weather"}));
        assert_eq!(content[2]["text"], "Sunny.");
        assert_eq!(content[2]["citations"][0]["encrypted_index"], "e");
    }

    #[test]
    fn after_a_fallback_only_text_and_finished_searches_go_back() {
        let content = vec![
            json!({"type": "thinking", "thinking": "", "signature": "s"}),
            json!({"type": "server_tool_use", "id": "a", "name": "web_search"}),
            json!({"type": "web_search_tool_result", "tool_use_id": "a", "content": []}),
            json!({"type": "server_tool_use", "id": "b", "name": "web_search"}),
            json!({"type": "text", "text": "Hi"}),
            json!({"type": "fallback", "from": {}, "to": {}}),
            json!({"type": "thinking", "thinking": "", "signature": "t"}),
        ];
        let kept: Vec<String> = echo(content)
            .iter()
            .map(|b| b["id"].as_str().or(b["type"].as_str()).unwrap_or_default().to_string())
            .collect();
        assert_eq!(kept, ["a", "web_search_tool_result", "text", "thinking"]);
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs ANTHROPIC_API_KEY"]
    async fn live_reply_streams_text() {
        dotenvy::dotenv().ok();
        let claude = Claude::from_env().unwrap();
        let messages = [json!({"role": "user", "content": "Say hello."})];
        let events: Vec<Event> = claude
            .stream("Reply in at most five words.", &messages, &[], "low")
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
