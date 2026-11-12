//! Every LLM provider behind one interface, so each bot can think with a different one.
//! Four wire formats cover them all: Anthropic's Messages API, OpenAI's Responses API
//! (OpenAI, xAI), Google's Gemini API, and OpenAI-style Chat Completions (everyone else,
//! models running on this Mac included).

use crate::claude::{Claude, Event, Source};
use anyhow::{Context, Result, bail};
use futures::{Stream, StreamExt, stream};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wire {
    Anthropic,
    Responses,
    Gemini,
    Chat,
}

/// A company or app that runs models.
#[derive(Debug, Serialize)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    #[serde(skip)]
    pub wire: Wire,
    #[serde(skip)]
    pub base: &'static str,
    /// The setting that holds its key; none for models on this Mac.
    pub key: Option<&'static str>,
    /// The model a new bot gets.
    pub model: &'static str,
    /// It can search the web with its own built-in search.
    pub web: bool,
    /// It can use Voxen's tools: memory, reminders and the rest.
    pub tools: bool,
    /// Where to get a key, or the app to install.
    pub site: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn provider(
    id: &'static str,
    name: &'static str,
    wire: Wire,
    base: &'static str,
    key: Option<&'static str>,
    model: &'static str,
    web: bool,
    tools: bool,
    site: &'static str,
) -> Provider {
    Provider { id, name, wire, base, key, model, web, tools, site }
}

use Wire::{Anthropic, Chat, Gemini, Responses};

// ponytail: default models are a starting point; the app lists each provider's current models live.
pub const PROVIDERS: &[Provider] = &[
    provider("anthropic", "Anthropic Claude", Anthropic, "https://api.anthropic.com", Some("ANTHROPIC_API_KEY"), "claude-opus-5-5", true, true, "console.anthropic.com"),
    provider("openai", "OpenAI", Responses, "https://api.openai.com/v1", Some("OPENAI_API_KEY"), "gpt-5.6", true, true, "platform.openai.com"),
    provider("google", "Google Gemini", Gemini, "https://generativelanguage.googleapis.com", Some("GEMINI_API_KEY"), "gemini-3.5-flash", true, true, "aistudio.google.com"),
    provider("xai", "xAI Grok", Responses, "https://api.x.ai/v1", Some("XAI_API_KEY"), "grok-4.7", true, true, "console.x.ai"),
    provider("mistral", "Mistral", Chat, "https://api.mistral.ai/v1", Some("MISTRAL_API_KEY"), "mistral-large-latest", false, true, "console.mistral.ai"),
    provider("deepseek", "DeepSeek", Chat, "https://api.deepseek.com", Some("DEEPSEEK_API_KEY"), "deepseek-v4-flash", false, true, "platform.deepseek.com"),
    provider("perplexity", "Perplexity", Chat, "https://api.perplexity.ai", Some("PERPLEXITY_API_KEY"), "sonar-pro", true, false, "perplexity.ai/settings/api"),
    provider("openrouter", "OpenRouter (hundreds of models)", Chat, "https://openrouter.ai/api/v1", Some("OPENROUTER_API_KEY"), "openrouter/auto", false, true, "openrouter.ai/keys"),
    provider("groq", "Groq", Chat, "https://api.groq.com/openai/v1", Some("GROQ_API_KEY"), "openai/gpt-oss-120b", false, true, "console.groq.com"),
    provider("together", "Together AI", Chat, "https://api.together.xyz/v1", Some("TOGETHER_API_KEY"), "meta-llama/Llama-3.3-70B-Instruct-Turbo", false, true, "api.together.ai"),
    provider("fireworks", "Fireworks", Chat, "https://api.fireworks.ai/inference/v1", Some("FIREWORKS_API_KEY"), "accounts/fireworks/models/llama-v3p3-70b-instruct", false, true, "fireworks.ai"),
    provider("cerebras", "Cerebras", Chat, "https://api.cerebras.ai/v1", Some("CEREBRAS_API_KEY"), "gpt-oss-120b", false, true, "cloud.cerebras.ai"),
    provider("moonshot", "Moonshot Kimi", Chat, "https://api.moonshot.ai/v1", Some("MOONSHOT_API_KEY"), "kimi-k3", false, true, "platform.moonshot.ai"),
    provider("qwen", "Alibaba Qwen", Chat, "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", Some("DASHSCOPE_API_KEY"), "qwen-max-latest", false, true, "dashscope.console.aliyun.com"),
    provider("ollama", "Ollama (on this Mac)", Chat, "http://localhost:11434/v1", None, "llama3.2", false, true, "ollama.com"),
    provider("lmstudio", "LM Studio (on this Mac)", Chat, "http://localhost:1234/v1", None, "", false, true, "lmstudio.ai"),
    provider("custom", "Other (OpenAI-compatible)", Chat, "", Some("CUSTOM_API_KEY"), "", false, true, "your provider's docs"),
];

/// The provider with this id.
pub fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id.eq_ignore_ascii_case(id.trim()))
}

