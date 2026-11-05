//! What bots can do besides talk: search the web and read pages (with providers that can),
//! remember things about you, set reminders and repeating tasks, look at files you attach,
//! and, with an xAI key, search X and make pictures with Grok Imagine.

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Days, Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// How hard the bots work. Claude decides on every reply how much to think and whether to
/// research, like Grok's auto mode; medium keeps quick questions quick on a call.
pub const EFFORT: &str = "medium";

/// A file you attached for the bots to look at.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Attachment {
    pub name: String,
    /// "image/png", "application/pdf", "text/plain", …
    pub media_type: String,
    /// Base64 for pictures and PDFs, the text itself for text files.
    pub data: String,
}

impl Attachment {
    /// The attachment as message content for Claude.
    pub fn block(&self) -> Value {
        if self.media_type.starts_with("image/") {
            json!({"type": "image", "source": {"type": "base64", "media_type": self.media_type, "data": self.data}})
        } else if self.media_type == "application/pdf" {
            json!({"type": "document", "title": self.name, "source": {"type": "base64", "media_type": "application/pdf", "data": self.data}})
        } else {
            json!({"type": "document", "title": self.name, "source": {"type": "text", "media_type": "text/plain", "data": self.data}})
        }
    }
}

/// Voxen's own tools, as plain `{name, description, parameters}` that each provider's client
/// puts in its own form. X search and pictures need the xAI key; `native_x` leaves X search
/// out for Grok, which searches X by itself.
pub fn functions(xai: bool, native_x: bool) -> Vec<Value> {
    let mut tools = vec![
        tool(
            "remember",
            "Save a lasting fact about the user, like a preference, a name or a plan, so every \
             bot knows it in later chats. Save only what the user would want kept.",
            json!({"fact": {"type": "string", "description": "One short sentence, like \"Prefers window seats\"."}}),
        ),
        tool(
            "forget",
            "Delete saved facts that contain these words.",
            json!({"words": {"type": "string"}}),
        ),
        tool(
            "schedule",
            "Set a reminder or a repeating task. At that time you get the task as a message in \
             this chat and answer it out loud, so write it as what to do then, like \"Remind \
             them to call Mum\" or \"Brief them on today's top news\".",
            json!({
                "task": {"type": "string"},
                "at": {"type": "string", "description": "The user's local date and time, as YYYY-MM-DD HH:MM."},
                "repeat": {"type": "string", "enum": ["never", "daily", "weekly"]},
            }),
        ),
        tool(
            "list_tasks",
            "List the reminders and repeating tasks that are set.",
            json!({}),
        ),
        tool(
            "cancel_task",
            "Cancel a reminder or repeating task by its number from list_tasks.",
            json!({"number": {"type": "integer"}}),
        ),
    ];
    if xai && !native_x {
        tools.push(tool(
            "search_x",
            "Search posts on X for what people are saying right now: reactions, breaking news, \
             trends, or someone's posts.",
            json!({"query": {"type": "string"}}),
        ));
    }
    if xai {
        tools.push(tool(
            "create_image",
            "Make a picture with Grok Imagine. It appears in the chat.",
            json!({"prompt": {"type": "string", "description": "A detailed description of the picture."}}),
        ));
    }
    tools
}

