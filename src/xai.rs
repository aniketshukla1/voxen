//! xAI's API, for Grok's own tools: X search and Grok Imagine pictures. Optional; it needs an
//! xAI key.

use crate::claude::Source;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub struct Xai {
    http: reqwest::Client,
    key: String,
    model: String,
    image_model: String,
}

const BASE: &str = "https://api.x.ai/v1";

impl Xai {
    /// Reads `XAI_API_KEY`, and optionally `XAI_MODEL` and `XAI_IMAGE_MODEL`.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("XAI_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .context("XAI_API_KEY is not set")?;
        Ok(Self {
            http: reqwest::Client::new(),
            key,
            model: std::env::var("XAI_MODEL").unwrap_or_else(|_| "grok-4.7".into()),
            image_model: std::env::var("XAI_IMAGE_MODEL")
                .unwrap_or_else(|_| "grok-imagine-image-2.0".into()),
        })
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{BASE}/{path}"))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .context("calling xAI")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("xAI {status}: {text}");
        }
        serde_json::from_str(&text).context("bad reply from xAI")
    }

    /// Has Grok search X for `query`. Returns its summary and the posts it used.
    pub async fn search_x(&self, query: &str) -> Result<(String, Vec<Source>)> {
        let ask = format!(
            "Search X for: {query}\nSum up what people are saying, quoting the most relevant \
             posts and who wrote them."
        );
        let body = json!({
            "model": self.model,
            "input": [{"role": "user", "content": ask}],
            "tools": [{"type": "x_search"}],
        });
        Ok(found(&self.post("responses", body).await?))
    }

    /// Makes a picture of `prompt`. Returns its link and the image file's bytes.
    pub async fn image(&self, prompt: &str) -> Result<(String, Vec<u8>)> {
        let body = json!({"model": self.image_model, "prompt": prompt});
        let reply = self.post("images/generations", body).await?;
        let url = reply["data"][0]["url"]
            .as_str()
            .context("xAI sent no picture")?
            .to_string();
        let bytes = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok((url, bytes.to_vec()))
    }
}

/// The answer and its sources in a Responses API reply.
fn found(reply: &Value) -> (String, Vec<Source>) {
    let mut text = String::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for item in reply["output"].as_array().into_iter().flatten() {
        for part in item["content"].as_array().into_iter().flatten() {
            if part["type"] == "output_text" {
                text.push_str(part["text"].as_str().unwrap_or_default());
                for note in part["annotations"].as_array().into_iter().flatten() {
                    links.push((str_of(&note["url"]), str_of(&note["title"])));
                }
            }
        }
    }
    for cited in reply["citations"].as_array().into_iter().flatten() {
        let url = cited.as_str().map(String::from).unwrap_or_else(|| str_of(&cited["url"]));
        links.push((url, str_of(&cited["title"])));
    }
    let mut sources: Vec<Source> = Vec::new();
    for (url, title) in links {
        if !url.is_empty() && !sources.iter().any(|s| s.url == url) {
            let title = if title.is_empty() { url.clone() } else { title };
            sources.push(Source { url, title });
        }
    }
    (text, sources)
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_search_reply_gives_the_summary_and_the_posts() {
        let reply = json!({
            "output": [
                {"type": "x_search_call", "status": "completed"},
                {"type": "message", "content": [{
                    "type": "output_text",
                    "text": "People love it.",
                    "annotations": [{"type": "url_citation", "url": "https://x.com/a/status/1", "title": "@a"}]
                }]}
            ],
            "citations": ["https://x.com/a/status/1", "https://x.com/b/status/2"]
        });
        let (text, sources) = found(&reply);
        assert_eq!(text, "People love it.");
        assert_eq!(
            sources,
            [
                Source { url: "https://x.com/a/status/1".into(), title: "@a".into() },
                Source { url: "https://x.com/b/status/2".into(), title: "https://x.com/b/status/2".into() },
            ]
        );
    }
}