fn setting(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Where a provider is reached: `<ID>_BASE_URL` overrides the usual address.
fn base_of(provider: &Provider) -> String {
    setting(&format!("{}_BASE_URL", provider.id.to_uppercase()))
        .unwrap_or_else(|| provider.base.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Makes the clients replies think with. Keys are read when a reply starts, so new ones
/// count at once.
pub struct Brains {
    http: reqwest::Client,
    /// Tests send every provider here.
    offline: Option<String>,
}

impl Default for Brains {
    fn default() -> Self {
        Self::new()
    }
}

impl Brains {
    pub fn new() -> Self {
        Self { http: reqwest::Client::new(), offline: None }
    }

    /// Every request fails at once, for tests.
    pub fn offline() -> Self {
        Self { http: reqwest::Client::new(), offline: Some("http://127.0.0.1:9".into()) }
    }

    /// Whether a provider can be used: its key is set (models on this Mac need none).
    pub fn ready(&self, id: &str) -> bool {
        let Some(provider) = find(id) else {
            return false;
        };
        if provider.id == "custom" {
            return setting("CUSTOM_BASE_URL").is_some();
        }
        self.offline.is_some() || provider.key.is_none_or(|key| setting(key).is_some())
    }

    /// A client for `model` on `provider`; an empty model means the provider's default.
    pub fn llm(&self, provider: &str, model: &str) -> Result<Llm> {
        let provider = find(provider).with_context(|| format!("there's no provider called {provider}"))?;
        let model = if model.trim().is_empty() { provider.model } else { model.trim() };
        if model.is_empty() {
            bail!("choose a {} model for this bot in Edit voice", provider.name);
        }
        let (base, key) = match &self.offline {
            Some(dead) => (dead.clone(), "test".to_string()),
            None => {
                let base = base_of(provider);
                if base.is_empty() {
                    bail!("add the address of your {} in Settings", provider.name);
                }
                let key = match provider.key {
                    Some(name) => match setting(name) {
                        Some(key) => key,
                        None if provider.id == "custom" => String::new(),
                        None => bail!("add your {} key in Settings", provider.name),
                    },
                    None => String::new(),
                };
                (base, key)
            }
        };
        Ok(Llm { provider, http: self.http.clone(), key, base, model: model.to_string() })
    }
}

/// One model on one provider.
pub struct Llm {
    pub provider: &'static Provider,
    http: reqwest::Client,
    key: String,
    base: String,
    pub model: String,
}

/// A reply in progress, in the provider's own message format.
#[derive(Debug, Default, PartialEq)]
pub struct Convo {
    pub items: Vec<Value>,
    /// Responses API: the response this one continues.
    pub previous: Option<String>,
}

/// What one of Voxen's tools gave back.
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub output: String,
    pub failed: bool,
}

pub type Events = Pin<Box<dyn Stream<Item = Result<Event>> + Send>>;

/// The part of a user turn the bot reads: words, pictures, PDFs or text files.
enum Part<'a> {
    Text(&'a str),
    Image { media_type: &'a str, data: &'a str },
    Pdf { name: &'a str, data: &'a str },
    Doc { name: &'a str, text: &'a str },
}

/// Reads a user turn written as Anthropic content blocks (the app's own format).
fn parts(content: &[Value]) -> Vec<Part<'_>> {
    content
        .iter()
        .filter_map(|b| {
            let source = &b["source"];
            let name = b["title"].as_str().unwrap_or("file");
            match b["type"].as_str()? {
                "text" => Some(Part::Text(b["text"].as_str()?)),
                "image" => Some(Part::Image {
                    media_type: source["media_type"].as_str()?,
                    data: source["data"].as_str()?,
                }),
                "document" if source["type"] == "base64" => Some(Part::Pdf { name, data: source["data"].as_str()? }),
                "document" => Some(Part::Doc { name, text: source["data"].as_str()? }),
                _ => None,
            }
        })
        .collect()
}

impl Llm {
    /// Starts a reply to a user turn (Anthropic content blocks: files, then the conversation).
    pub fn start(&self, content: &[Value]) -> Convo {
        let parts = parts(content);
        let message = match self.provider.wire {
            Anthropic => json!({"role": "user", "content": content}),
            Responses => {
                let content: Vec<Value> = parts
                    .iter()
                    .map(|p| match p {
                        Part::Text(text) => json!({"type": "input_text", "text": text}),
                        Part::Image { media_type, data } => {
                            json!({"type": "input_image", "image_url": format!("data:{media_type};base64,{data}")})
                        }
                        Part::Pdf { name, data } => json!({
                            "type": "input_file",
                            "filename": name,
                            "file_data": format!("data:application/pdf;base64,{data}"),
                        }),
                        Part::Doc { name, text } => json!({"type": "input_text", "text": format!("File {name}:\n{text}")}),
                    })
                    .collect();
                json!({"role": "user", "content": content})
            }
            Gemini => {
                let parts: Vec<Value> = parts
                    .iter()
                    .map(|p| match p {
                        Part::Text(text) => json!({"text": text}),
                        Part::Image { media_type, data } => json!({"inlineData": {"mimeType": media_type, "data": data}}),
                        Part::Pdf { data, .. } => json!({"inlineData": {"mimeType": "application/pdf", "data": data}}),
                        Part::Doc { name, text } => json!({"text": format!("File {name}:\n{text}")}),
                    })
                    .collect();
                json!({"role": "user", "parts": parts})
            }
            Chat => {
                let text_of = |p: &Part| match p {
                    Part::Text(text) => text.to_string(),
                    Part::Doc { name, text } => format!("File {name}:\n{text}"),
                    Part::Pdf { name, .. } => format!("[{name} is a PDF, which this model can't read]"),
                    Part::Image { .. } => String::new(),
                };
                // Many of these APIs only take plain text, so a list is sent only with pictures.
                if parts.iter().any(|p| matches!(p, Part::Image { .. })) {
                    let content: Vec<Value> = parts
                        .iter()
                        .map(|p| match p {
                            Part::Image { media_type, data } => {
                                json!({"type": "image_url", "image_url": {"url": format!("data:{media_type};base64,{data}")}})
                            }
                            other => json!({"type": "text", "text": text_of(other)}),
                        })
                        .collect();
                    json!({"role": "user", "content": content})
                } else {
                    let text: Vec<String> = parts.iter().map(text_of).collect();
                    json!({"role": "user", "content": text.join("\n\n")})
                }
            }
        };
        Convo { items: vec![message], previous: None }
    }

    /// Adds the bot's reply so far and what its tools gave back, so the reply can go on.
    pub fn carry_on(&self, convo: &mut Convo, reply: Vec<Value>, results: Vec<ToolResult>) {
        let said = |r: &ToolResult| if r.failed { format!("Error: {}", r.output) } else { r.output.clone() };
        match self.provider.wire {
            Anthropic => {
                convo.items.push(json!({"role": "assistant", "content": reply}));
                if !results.is_empty() {
                    let results: Vec<Value> = results
                        .iter()
                        .map(|r| json!({"type": "tool_result", "tool_use_id": r.id, "content": r.output, "is_error": r.failed}))
                        .collect();
                    convo.items.push(json!({"role": "user", "content": results}));
                }
            }
            // The provider keeps the reply; the next request names it and adds the results.
            Responses => {
                convo.previous = reply.first().and_then(|r| r["id"].as_str()).map(String::from);
                convo.items = results
                    .iter()
                    .map(|r| json!({"type": "function_call_output", "call_id": r.id, "output": said(r)}))
                    .collect();
            }
            Gemini => {
                convo.items.push(json!({"role": "model", "parts": reply}));
                let parts: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        let mut response = json!({"functionResponse": {"name": r.name, "response": {"result": said(r)}}});
                        if r.id != r.name {
                            response["functionResponse"]["id"] = Value::from(r.id.clone());
                        }
                        response
                    })
                    .collect();
                if !parts.is_empty() {
                    convo.items.push(json!({"role": "user", "parts": parts}));
                }
            }
            Chat => {
                convo.items.extend(reply);
                for r in &results {
                    convo.items.push(json!({"role": "tool", "tool_call_id": r.id, "content": said(r)}));
                }
            }
        }
    }

    /// Streams the next part of the reply. `functions` are Voxen's tools as plain
    /// `{name, description, parameters}`; built-in web search is added where the provider
    /// has it.
    pub async fn stream(
        &self,
        system: &str,
        convo: &Convo,
        functions: &[Value],
        effort: &str,
        timezone: Option<&str>,
    ) -> Result<Events> {
        let functions = if self.provider.tools { functions } else { &[] };
        match self.provider.wire {
            Anthropic => {
                let claude = Claude::new(self.key.clone(), &self.base, self.model.clone());
                let mut tools = Vec::new();
                if self.provider.web {
                    let mut search = json!({"type": "web_search_20260209", "name": "web_search", "max_uses": 15});
                    if let Some(tz) = timezone {
                        search["user_location"] = json!({"type": "approximate", "timezone": tz});
                    }
                    tools.push(search);
                    tools.push(json!({"type": "web_fetch_20260209", "name": "web_fetch", "max_uses": 10}));
                }
                tools.extend(functions.iter().map(|f| {
                    json!({
                        "name": f["name"],
                        "description": f["description"],
                        "input_schema": f["parameters"],
                        "eager_input_streaming": true,
                    })
                }));
                Ok(Box::pin(claude.stream(system, &convo.items, &tools, effort).await?))
            }
            Responses => {
                let mut tools = Vec::new();
                if self.provider.web {
                    let mut search = json!({"type": "web_search"});
                    if let Some(tz) = timezone.filter(|_| self.provider.id == "openai") {
                        search["user_location"] = json!({"type": "approximate", "timezone": tz});
                    }
                    tools.push(search);
                }
                if self.provider.id == "xai" {
                    tools.push(json!({"type": "x_search"}));
                }
                tools.extend(functions.iter().map(|f| {
                    json!({"type": "function", "name": f["name"], "description": f["description"], "parameters": f["parameters"]})
                }));
                let mut body = json!({"model": self.model, "instructions": system, "input": convo.items, "stream": true});
                if let Some(previous) = &convo.previous {
                    body["previous_response_id"] = Value::from(previous.clone());
                }
                with_tools(&mut body, tools);
                let resp = self.send(format!("{}/responses", self.base), body).await?;
                Ok(events(resp, ResponsesParser::default()))
            }
            Gemini => {
                let mut tools = Vec::new();
                if self.provider.web {
                    tools.push(json!({"googleSearch": {}}));
                }
                if !functions.is_empty() {
                    let declarations: Vec<Value> = functions
                        .iter()
                        .map(|f| json!({"name": f["name"], "description": f["description"], "parameters": plain_schema(&f["parameters"])}))
                        .collect();
                    tools.push(json!({"functionDeclarations": declarations}));
                }
                let mut body = json!({"systemInstruction": {"parts": [{"text": system}]}, "contents": convo.items});
                with_tools(&mut body, tools);
                let url = format!("{}/v1beta/models/{}:streamGenerateContent?alt=sse", self.base, self.model);
                let resp = self.send(url, body).await?;
                Ok(events(resp, GeminiParser::default()))
            }
            Chat => {
                let mut messages = vec![json!({"role": "system", "content": system})];
                messages.extend(convo.items.iter().cloned());
                let mut body = json!({"model": self.model, "messages": messages, "stream": true});
                with_tools(&mut body, functions.iter().map(|f| json!({"type": "function", "function": f})).collect());
                let resp = self.send(format!("{}/chat/completions", self.base), body).await?;
                Ok(events(resp, ChatParser::default()))
            }
        }
    }

    /// Posts a request. A model that can't take tools (many local ones) gets asked again
    /// without them, so it still answers.
    async fn send(&self, url: String, mut body: Value) -> Result<reqwest::Response> {
        loop {
            let mut request = self.http.post(&url).json(&body);
            request = match self.provider.wire {
                Gemini => request.header("x-goog-api-key", &self.key),
                _ if !self.key.is_empty() => request.bearer_auth(&self.key),
                _ => request,
            };
            if self.provider.id == "openrouter" {
                request = request.header("X-Title", "Voxen");
            }
            let resp = request
                .send()
                .await
                .with_context(|| format!("calling {}", self.provider.name))?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            let text = resp.text().await.unwrap_or_default();
            let about_tools = ["tool", "function"].iter().any(|w| text.to_lowercase().contains(w));
            if status == 400 && about_tools && body.as_object_mut().and_then(|b| b.remove("tools")).is_some() {
                continue;
            }
            bail!("{} {status}: {text}", self.provider.name);
        }
    }

    /// The models this provider offers right now.
    pub async fn models(&self) -> Result<Vec<String>> {
        if self.provider.id == "ollama" {
            // Ollama's own listing says what each model can do, so speech models stay out.
            let url = format!("{}/api/tags", self.base.trim_end_matches("/v1"));
            let tags: Value = self
                .http
                .get(&url)
                .send()
                .await
                .context("asking Ollama on this Mac for its models")?
                .error_for_status()?
                .json()
                .await?;
            return Ok(ollama_models(&tags, "completion"));
        }
        let (url, request) = match self.provider.wire {
            Anthropic => {
                let url = format!("{}/v1/models?limit=100", self.base);
                let request = self.http.get(&url).header("x-api-key", &self.key).header("anthropic-version", "2023-06-01");
                (url, request)
            }
            Gemini => {
                let url = format!("{}/v1beta/models?pageSize=1000", self.base);
                (url.clone(), self.http.get(&url).header("x-goog-api-key", &self.key))
            }
            _ => {
                let url = format!("{}/models", self.base);
                let request = self.http.get(&url);
                let request = if self.key.is_empty() { request } else { request.bearer_auth(&self.key) };
                (url, request)
            }
        };
        let reply: Value = request
            .send()
            .await
            .with_context(|| format!("asking {} for its models", self.provider.name))?
            .error_for_status()
            .with_context(|| format!("listing models at {url}"))?
            .json()
            .await?;
        Ok(model_names(self.provider, &reply))
    }
}

