//! What bots can do besides talk: search the web and read pages (with providers that can),
//! remember things about you, set reminders and repeating tasks, look at files you attach,
//! and, with an xAI key, search X and make pictures with Grok Imagine.

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDateTime, TimeZone, Weekday};
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
            "Set a reminder, or a task that comes back at any interval: every few minutes (5 \
             or more), hours, days, weeks or months, or every weekday. Each time it runs you get \
             the task as a message in this chat and answer it out loud, so write it as what to do \
             then, like \"Check the Bitcoin price and say how it moved\" or \"Brief them on \
             today's top AI news\".",
            json!({
                "task": {"type": "string"},
                "at": {"type": "string", "description": "The first run, in the user's local time, as YYYY-MM-DD HH:MM."},
                "repeat": {"type": "string", "enum": ["never", "minutes", "hours", "days", "weeks", "months", "weekdays"]},
                "every": {"type": "integer", "description": "How many minutes, hours, days, weeks or months between runs; 1 unless the user said otherwise."},
            }),
        ),
        tool(
            "list_tasks",
            "List every bot's reminders and scheduled tasks.",
            json!({}),
        ),
        tool_needing(
            "change_task",
            "Change a reminder or scheduled task by its number from list_tasks: what it does, \
             when it runs next, how often it repeats, or pause and resume it. Give only what \
             changes.",
            json!({
                "number": {"type": "integer"},
                "task": {"type": "string"},
                "at": {"type": "string", "description": "The next run, in the user's local time, as YYYY-MM-DD HH:MM."},
                "repeat": {"type": "string", "enum": ["never", "minutes", "hours", "days", "weeks", "months", "weekdays"]},
                "every": {"type": "integer"},
                "paused": {"type": "boolean"},
            }),
            &["number"],
        ),
        tool(
            "cancel_task",
            "Cancel a reminder or scheduled task by its number from list_tasks.",
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

/// A tool whose parameters are optional, apart from `required`.
fn tool_needing(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut tool = tool(name, description, properties);
    tool["parameters"]["required"] = json!(required);
    tool
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
             it with remember. Use schedule for reminders and for tasks the user wants done again \
             and again, at whatever interval they say. A line from \"Scheduled task\" is one of \
             yours running now: do it, and if it ran before in this chat, say what changed. To \
             change, pause or resume a task, check list_tasks, then use change_task.",
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
        "list_tasks" => "Checking the tasks".into(),
        "change_task" => "Changing a task".into(),
        "cancel_task" => "Cancelling a task".into(),
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

/// How often a task comes back; the task's `every` says how many of these between runs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    Never,
    Minutes,
    Hours,
    #[serde(alias = "daily")]
    Days,
    #[serde(alias = "weekly")]
    Weeks,
    Months,
    /// Monday to Friday.
    Weekdays,
}

/// The shortest gap between runs, so a task can't run up a bill by the minute.
const MIN_MINUTES: u32 = 5;

fn one() -> u32 {
    1
}

/// A bot's scheduled task: at `at`, `bot` gets `task` in `chat` and answers it, then the task
/// comes back as `repeat` and `every` say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub chat: String,
    pub bot: String,
    pub task: String,
    /// The next run.
    pub at: DateTime<Local>,
    pub repeat: Repeat,
    #[serde(default = "one")]
    pub every: u32,
    #[serde(default)]
    pub paused: bool,
}

impl Task {
    /// Builds a task from a bot's tool call: `task`, `at`, `repeat` and `every`.
    pub fn new(chat: &str, bot: &str, input: &Value, now: DateTime<Local>) -> Result<Self> {
        let task = input["task"].as_str().unwrap_or_default().trim();
        ensure!(!task.is_empty(), "say what the task is");
        let repeat: Repeat = serde_json::from_value(input["repeat"].clone()).unwrap_or(Repeat::Never);
        let every = input["every"].as_u64().unwrap_or(1).clamp(1, 10_000) as u32;
        ensure!(
            repeat != Repeat::Minutes || every >= MIN_MINUTES,
            "the shortest repeat is every {MIN_MINUTES} minutes"
        );
        let at = parse_time(input["at"].as_str().unwrap_or_default())?;
        if repeat == Repeat::Never {
            let now_text = now.format("%Y-%m-%d %H:%M");
            ensure!(at > now, "that time has passed; it's {now_text} now");
        }
        let mut task = Self {
            chat: chat.into(),
            bot: bot.into(),
            task: task.into(),
            at,
            repeat,
            every,
            paused: false,
        };
        task.move_past(now);
        Ok(task)
    }