fn tool(name: &str, description: &str, properties: Value) -> Value {
    let required: Vec<String> = properties
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    json!({
        "name": name,
        "description": description,
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

/// What a bot can do with the brain it has.
#[derive(Debug, Clone, Copy)]
pub struct Abilities {
    /// It can search the web.
    pub web: bool,
    /// It can use Voxen's tools: memory, reminders and the rest.
    pub tools: bool,
    /// The xAI key is set: X search and pictures.
    pub xai: bool,
}

/// What a bot needs to know to use its tools well, added to its system prompt.
pub fn guidance(memory: &[String], now: DateTime<Local>, can: Abilities) -> String {
    let mut text = format!("\n\nIt's {} where the user is.", now.format("%A %-d %B %Y, %H:%M"));
    if can.web {
        text.push_str(
            " You can search the web and read pages: do it for anything current or that you \
             aren't sure of, like news, weather, prices, scores, times and places, and give the \
             answer in your own words, without links. Decide for yourself how much each question \
             needs: answer simple things straight away, and research anything that needs it \
             properly, searching from a few angles and reading the best sources, then say what's \
             still uncertain.",
        );
    } else {
        text.push_str(" You can't search the web, so say so when an answer may be out of date.");
    }
    if can.tools {
        if can.xai {
            text.push_str(
                " You can search X for what people are saying right now, and make pictures with \
                 create_image.",
            );
        }
        text.push_str(
            " When the user tells you something worth keeping, like a preference or a name, save \
             it with remember. Use schedule for reminders and repeating tasks. A line from \
             \"Scheduled task\" is one of those firing now: do it.",
        );
    }
    if !memory.is_empty() {
        text.push_str("\n\nWhat you know about the user from earlier chats:\n- ");
        text.push_str(&memory.join("\n- "));
    }
    text
}

/// What a bot is doing with a tool, for the screen.
pub fn step(tool: &str, input: &Value) -> String {
    let field = |key: &str| input[key].as_str().unwrap_or_default().trim().to_string();
    match tool {
        "web_search" if field("query").is_empty() => "Searching the web".into(),
        "web_search" => format!("Searching the web for “{}”", field("query")),
        "search_x" | "x_search" if field("query").is_empty() => "Searching X".into(),
        "web_fetch" => format!("Reading {}", host(&field("url"))),
        "search_x" | "x_search" => format!("Searching X for “{}”", field("query")),
        "create_image" => "Making a picture".into(),
        "remember" => format!("Remembering: {}", field("fact")),
        "forget" => format!("Forgetting “{}”", field("words")),
        "schedule" => format!("Scheduling: {}", field("task")),
        "list_tasks" => "Checking your reminders".into(),
        "cancel_task" => "Cancelling a reminder".into(),
        other => format!("Using {other}"),
    }
}

/// Something to say while a slow tool runs, so a call never goes quiet.
pub fn filler(tool: &str) -> Option<&'static str> {
    match tool {
        "web_search" => Some("Let me look that up."),
        "web_fetch" => Some("Let me read that."),
        "search_x" | "x_search" => Some("Let me check X."),
        "create_image" => Some("Making that now."),
        _ => None,
    }
}

fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or(rest);
    host.trim_start_matches("www.").to_string()
}

/// The Mac's time zone, like "Asia/Kolkata".
pub fn timezone() -> Option<String> {
    let link = std::fs::read_link("/etc/localtime").ok()?;
    let link = link.to_string_lossy();
    link.split_once("zoneinfo/").map(|(_, tz)| tz.to_string())
}

/// Facts the bots remember about you, kept as a list in a Markdown file you can edit.
#[derive(Default)]
pub struct Memory {
    path: Option<PathBuf>,
    pub facts: Vec<String>,
}

impl Memory {
    pub fn load(path: Option<PathBuf>) -> Self {
        let text = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let facts = text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("- "))
            .map(str::to_string)
            .collect();
        Self { path, facts }
    }

    pub fn remember(&mut self, fact: &str) -> Result<String> {
        let fact = fact.split_whitespace().collect::<Vec<_>>().join(" ");
        ensure!(!fact.is_empty(), "say what to remember");
        if !self.facts.iter().any(|f| f.eq_ignore_ascii_case(&fact)) {
            self.facts.push(fact);
            self.save()?;
        }
        Ok("Saved. Every bot knows it from now on.".into())
    }

    pub fn forget(&mut self, words: &str) -> Result<String> {
        let words = words.trim().to_lowercase();
        ensure!(!words.is_empty(), "say what to forget");
        let before = self.facts.len();
        self.facts.retain(|f| !f.to_lowercase().contains(&words));
        self.save()?;
        Ok(match before - self.facts.len() {
            0 => "Nothing saved matched that.".into(),
            1 => "Forgot 1 fact.".into(),
            n => format!("Forgot {n} facts."),
        })
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut text = String::from("# What Voxen's bots remember about you. Edit or delete lines freely.\n\n");
        for fact in &self.facts {
            text.push_str(&format!("- {fact}\n"));
        }
        write(path, &text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    Never,
    Daily,
    Weekly,
}

/// A reminder or repeating task: at `at`, `bot` gets `task` in `chat` and answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub chat: String,
    pub bot: String,
    pub task: String,
    pub at: DateTime<Local>,
    pub repeat: Repeat,
}

impl Task {
    /// Builds a task from what a bot asked for.
    pub fn new(chat: &str, bot: &str, input: &Value, now: DateTime<Local>) -> Result<Self> {
        let task = input["task"].as_str().unwrap_or_default().trim();
        ensure!(!task.is_empty(), "say what the task is");
        let repeat = serde_json::from_value(input["repeat"].clone()).unwrap_or(Repeat::Never);
        let mut at = parse_time(input["at"].as_str().unwrap_or_default())?;
        if repeat == Repeat::Never {
            let now_text = now.format("%Y-%m-%d %H:%M");
            ensure!(at > now, "that time has passed; it's {now_text} now");
        }
        while at <= now {
            at = next(at, repeat);
        }
        Ok(Self {
            chat: chat.into(),
            bot: bot.into(),
            task: task.into(),
            at,
            repeat,
        })
    }

    fn said(&self) -> String {
        let when = match self.repeat {
            Repeat::Never => self.at.format("%a %-d %b at %H:%M").to_string(),
            Repeat::Daily => format!("every day at {}", self.at.format("%H:%M")),
            Repeat::Weekly => format!("every {} at {}", self.at.format("%A"), self.at.format("%H:%M")),
        };
        format!("{when}, {}: {}", self.bot, self.task)
    }
}

/// Reads a local time written "YYYY-MM-DD HH:MM" (a "T" in the middle works too).
fn parse_time(at: &str) -> Result<DateTime<Local>> {
    let at = at.trim().replace('T', " ");
    let naive = NaiveDateTime::parse_from_str(&at, "%Y-%m-%d %H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(&at, "%Y-%m-%d %H:%M:%S"))
        .context("write the time as YYYY-MM-DD HH:MM")?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .context("that time is skipped by a clock change")
}

/// The task's next time: the same clock time a day or a week later.
fn next(at: DateTime<Local>, repeat: Repeat) -> DateTime<Local> {
    let days = if repeat == Repeat::Weekly { 7 } else { 1 };
    Local
        .from_local_datetime(&(at.naive_local() + Days::new(days)))
        .earliest()
        .unwrap_or(at + chrono::Duration::days(days as i64))
}

/// Your reminders and repeating tasks, kept in a JSON file.
#[derive(Default)]
pub struct Tasks {
    path: Option<PathBuf>,
    pub list: Vec<Task>,
}

impl Tasks {
    pub fn load(path: Option<PathBuf>) -> Self {
        let list = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, list }
    }

    pub fn add(&mut self, task: Task) -> Result<String> {
        let said = task.said();
        self.list.push(task);
        self.list.sort_by_key(|t| t.at);
        self.save()?;
        Ok(format!("Set for {said}."))
    }

    pub fn cancel(&mut self, number: usize) -> Result<String> {
        ensure!(
            (1..=self.list.len()).contains(&number),
            "there's no task {number}; check list_tasks"
        );
        let task = self.list.remove(number - 1);
        self.save()?;
        Ok(format!("Cancelled the one for {}.", task.said()))
    }

    pub fn said(&self) -> String {
        if self.list.is_empty() {
            return "No reminders or tasks are set.".into();
        }
        let lines: Vec<String> = self
            .list
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}. {}", i + 1, t.said()))
            .collect();
        lines.join("\n")
    }

    /// Takes a task that's due at `now`. A repeating one stays, moved to its next time.
    pub fn take_due(&mut self, now: DateTime<Local>) -> Option<Task> {
        let i = self.list.iter().position(|t| t.at <= now)?;
        let task = self.list[i].clone();
        if task.repeat == Repeat::Never {
            self.list.remove(i);
        } else {
            let mut at = task.at;
            while at <= now {
                at = next(at, task.repeat);
            }
            self.list[i].at = at;
            self.list.sort_by_key(|t| t.at);
        }
        if let Err(e) = self.save() {
            eprintln!("Voxen couldn't save your tasks: {e:#}");
        }
        Some(task)
    }

    fn save(&self) -> Result<()> {
        match &self.path {
            Some(path) => write(path, &serde_json::to_string_pretty(&self.list)?),
            None => Ok(()),
        }
    }
}