/// Where Ollama on this Mac listens, without the `/v1` its OpenAI-style API adds.
pub fn ollama_root() -> String {
    find("ollama").map(base_of).unwrap_or_default().trim_end_matches("/v1").to_string()
}

/// The models pulled into Ollama that can do `capability` ("completion" to chat, "speech" to
/// talk), from its `/api/tags` listing. A model that doesn't say what it can do is kept.
pub fn ollama_models(tags: &Value, capability: &str) -> Vec<String> {
    let mut names: Vec<String> = tags["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["capabilities"].as_array().is_none_or(|can| can.iter().any(|c| c == capability)))
        .filter_map(|m| m["name"].as_str().map(|n| n.trim_end_matches(":latest").to_string()))
        .collect();
    names.sort();
    names
}

/// Model ids from a models listing, without the ones that can't chat.
fn model_names(provider: &Provider, reply: &Value) -> Vec<String> {
    let mut names: Vec<String> = if provider.wire == Gemini {
        reply["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["supportedGenerationMethods"].as_array().is_some_and(|ways| ways.iter().any(|w| w == "generateContent")))
            .filter_map(|m| m["name"].as_str()?.strip_prefix("models/").map(String::from))
            .collect()
    } else {
        reply["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect()
    };
    let not_chat = ["embed", "audio", "realtime", "tts", "transcribe", "whisper", "image", "dall-e", "moderation", "search"];
    names.retain(|n| !not_chat.iter().any(|w| n.contains(w)));
    if provider.id == "openai" {
        names.retain(|n| n.starts_with("gpt") || n.starts_with("chatgpt") || n.starts_with('o') && n[1..].starts_with(|c: char| c.is_ascii_digit()));
    }
    names.sort();
    names.dedup();
    names
}

fn with_tools(body: &mut Value, tools: Vec<Value>) {
    if !tools.is_empty() {
        body["tools"] = Value::from(tools);
    }
}

/// A JSON schema without `additionalProperties`, which Gemini doesn't take.
fn plain_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "additionalProperties")
                .map(|(k, v)| (k.clone(), plain_schema(v)))
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(plain_schema).collect()),
        other => other.clone(),
    }
}