    /// Moves a repeating task's next run past `now`, skipping the runs it missed, and onto
    /// a weekday when it only runs on weekdays.
    fn move_past(&mut self, now: DateTime<Local>) {
        if self.repeat == Repeat::Never {
            return;
        }
        while self.at <= now || (self.repeat == Repeat::Weekdays && weekend(self.at)) {
            self.at = next(self.at, self.repeat, self.every);
        }
    }

    /// When it runs, in words: "every 2 hours", "every weekday at 08:00", "Fri 6 Nov at 09:00".
    pub fn when(&self) -> String {
        let (time, n) = (self.at.format("%H:%M"), self.every);
        let unit = |one: &str, many: &str| if n == 1 { one.to_string() } else { format!("{n} {many}") };
        match self.repeat {
            Repeat::Never => self.at.format("%a %-d %b at %H:%M").to_string(),
            Repeat::Minutes => format!("every {}", unit("minute", "minutes")),
            Repeat::Hours => format!("every {}", unit("hour", "hours")),
            Repeat::Days => format!("every {} at {time}", unit("day", "days")),
            Repeat::Weeks if n == 1 => format!("every {} at {time}", self.at.format("%A")),
            Repeat::Weeks => format!("every {n} weeks on {} at {time}", self.at.format("%A")),
            Repeat::Months => format!("every {} on the {} at {time}", unit("month", "months"), ordinal(self.at.day())),
            Repeat::Weekdays => format!("every weekday at {time}"),
        }
    }

    fn said(&self) -> String {
        let next = match (self.paused, self.repeat) {
            (true, _) => " (paused)".to_string(),
            (false, Repeat::Never) => String::new(),
            (false, _) => format!(" (next {})", self.at.format("%a %-d %b %H:%M")),
        };
        format!("{}{next}, {}: {}", self.when(), self.bot, self.task)
    }
}

fn weekend(at: DateTime<Local>) -> bool {
    matches!(at.weekday(), Weekday::Sat | Weekday::Sun)
}