fn write(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text).with_context(|| format!("saving {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, day, hour, minute, 0).unwrap()
    }

    #[test]
    fn memory_keeps_each_fact_once_and_forgets_by_words() {
        let dir = std::env::temp_dir().join(format!("voxen-memory-{}", std::process::id()));
        let path = dir.join("memory.md");
        let mut memory = Memory::load(Some(path.clone()));
        memory.remember("Prefers  window seats").unwrap();
        memory.remember("prefers window seats").unwrap();
        memory.remember("Has a dog called Biscuit").unwrap();
        assert_eq!(memory.facts, ["Prefers window seats", "Has a dog called Biscuit"]);
        assert_eq!(Memory::load(Some(path.clone())).facts, memory.facts);
        assert_eq!(memory.forget("DOG").unwrap(), "Forgot 1 fact.");
        assert_eq!(memory.forget("cat").unwrap(), "Nothing saved matched that.");
        assert_eq!(Memory::load(Some(path)).facts, ["Prefers window seats"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn task_times_are_read_and_checked() {
        let now = at(5, 10, 0);
        let task = |at: &str, repeat: &str| {
            Task::new("main", "Maya", &json!({"task": "Brief me", "at": at, "repeat": repeat}), now)
        };
        assert_eq!(task("2026-11-05 18:30", "never").unwrap().at, at(5, 18, 30));
        assert_eq!(task("2026-11-05T18:30:00", "never").unwrap().at, at(5, 18, 30));
        assert!(task("2026-11-05 09:00", "never").unwrap_err().to_string().contains("passed"));
        assert!(task("tomorrow at 9", "never").is_err());
        // A daily time that's already gone today starts tomorrow.
        assert_eq!(task("2026-11-05 08:00", "daily").unwrap().at, at(6, 8, 0));
    }

    #[test]
    fn due_tasks_fire_once_and_repeating_ones_move_on() {
        let mut tasks = Tasks::default();
        let task = |text: &str, at, repeat| Task {
            chat: "main".into(),
            bot: "Maya".into(),
            task: text.into(),
            at,
            repeat,
        };
        tasks.add(task("Call Mum", at(5, 9, 0), Repeat::Never)).unwrap();
        tasks.add(task("News", at(5, 8, 0), Repeat::Daily)).unwrap();
        let now = at(5, 9, 30);
        assert_eq!(tasks.take_due(now).unwrap().task, "News");
        assert_eq!(tasks.take_due(now).unwrap().task, "Call Mum");
        assert_eq!(tasks.take_due(now), None);
        assert_eq!(tasks.list.len(), 1);
        assert_eq!(tasks.list[0].at, at(6, 8, 0));
        assert!(tasks.said().starts_with("1. every day at 08:00, Maya: News"));
        assert!(tasks.cancel(2).is_err());
        tasks.cancel(1).unwrap();
        assert_eq!(tasks.said(), "No reminders or tasks are set.");
    }

    #[test]
    fn x_and_pictures_need_the_xai_key() {
        let names = |tools: &[Value]| -> Vec<String> {
            tools.iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
        };
        let plain = names(&functions(false, false));
        assert!(!plain.contains(&"search_x".to_string()) && !plain.contains(&"create_image".to_string()));
        let with_xai = names(&functions(true, false));
        assert!(with_xai.contains(&"search_x".to_string()) && with_xai.contains(&"create_image".to_string()));
        assert!(!names(&functions(true, true)).contains(&"search_x".to_string()), "Grok searches X itself");
        assert_eq!(functions(false, false)[0]["parameters"]["required"], json!(["fact"]));
    }

    #[test]
    fn steps_say_what_a_bot_is_doing() {
        let url = json!({"url": "https://www.bbc.co.uk/news/1"});
        assert_eq!(step("web_fetch", &url), "Reading bbc.co.uk");
        assert_eq!(
            step("web_search", &json!({"query": "lisbon"})),
            "Searching the web for “lisbon”"
        );
    }

    #[test]
    fn attachments_become_content_blocks() {
        let file = |media_type: &str| Attachment {
            name: "f".into(),
            media_type: media_type.into(),
            data: "abc".into(),
        };
        assert_eq!(file("image/png").block()["type"], "image");
        assert_eq!(file("application/pdf").block()["source"]["type"], "base64");
        assert_eq!(file("text/markdown").block()["source"]["type"], "text");
    }
}