/// Reads a provider's stream of server-sent events.
trait Parser: Send {
    fn data(&mut self, data: &str, out: &mut Vec<Event>) -> Result<()>;
    /// The stream ended.
    fn end(&mut self, _out: &mut Vec<Event>) {}
}

/// Splits raw bytes into server-sent events' data.
#[derive(Default)]
struct Split {
    buf: Vec<u8>,
}

impl Split {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend(bytes.iter().filter(|&&b| b != b'\r'));
        let mut out = Vec::new();
        while let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = self.buf.drain(..end + 2).collect();
            out.extend(data_of(&block));
        }
        out
    }

    /// An event the stream ended without its blank line.
    fn rest(&mut self) -> Option<String> {
        data_of(&std::mem::take(&mut self.buf))
    }
}

fn data_of(block: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(block);
    let data: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim).collect();
    (!data.is_empty()).then(|| data.join("\n"))
}

fn events(resp: reqwest::Response, parser: impl Parser + 'static) -> Events {
    let bytes = Box::pin(resp.bytes_stream());
    let state = (bytes, parser, Split::default(), VecDeque::new(), false);
    Box::pin(stream::unfold(state, |(mut bytes, mut parser, mut split, mut queue, mut ended)| async move {
        loop {
            if let Some(item) = queue.pop_front() {
                return Some((item, (bytes, parser, split, queue, ended)));
            }
            if ended {
                return None;
            }
            let mut out = Vec::new();
            let result = match bytes.next().await {
                Some(Ok(chunk)) => split.feed(&chunk).iter().try_for_each(|data| parser.data(data, &mut out)),
                Some(Err(e)) => Err(e.into()),
                None => {
                    ended = true;
                    let rest = split.rest().map_or(Ok(()), |data| parser.data(&data, &mut out));
                    parser.end(&mut out);
                    rest
                }
            };
            queue.extend(out.into_iter().map(Ok));
            if let Err(e) = result {
                queue.push_back(Err(e));
                ended = true;
            }
        }
    }))
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn input_of(arguments: &str) -> Value {
    serde_json::from_str(if arguments.trim().is_empty() { "{}" } else { arguments }).unwrap_or(Value::Null)
}

/// OpenAI's Responses API, which xAI speaks too.
#[derive(Default)]
struct ResponsesParser {
    calls: Vec<String>,
    done: bool,
}

impl Parser for ResponsesParser {
    fn data(&mut self, data: &str, out: &mut Vec<Event>) -> Result<()> {
        let v: Value = serde_json::from_str(data).context("bad event")?;
        match v["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => out.push(Event::Text(str_of(&v["delta"]))),
            "response.output_item.added" => {
                let kind = str_of(&v["item"]["type"]);
                if let Some(tool) = kind.strip_suffix("_call").filter(|t| t.ends_with("search")) {
                    out.push(Event::Using { tool: tool.to_string(), input: json!({}) });
                }
            }
            "response.output_item.done" => self.call(&v["item"], out),
            "response.output_text.annotation.added" => cite(&v["annotation"], out),
            "response.completed" | "response.incomplete" => {
                let response = &v["response"];
                for item in response["output"].as_array().into_iter().flatten() {
                    self.call(item, out);
                    for part in item["content"].as_array().into_iter().flatten() {
                        for note in part["annotations"].as_array().into_iter().flatten() {
                            cite(note, out);
                        }
                    }
                }
                for url in response["citations"].as_array().into_iter().flatten() {
                    cite(&json!({"url": url, "type": "url_citation"}), out);
                }
                self.done = true;
                out.push(Event::Done {
                    stop_reason: if self.calls.is_empty() { "end_turn" } else { "tool_use" }.into(),
                    content: vec![json!({"id": response["id"]})],
                });
            }
            "response.failed" => bail!("{}", response_error(&v["response"]["error"])),
            "error" => bail!("{}", response_error(&v)),
            _ => {}
        }
        Ok(())
    }
}

impl ResponsesParser {
    /// A tool call the bot made, once; it arrives both on its own and in the final reply.
    fn call(&mut self, item: &Value, out: &mut Vec<Event>) {
        let id = str_of(&item["call_id"]);
        if item["type"] != "function_call" || self.calls.contains(&id) {
            return;
        }
        self.calls.push(id.clone());
        let input = input_of(item["arguments"].as_str().unwrap_or_default());
        out.push(Event::Tool { id, name: str_of(&item["name"]), input });
    }
}

fn response_error(v: &Value) -> String {
    v["message"].as_str().or(v["error"]["message"].as_str()).unwrap_or("the provider sent an error").to_string()
}

fn cite(note: &Value, out: &mut Vec<Event>) {
    let url = note["url"].as_str().unwrap_or_default();
    if note["type"] == "url_citation" && !url.is_empty() {
        let title = note["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(url);
        out.push(Event::Cited(Source { url: url.into(), title: title.into() }));
    }
}

/// OpenAI-style Chat Completions, which most providers and local apps speak.
#[derive(Default)]
struct ChatParser {
    text: String,
    /// Tool calls by position: id, name, arguments so far.
    calls: Vec<(String, String, String)>,
    finish: String,
    done: bool,
}

impl Parser for ChatParser {
    fn data(&mut self, data: &str, out: &mut Vec<Event>) -> Result<()> {
        if data == "[DONE]" {
            self.end(out);
            return Ok(());
        }
        let v: Value = serde_json::from_str(data).context("bad event")?;
        if v.get("error").is_some() {
            bail!("{}", response_error(&v["error"]));
        }
        // Perplexity says where its answer came from.
        for page in v["search_results"].as_array().into_iter().flatten() {
            cite(&json!({"type": "url_citation", "url": page["url"], "title": page["title"]}), out);
        }
        for url in v["citations"].as_array().into_iter().flatten() {
            cite(&json!({"type": "url_citation", "url": url}), out);
        }
        let choice = &v["choices"][0];
        if let Some(text) = choice["delta"]["content"].as_str().filter(|t| !t.is_empty()) {
            self.text.push_str(text);
            out.push(Event::Text(text.to_string()));
        }
        for (i, call) in choice["delta"]["tool_calls"].as_array().into_iter().flatten().enumerate() {
            let at = call["index"].as_u64().map_or(i, |n| n as usize);
            if self.calls.len() <= at {
                self.calls.resize(at + 1, Default::default());
            }
            let (id, name, arguments) = &mut self.calls[at];
            id.push_str(call["id"].as_str().unwrap_or_default());
            name.push_str(call["function"]["name"].as_str().unwrap_or_default());
            arguments.push_str(call["function"]["arguments"].as_str().unwrap_or_default());
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = reason.to_string();
        }
        Ok(())
    }

    fn end(&mut self, out: &mut Vec<Event>) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        let mut calls = Vec::new();
        for (i, (id, name, arguments)) in self.calls.iter().enumerate() {
            let id = if id.is_empty() { format!("call_{i}") } else { id.clone() };
            out.push(Event::Tool { id: id.clone(), name: name.clone(), input: input_of(arguments) });
            calls.push(json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}}));
        }
        let mut message = json!({"role": "assistant", "content": self.text});
        if !calls.is_empty() {
            message["tool_calls"] = Value::from(calls);
            if self.text.is_empty() {
                message["content"] = Value::Null;
            }
        }
        let stop_reason = if !self.calls.is_empty() {
            "tool_use"
        } else if self.finish == "content_filter" {
            "refusal"
        } else {
            "end_turn"
        };
        out.push(Event::Done { stop_reason: stop_reason.into(), content: vec![message] });
    }
}