fn ordinal(day: u32) -> String {
    let suffix = match (day % 10, day % 100) {
        (1, n) if n != 11 => "st",
        (2, n) if n != 12 => "nd",
        (3, n) if n != 13 => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
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

/// The run after `at`. Minutes and hours are fixed gaps; days, weeks, months and weekdays
/// keep the same time on the clock.
fn next(at: DateTime<Local>, repeat: Repeat, every: u32) -> DateTime<Local> {
    let n = every.max(1);
    let wall = at.naive_local();
    let later = match repeat {
        Repeat::Never => return at,
        Repeat::Minutes => return at + chrono::Duration::minutes(n.into()),
        Repeat::Hours => return at + chrono::Duration::hours(n.into()),
        Repeat::Days => wall + Days::new(n.into()),
        Repeat::Weeks => wall + Days::new(7 * u64::from(n)),
        Repeat::Months => wall.checked_add_months(Months::new(n)).unwrap_or(wall + Days::new(30)),
        Repeat::Weekdays => {
            let mut day = wall + Days::new(1);
            while matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
                day = day + Days::new(1);
            }
            day
        }
    };
    Local.from_local_datetime(&later).earliest().unwrap_or(at + (later - wall))
}

/// The bots' scheduled tasks, kept in a JSON file.
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
        Ok(format!("Set: {said}."))
    }

    /// Cancels a task by its number in `said()`.
    pub fn cancel(&mut self, number: usize) -> Result<String> {
        ensure!(
            (1..=self.list.len()).contains(&number),
            "there's no task {number}; check list_tasks"
        );
        let task = self.list.remove(number - 1);
        self.save()?;
        Ok(format!("Cancelled: {}.", task.said()))
    }

    /// Changes a task by its number in `said()`: any of `task`, `at`, `repeat`, `every` and
    /// `paused` in `input`, the rest kept. A resumed task skips the runs it missed.
    pub fn change(&mut self, number: usize, input: &Value, now: DateTime<Local>) -> Result<String> {
        ensure!(
            (1..=self.list.len()).contains(&number),
            "there's no task {number}; check list_tasks"
        );
        let old = &self.list[number - 1];
        let given = |key: &str| input.get(key).filter(|v| !v.is_null()).cloned();
        let merged = json!({
            "task": given("task").unwrap_or_else(|| old.task.clone().into()),
            "at": given("at").unwrap_or_else(|| old.at.format("%Y-%m-%d %H:%M").to_string().into()),
            "repeat": given("repeat").unwrap_or(serde_json::to_value(old.repeat)?),
            "every": given("every").unwrap_or_else(|| old.every.into()),
        });
        let mut task = Task::new(&old.chat, &old.bot, &merged, now)?;
        task.paused = given("paused").and_then(|v| v.as_bool()).unwrap_or(old.paused);
        let said = task.said();
        self.list[number - 1] = task;
        self.list.sort_by_key(|t| t.at);
        self.save()?;
        Ok(format!("Now: {said}."))
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

    /// Takes a task that's due at `now`. A repeating one stays, moved to its next run.
    pub fn take_due(&mut self, now: DateTime<Local>) -> Option<Task> {
        let i = self.list.iter().position(|t| !t.paused && t.at <= now)?;
        let task = self.list[i].clone();
        if task.repeat == Repeat::Never {
            self.list.remove(i);
        } else {
            self.list[i].move_past(now);
            self.list.sort_by_key(|t| t.at);
        }
        if let Err(e) = self.save() {
            eprintln!("Voxen couldn't save the tasks: {e:#}");
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
        let now = at(5, 10, 0); // a Thursday
        let task = |at: &str, repeat: &str, every: u32| {
            let input = json!({"task": "Brief me", "at": at, "repeat": repeat, "every": every});
            Task::new("main", "Maya", &input, now)
        };
        assert_eq!(task("2026-11-05 18:30", "never", 1).unwrap().at, at(5, 18, 30));
        assert_eq!(task("2026-11-05T18:30:00", "never", 1).unwrap().at, at(5, 18, 30));
        assert!(task("2026-11-05 09:00", "never", 1).unwrap_err().to_string().contains("passed"));
        assert!(task("tomorrow at 9", "never", 1).is_err());
        // A daily time that's gone today starts tomorrow; older files say "daily".
        assert_eq!(task("2026-11-05 08:00", "days", 1).unwrap().at, at(6, 8, 0));
        assert_eq!(task("2026-11-05 08:00", "daily", 1).unwrap().at, at(6, 8, 0));
        // Every 2 hours from 08:00: the next run after 10:00 is 12:00.
        assert_eq!(task("2026-11-05 08:00", "hours", 2).unwrap().at, at(5, 12, 0));
        assert!(task("2026-11-05 11:00", "minutes", 2).unwrap_err().to_string().contains("5 minutes"));
        assert_eq!(task("2026-11-05 10:30", "minutes", 30).unwrap().when(), "every 30 minutes");
        // Weekdays skip the weekend: Friday 6 Nov, then Monday 9 Nov.
        let mut weekdays = task("2026-11-06 08:00", "weekdays", 1).unwrap();
        assert_eq!(weekdays.at, at(6, 8, 0));
        weekdays.move_past(at(6, 9, 0));
        assert_eq!(weekdays.at, at(9, 8, 0));
        assert_eq!(weekdays.when(), "every weekday at 08:00");
        let monthly = task("2026-11-05 09:00", "months", 1).unwrap();
        assert_eq!(monthly.at, Local.with_ymd_and_hms(2026, 12, 5, 9, 0, 0).unwrap());
        assert_eq!(monthly.when(), "every month on the 5th at 09:00");
        assert_eq!(task("2026-11-05 09:00", "weeks", 2).unwrap().when(), "every 2 weeks on Thursday at 09:00");
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
            every: 1,
            paused: false,
        };
        tasks.add(task("Call Mum", at(5, 9, 0), Repeat::Never)).unwrap();
        tasks.add(task("News", at(5, 8, 0), Repeat::Days)).unwrap();
        let now = at(5, 9, 30);
        assert_eq!(tasks.take_due(now).unwrap().task, "News");
        assert_eq!(tasks.take_due(now).unwrap().task, "Call Mum");
        assert_eq!(tasks.take_due(now), None);
        assert_eq!(tasks.list.len(), 1);
        assert_eq!(tasks.list[0].at, at(6, 8, 0));
        assert!(tasks.said().starts_with("1. every day at 08:00 (next Fri 6 Nov 08:00), Maya: News"));
        // Asked by voice or chat: pause it, and a paused task doesn't run…
        tasks.change(1, &json!({"number": 1, "paused": true}), now).unwrap();
        assert_eq!(tasks.take_due(at(9, 9, 0)), None);
        // …resume it, and it skips the runs it missed…
        tasks.change(1, &json!({"number": 1, "paused": false}), at(9, 9, 0)).unwrap();
        assert_eq!(tasks.list[0].at, at(10, 8, 0));
        // …or make it every 3 hours from 09:30 instead.
        let input = json!({"number": 1, "repeat": "hours", "every": 3, "at": "2026-11-09 09:30"});
        let said = tasks.change(1, &input, at(9, 9, 0)).unwrap();
        assert!(said.starts_with("Now: every 3 hours"), "{said}");
        assert_eq!(tasks.list[0].at, at(9, 9, 30));
        assert!(tasks.change(5, &json!({"paused": true}), now).is_err());
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