/// Google's Gemini API.
#[derive(Default)]
struct GeminiParser {
    /// Everything the model sent, signatures included, to send back as it was.
    parts: Vec<Value>,
    calls: bool,
    searched: Vec<String>,
    blocked: bool,
}

impl Parser for GeminiParser {
    fn data(&mut self, data: &str, out: &mut Vec<Event>) -> Result<()> {
        let v: Value = serde_json::from_str(data).context("bad event")?;
        if v.get("error").is_some() {
            bail!("{}", response_error(&v["error"]));
        }
        let candidate = &v["candidates"][0];
        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            if part["thought"] != true {
                if let Some(text) = part["text"].as_str() {
                    out.push(Event::Text(text.to_string()));
                }
                if let Some(call) = part.get("functionCall") {
                    self.calls = true;
                    let name = str_of(&call["name"]);
                    let id = call["id"].as_str().map_or_else(|| name.clone(), String::from);
                    out.push(Event::Tool { id, name, input: call.get("args").cloned().unwrap_or_else(|| json!({})) });
                }
            }
            self.parts.push(part.clone());
        }
        let grounding = &candidate["groundingMetadata"];
        for query in grounding["webSearchQueries"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if !self.searched.iter().any(|q| q == query) {
                self.searched.push(query.to_string());
                out.push(Event::Using { tool: "web_search".into(), input: json!({"query": query}) });
            }
        }
        let pages: Vec<Source> = grounding["groundingChunks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let url = c["web"]["uri"].as_str()?;
                let title = c["web"]["title"].as_str().unwrap_or(url);
                Some(Source { url: url.into(), title: title.into() })
            })
            .collect();
        if !pages.is_empty() {
            out.push(Event::Found(pages));
        }
        let finish = candidate["finishReason"].as_str().unwrap_or_default();
        self.blocked |= matches!(finish, "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST") || v["promptFeedback"]["blockReason"].is_string();
        Ok(())
    }

    fn end(&mut self, out: &mut Vec<Event>) {
        let stop_reason = if self.calls {
            "tool_use"
        } else if self.blocked {
            "refusal"
        } else {
            "end_turn"
        };
        out.push(Event::Done { stop_reason: stop_reason.into(), content: std::mem::take(&mut self.parts) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mut parser: impl Parser, stream: &str) -> Vec<Event> {
        let mut split = Split::default();
        let mut out = Vec::new();
        for data in split.feed(stream.as_bytes()) {
            parser.data(&data, &mut out).unwrap();
        }
        if let Some(data) = split.rest() {
            parser.data(&data, &mut out).unwrap();
        }
        parser.end(&mut out);
        out
    }

    fn llm(id: &str) -> Llm {
        Brains::offline().llm(id, "").unwrap()
    }

    #[test]
    fn ollama_lists_models_by_what_they_can_do() {
        let tags = json!({"models": [
            {"name": "phonon:latest", "capabilities": ["speech"]},
            {"name": "gemma4:e4b-it-qat", "capabilities": ["completion", "vision", "audio", "tools", "thinking"]},
            {"name": "llama3.2:latest"},
        ]});
        assert_eq!(ollama_models(&tags, "completion"), ["gemma4:e4b-it-qat", "llama3.2"]);
        assert_eq!(ollama_models(&tags, "speech"), ["llama3.2", "phonon"]);
        assert!(ollama_models(&json!({}), "speech").is_empty());
    }

    #[test]
    fn every_provider_is_listed_once_with_a_way_in() {
        for (i, p) in PROVIDERS.iter().enumerate() {
            assert!(!PROVIDERS[..i].iter().any(|q| q.id == p.id), "{} twice", p.id);
            assert!(p.key.is_some() || p.base.starts_with("http://localhost"), "{} has no key", p.id);
        }
        assert_eq!(find("OpenAI").unwrap().wire, Responses);
        assert!(find("nobody").is_none());
        assert!(Brains::offline().llm("lmstudio", "").is_err()); // no default model to fall back on
        assert_eq!(Brains::offline().llm("lmstudio", "qwen3").unwrap().model, "qwen3");
    }

    #[test]
    fn responses_stream_gives_searches_text_quotes_and_tool_calls() {
        let events = run(
            ResponsesParser::default(),
            r#"event: response.output_item.added
data: {"type":"response.output_item.added","item":{"type":"web_search_call","id":"ws1"}}

data: {"type":"response.output_text.delta","delta":"Sunny"}

data: {"type":"response.output_text.annotation.added","annotation":{"type":"url_citation","url":"https://w.example","title":"Forecast"}}

data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"c1","name":"remember","arguments":"{\"fact\":\"Likes sun\"}"}}

data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"function_call","call_id":"c1","name":"remember","arguments":"{}"}]}}

"#,
        );
        assert_eq!(
            events,
            vec![
                Event::Using { tool: "web_search".into(), input: json!({}) },
                Event::Text("Sunny".into()),
                Event::Cited(Source { url: "https://w.example".into(), title: "Forecast".into() }),
                Event::Tool { id: "c1".into(), name: "remember".into(), input: json!({"fact": "Likes sun"}) },
                Event::Done { stop_reason: "tool_use".into(), content: vec![json!({"id": "resp_1"})] },
            ]
        );
    }

    #[test]
    fn chat_stream_gathers_tool_calls_split_across_chunks() {
        let events = run(
            ChatParser::default(),
            r#"data: {"choices":[{"delta":{"content":"On it. "}}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"t1","function":{"name":"schedule","arguments":"{\"task\":"}}]}}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Stretch\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
        );
        assert_eq!(events[0], Event::Text("On it. ".into()));
        assert_eq!(events[1], Event::Tool { id: "t1".into(), name: "schedule".into(), input: json!({"task": "Stretch"}) });
        let Event::Done { stop_reason, content } = &events[2] else { panic!("{events:?}") };
        assert_eq!(stop_reason, "tool_use");
        assert_eq!(content[0]["tool_calls"][0]["function"]["arguments"], "{\"task\":\"Stretch\"}");
        assert_eq!(events.len(), 3, "the end after [DONE] adds nothing");
    }

    #[test]
    fn gemini_stream_gives_searches_sources_and_calls_and_keeps_signatures() {
        let events = run(
            GeminiParser::default(),
            r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"Lisbon is ","thoughtSignature":"sig"}]}}]}

data: {"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"remember","args":{"fact":"Loves Lisbon"}}}]},"groundingMetadata":{"webSearchQueries":["lisbon weather"],"groundingChunks":[{"web":{"uri":"https://g.example/x","title":"weather.com"}}]}}]}"#,
        );
        assert_eq!(events[0], Event::Text("Lisbon is ".into()));
        assert_eq!(events[1], Event::Tool { id: "remember".into(), name: "remember".into(), input: json!({"fact": "Loves Lisbon"}) });
        assert_eq!(events[2], Event::Using { tool: "web_search".into(), input: json!({"query": "lisbon weather"}) });
        assert_eq!(events[3], Event::Found(vec![Source { url: "https://g.example/x".into(), title: "weather.com".into() }]));
        let Event::Done { stop_reason, content } = &events[4] else { panic!("{events:?}") };
        assert_eq!(stop_reason, "tool_use");
        assert_eq!(content[0]["thoughtSignature"], "sig");
    }

    #[test]
    fn a_turn_with_a_picture_reaches_each_wire_in_its_own_form() {
        let content = [
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}}),
            json!({"type": "document", "title": "notes.txt", "source": {"type": "text", "media_type": "text/plain", "data": "hi"}}),
            json!({"type": "text", "text": "You: what's this?"}),
        ];
        let openai = llm("openai").start(&content).items[0].clone();
        assert_eq!(openai["content"][0]["image_url"], "data:image/png;base64,QUJD");
        let gemini = llm("google").start(&content).items[0].clone();
        assert_eq!(gemini["parts"][0]["inlineData"]["mimeType"], "image/png");
        let chat = llm("mistral").start(&content).items[0].clone();
        assert_eq!(chat["content"][0]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert_eq!(chat["content"][1]["text"], "File notes.txt:\nhi");
        // Without pictures, plain text, which every Chat Completions API takes.
        let plain = llm("deepseek").start(&content[1..]).items[0].clone();
        assert_eq!(plain["content"], "File notes.txt:\nhi\n\nYou: what's this?");
    }

    #[test]
    fn tool_results_go_back_in_each_wires_form() {
        let result = || vec![ToolResult { id: "c1".into(), name: "remember".into(), output: "Saved.".into(), failed: false }];
        let openai = llm("openai");
        let mut convo = openai.start(&[json!({"type": "text", "text": "hi"})]);
        openai.carry_on(&mut convo, vec![json!({"id": "resp_1"})], result());
        assert_eq!(convo.previous.as_deref(), Some("resp_1"));
        assert_eq!(convo.items, [json!({"type": "function_call_output", "call_id": "c1", "output": "Saved."})]);

        let chat = llm("groq");
        let mut convo = chat.start(&[json!({"type": "text", "text": "hi"})]);
        chat.carry_on(&mut convo, vec![json!({"role": "assistant", "content": null})], result());
        assert_eq!(convo.items[2], json!({"role": "tool", "tool_call_id": "c1", "content": "Saved."}));

        let gemini = llm("google");
        let mut convo = gemini.start(&[json!({"type": "text", "text": "hi"})]);
        let mut by_name = result();
        by_name[0].id = "remember".into();
        gemini.carry_on(&mut convo, vec![json!({"functionCall": {"name": "remember"}})], by_name);
        assert_eq!(convo.items[2]["parts"][0]["functionResponse"], json!({"name": "remember", "response": {"result": "Saved."}}));
    }

    #[test]
    fn model_lists_keep_only_chat_models() {
        let openai = find("openai").unwrap();
        let listed = json!({"data": [{"id": "gpt-5.6"}, {"id": "o4-mini"}, {"id": "text-embedding-3-large"}, {"id": "gpt-realtime"}, {"id": "dall-e-3"}, {"id": "omni-moderation-latest"}]});
        assert_eq!(model_names(openai, &listed), ["gpt-5.6", "o4-mini"]);
        let google = find("google").unwrap();
        let listed = json!({"models": [
            {"name": "models/gemini-3.5-flash", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"]}
        ]});
        assert_eq!(model_names(google, &listed), ["gemini-3.5-flash"]);
    }

    #[test]
    fn gemini_schemas_lose_additional_properties() {
        let schema = json!({"type": "object", "additionalProperties": false, "properties": {"a": {"type": "object", "additionalProperties": false}}});
        assert_eq!(plain_schema(&schema), json!({"type": "object", "properties": {"a": {"type": "object"}}}));
    }
}
