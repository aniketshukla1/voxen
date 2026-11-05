//! Conversations with your bots: the chat you have open (your main bot, one of your bots, or a
//! group), who answers, replies streaming in, bots taking turns in a group, cut-ins, the
//! bots' tools, and reminders that fire on their own.
//! A front-end (the terminal, the app window) sends it commands and shows its updates.

use crate::audio::{self, Mic, Speaker};
use crate::claude::{Event, Source};
use crate::llm::{self, Brains, ToolResult};
use crate::stt::{self, Heard};
use crate::team::{self, Bot, Group, Team};
use crate::tools::{self, Attachment, Memory, Task, Tasks};
use crate::tts::{self, Voices};
use crate::turn::{self, Action, Turns};
use crate::xai::Xai;
use anyhow::{Context, Result, bail, ensure};
use chrono::Local;
use futures::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle};

/// The chat with your main bot. The others are "bot:<name>" and "group:<name>".
pub const MAIN: &str = "main";

/// What a front-end tells the conversations.
#[derive(Debug)]
pub enum Command {
    /// Switch to another chat.
    Open(String),
    /// You typed this in the open chat.
    Typed(String),
    /// The listener heard something; `now` is the current stream time in seconds.
    Heard {
        heard: Heard,
        now: f64,
    },
    /// Stop whoever is talking.
    Stop,
    /// Say replies out loud (voice on) or only show them.
    Speak(bool),
    /// Forget the open chat's conversation.
    Clear,
    /// Files for the bots to look at with your next turn, typed or spoken. Replaces any
    /// attached before.
    Attach(Vec<Attachment>),
    AddBot(Bot),
    /// Change a bot, found by name: its brain, voice, colour or what it's for.
    UpdateBot(Bot),
    AddGroup(Group),
    /// Something outside the conversations went wrong, to pass on as an update.
    Problem(String),
}

/// One update and the chat it belongs to ("" when it's about the whole team).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatUpdate {
    pub chat: String,
    #[serde(flatten)]
    pub update: Update,
}

/// What happened, for the front-end to show.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Update {
    /// Your turn, as it goes into the conversation.
    You { text: String },
    /// Words you're saying right now, before your turn ends.
    Hearing { text: String },
    /// A bot started replying.
    Start { bot: String },
    /// More of the bot's reply.
    Text { bot: String, text: String },
    /// What the bot is doing with a tool, like "Searching the web for …".
    Step { bot: String, text: String },
    /// The pages behind the bot's reply.
    Sources { bot: String, sources: Vec<Source> },
    /// A picture the bot made.
    Image {
        bot: String,
        url: String,
        prompt: String,
    },
    /// The bot finished.
    Done { bot: String },
    /// You cut the bot off; `after_ms` is how long it took to notice you talking.
    Cut { bot: String, after_ms: Option<u64> },
    /// Time from the end of your turn to the bot's first sound.
    FirstAudio { bot: String, ms: u64 },
    /// A reminder or repeating task is due; the bot's reply follows.
    Task { bot: String, task: String },
    /// Your bots and group chats, after a change.
    Team {
        main: String,
        bots: Vec<Bot>,
        groups: Vec<Group>,
    },
    /// Something went wrong; the conversation carries on.
    Problem { text: String },
}

/// Starts the conversations on the current Tokio runtime. Send commands in, read updates out.
/// `brains` makes each bot's model client from its provider and model. Without `voices` and
/// `speaker` it still works, as text chat; without `xai` the bots can't search X or make
/// pictures.
pub fn start(
    team: Team,
    brains: Brains,
    voices: Option<Voices>,
    speaker: Option<Speaker>,
    xai: Option<Xai>,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<ChatUpdate>,
) {
    let (command_tx, commands) = mpsc::unbounded_channel();
    let (updates, update_rx) = mpsc::unbounded_channel();
    // ponytail: data sits next to the team file; move it to Application Support when the app ships.
    let data = team.dir().map(|dir| dir.join("data"));
    let file = |name: &str| data.as_ref().map(|dir| dir.join(name));
    let shared = Arc::new(Shared {
        memory: Mutex::new(Memory::load(file("memory.md"))),
        tasks: Mutex::new(Tasks::load(file("tasks.json"))),
        pictures: file("pictures"),
        timezone: tools::timezone(),
        team: RwLock::new(team),
        brains,
        voices,
        speaker,
        xai,
        updates,
    });
    tokio::spawn(run(shared, commands));
    (command_tx, update_rx)
}

/// Opens the microphone and streams it to Deepgram, sending what it hears to the
/// conversations, until the returned `Mic` is dropped.
pub fn listen(
    key: String,
    names: Vec<String>,
    commands: mpsc::UnboundedSender<Command>,
) -> Result<Mic> {
    let (audio_tx, mut audio_rx) = mpsc::unbounded_channel();
    let mic = audio::mic(audio_tx)?;
    let rate = mic.rate;
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel();
    let sent = Arc::new(AtomicU64::new(0));
    let (counter, reports) = (sent.clone(), commands.clone());
    tokio::spawn(async move {
        let mut failures = 0;
        loop {
            let began = Instant::now();
            let Err(e) = stt::listen(&key, rate, &names, &mut audio_rx, &heard_tx, &counter).await
            else {
                break; // the mic closed
            };
            failures = if began.elapsed() > Duration::from_secs(10) {
                1
            } else {
                failures + 1
            };
            if failures >= 3 {
                reports
                    .send(Command::Problem(format!("Voice stopped: {e:#}")))
                    .ok();
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    tokio::spawn(async move {
        while let Some(heard) = heard_rx.recv().await {
            let now = sent.load(Ordering::Relaxed) as f64 / f64::from(rate);
            if commands.send(Command::Heard { heard, now }).is_err() {
                break;
            }
        }
    });
    Ok(mic)
}

/// Everything the replies need, shared with their tasks.
struct Shared {
    team: RwLock<Team>,
    brains: Brains,
    voices: Option<Voices>,
    speaker: Option<Speaker>,
    xai: Option<Xai>,
    memory: Mutex<Memory>,
    tasks: Mutex<Tasks>,
    /// Where pictures the bots make are saved.
    pictures: Option<PathBuf>,
    /// For local search results.
    timezone: Option<String>,
    updates: mpsc::UnboundedSender<ChatUpdate>,
}

impl Shared {
    fn send(&self, chat: &str, update: Update) {
        self.updates
            .send(ChatUpdate {
                chat: chat.to_string(),
                update,
            })
            .ok();
    }

    fn team(&self) -> RwLockReadGuard<'_, Team> {
        self.team.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Applies a change to the team and tells the front-end, or reports why it failed.
    fn change_team(&self, chat: &str, change: impl FnOnce(&mut Team) -> Result<()>) {
        let mut team = self.team.write().unwrap_or_else(|e| e.into_inner());
        match change(&mut team) {
            Ok(()) => {
                let update = Update::Team {
                    main: team.main.clone(),
                    bots: team.bots.clone(),
                    groups: team.groups.clone(),
                };
                drop(team);
                self.send("", update);
            }
            Err(e) => {
                drop(team);
                self.send(
                    chat,
                    Update::Problem {
                        text: format!("{e:#}"),
                    },
                );
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A bot's reply in progress.
struct Reply {
    chat: String,
    bot: String,
    /// What you've seen or heard of the reply so far, kept if you cut it off.
    heard: Arc<Mutex<String>>,
    task: JoinHandle<Result<()>>,
}

/// One chat's conversation so far.
#[derive(Default)]
struct Chat {
    history: Vec<(String, String)>,
    last_bot: Option<String>,
    /// Files you attached this round, as message content.
    files: Vec<Value>,
}

#[derive(Default)]
struct State {
    open: String,
    chats: HashMap<String, Chat>,
    turns: Turns,
    reply: Option<Reply>,
    /// Bots still to speak this round in a group chat.
    queue: Vec<String>,
    last_word_end: f64,
    speak: bool,
    /// Files waiting to go with your next turn.
    attached: Vec<Attachment>,
}

async fn run(shared: Arc<Shared>, mut commands: mpsc::UnboundedReceiver<Command>) {
    let mut state = State {
        open: MAIN.to_string(),
        ..State::default()
    };
    let mut clock = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => state.handle(&shared, command),
                None => break,
            },
            result = finished(&mut state.reply) => state.finish(&shared, result),
            _ = clock.tick() => state.tick(&shared),
        }
    }
    state.cut(&shared, None);
}

/// Waits for the current reply to finish; never resolves when there isn't one.
async fn finished(reply: &mut Option<Reply>) -> Result<Result<()>, JoinError> {
    match reply {
        Some(r) => (&mut r.task).await,
        None => std::future::pending().await,
    }
}

/// The other members, in turn after `first`, who answer next in a group chat.
fn after(members: &[String], first: &str) -> Vec<String> {
    let at = members
        .iter()
        .position(|m| m.eq_ignore_ascii_case(first))
        .unwrap_or(0);
    members
        .iter()
        .cycle()
        .skip(at + 1)
        .take(members.len().saturating_sub(1))
        .cloned()
        .collect()
}

/// Who's in a chat.
fn members(shared: &Shared, chat: &str) -> Vec<String> {
    let team = shared.team();
    if let Some(name) = chat.strip_prefix("bot:")
        && let Some(bot) = team.bot(name)
    {
        return vec![bot.name.clone()];
    }
    if let Some(name) = chat.strip_prefix("group:")
        && let Some(group) = team.group(name)
    {
        return group.bots.clone();
    }
    vec![team.main_bot().name.clone()]
}

impl State {
    fn handle(&mut self, shared: &Arc<Shared>, command: Command) {
        match command {
            Command::Open(chat) => {
                if chat != self.open {
                    self.cut(shared, None);
                    self.open = chat;
                    self.turns = Turns::default();
                }
            }
            Command::Typed(text) => {
                let text = text.trim();
                if !text.is_empty() {
                    self.your_turn(shared, text.to_string(), Instant::now());
                }
            }
            Command::Heard { heard, now } => self.heard(shared, heard, now),
            Command::Stop => self.cut(shared, None),
            Command::Speak(on) => {
                self.speak = on;
                if !on {
                    self.cut(shared, None);
                }
            }
            Command::Clear => {
                self.cut(shared, None);
                self.chats.remove(&self.open);
            }
            Command::Attach(files) => self.attached = files,
            Command::AddBot(bot) => shared.change_team(&self.open, |team| team.add_bot(bot)),
            Command::UpdateBot(bot) => shared.change_team(&self.open, |team| team.update_bot(bot)),
            Command::AddGroup(group) => {
                shared.change_team(&self.open, |team| team.add_group(group))
            }
            Command::Problem(text) => shared.send(&self.open, Update::Problem { text }),
        }
    }

    fn talking(&self, shared: &Shared) -> bool {
        self.reply.is_some() || shared.speaker.as_ref().is_some_and(|s| !s.idle())
    }

    fn heard(&mut self, shared: &Arc<Shared>, heard: Heard, now: f64) {
        let talking = self.talking(shared);
        let mut cut_in_ms = None;
        match &heard {
            Heard::SpeechStarted { at } if talking => {
                cut_in_ms = Some(((now - at) * 1000.0).max(0.0) as u64);
            }
            Heard::Partial(text) => shared.send(&self.open, Update::Hearing { text: text.clone() }),
            Heard::Final { end, text, .. } if !text.is_empty() => self.last_word_end = *end,
            _ => {}
        }
        match self.turns.on_heard(heard, talking) {
            Action::Interrupt => self.cut(shared, cut_in_ms),
            Action::Reply(said) => {
                // Time from your last word, not from when the pause was detected.
                let waited = Duration::from_secs_f64((now - self.last_word_end).max(0.0));
                let started = Instant::now()
                    .checked_sub(waited)
                    .unwrap_or_else(Instant::now);
                self.your_turn(shared, said, started);
            }
            Action::Wait => {}
        }
    }

    /// Your turn ended: note what you said and start the first bot's reply. In a group, the
    /// others answer after it, one at a time.
    fn your_turn(&mut self, shared: &Arc<Shared>, said: String, started: Instant) {
        self.cut(shared, None);
        let chat = self.open.clone();
        shared.send(&chat, Update::You { text: said.clone() });
        let files = std::mem::take(&mut self.attached);
        let mut line = said.clone();
        if !files.is_empty() {
            let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
            line.push_str(&format!(" [attached: {}]", names.join(", ")));
        }
        let members = members(shared, &chat);
        let entry = self.chats.entry(chat.clone()).or_default();
        let first = shared
            .team()
            .pick(&said, entry.last_bot.as_deref(), &members)
            .name
            .clone();
        entry.history.push(("You".into(), line));
        entry.files = files.iter().map(Attachment::block).collect();
        self.queue = after(&members, &first);
        self.start(shared, &chat, first, started, self.speak);
    }

    /// Starts a task that's due, when nobody is talking.
    fn tick(&mut self, shared: &Arc<Shared>) {
        if self.talking(shared) || !self.queue.is_empty() {
            return;
        }
        let Some(task) = lock(&shared.tasks).take_due(Local::now()) else {
            return;
        };
        shared.send(
            &task.chat,
            Update::Task {
                bot: task.bot.clone(),
                task: task.task.clone(),
            },
        );
        let entry = self.chats.entry(task.chat.clone()).or_default();
        entry.history.push(("Scheduled task".into(), task.task));
        entry.files.clear();
        // A task's answer is said out loud even with the mic off.
        self.start(shared, &task.chat, task.bot, Instant::now(), true);
    }

    fn start(
        &mut self,
        shared: &Arc<Shared>,
        chat: &str,
        name: String,
        started: Instant,
        speak: bool,
    ) {
        let members = members(shared, chat);
        let (bot, prompt) = {
            let team = shared.team();
            let Some(bot) = team.bot(&name).cloned() else {
                return;
            };
            let others: Vec<&Bot> = members
                .iter()
                .filter(|m| !m.eq_ignore_ascii_case(&bot.name))
                .filter_map(|m| team.bot(m))
                .collect();
            let prompt = team.instructions(&bot, &others, speak);
            (bot, prompt)
        };
        let facts = lock(&shared.memory).facts.clone();
        let brain = llm::find(&bot.provider);
        let can = tools::Abilities {
            web: brain.is_some_and(|p| p.web),
            tools: brain.is_some_and(|p| p.tools),
            xai: shared.xai.is_some(),
        };
        let prompt = prompt + &tools::guidance(&facts, Local::now(), can);
        let entry = self.chats.entry(chat.to_string()).or_default();
        entry.last_bot = Some(bot.name.clone());
        let mut content = entry.files.clone();
        content.push(json!({"type": "text", "text": team::transcript(&entry.history)}));
        let job = Job {
            chat: chat.to_string(),
            content,
            prompt,
            heard: Arc::new(Mutex::new(String::new())),
            started,
            speak,
            bot: bot.clone(),
        };
        shared.send(
            chat,
            Update::Start {
                bot: bot.name.clone(),
            },
        );
        let heard = job.heard.clone();
        let task = tokio::spawn(reply(shared.clone(), job));
        self.reply = Some(Reply {
            chat: chat.to_string(),
            bot: bot.name,
            heard,
            task,
        });
    }

    /// Go quiet and stop the reply (and the rest of the group's round), keeping what you got.
    fn cut(&mut self, shared: &Shared, after_ms: Option<u64>) {
        self.queue.clear();
        if let Some(speaker) = &shared.speaker {
            speaker.stop();
        }
        if let Some(cut) = self.reply.take() {
            cut.task.abort();
            let heard = cut
                .heard
                .lock()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if !heard.is_empty() {
                let chat = self.chats.entry(cut.chat.clone()).or_default();
                chat.history
                    .push((cut.bot.clone(), format!("{heard} [cut off]")));
            }
            shared.send(
                &cut.chat,
                Update::Cut {
                    bot: cut.bot,
                    after_ms,
                },
            );
        }
    }

    fn finish(&mut self, shared: &Arc<Shared>, result: Result<Result<()>, JoinError>) {
        let Some(done) = self.reply.take() else {
            return;
        };
        let said = done
            .heard
            .lock()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if !said.is_empty() {
            self.chats
                .entry(done.chat.clone())
                .or_default()
                .history
                .push((done.bot.clone(), said));
        }
        shared.send(
            &done.chat,
            Update::Done {
                bot: done.bot.clone(),
            },
        );
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.queue.clear();
                let text = format!("{} couldn't answer: {e:#}", done.bot);
                shared.send(&done.chat, Update::Problem { text });
            }
            Err(e) if e.is_cancelled() => {}
            Err(e) => shared.send(
                &done.chat,
                Update::Problem {
                    text: format!("{} stopped: {e}", done.bot),
                },
            ),
        }
        if done.chat == self.open && !self.queue.is_empty() {
            let next = self.queue.remove(0);
            self.start(shared, &done.chat, next, Instant::now(), self.speak);
        }
    }
}

/// Everything one reply needs.
struct Job {
    chat: String,
    bot: Bot,
    prompt: String,
    /// The user message: files attached this round, then the conversation so far.
    content: Vec<Value>,
    heard: Arc<Mutex<String>>,
    started: Instant,
    speak: bool,
}

/// Streams a bot's reply from Claude, showing it as it arrives and, when `speak` is on,
/// saying it sentence by sentence. Runs the bot's tools until it has its answer.
async fn reply(shared: Arc<Shared>, job: Job) -> Result<()> {
    let llm = shared.brains.llm(&job.bot.provider, &job.bot.model)?;
    let functions = tools::functions(shared.xai.is_some(), llm.provider.id == "xai");
    let mut convo = llm.start(&job.content);
    let mouth = job
        .speak
        .then(|| shared.voices.as_ref().zip(shared.speaker.as_ref()))
        .flatten()
        .map(|(voices, speaker)| Mouth {
            shared: &shared,
            chat: &job.chat,
            voices,
            speaker,
            bot: &job.bot,
            started: job.started,
            previous: String::new(),
            first: true,
        });
    let mut out = Out {
        shared: &shared,
        job: &job,
        mouth,
        pending: String::new(),
        said: false,
    };
    let (mut cited, mut found) = (Vec::new(), Vec::new());
    // ponytail: at most 8 rounds of tools per reply; raise it if deep research needs more.
    for _ in 0..8 {
        let events = llm
            .stream(&job.prompt, &convo, &functions, tools::EFFORT, shared.timezone.as_deref())
            .await?;
        let mut events = std::pin::pin!(events);
        let (mut calls, mut end) = (Vec::new(), None);
        while let Some(event) = events.next().await {
            match event? {
                Event::Text(text) => out.text(&text).await?,
                Event::Using { tool, input } => out.step(&tool, &input).await?,
                Event::Found(pages) => add_new(&mut found, pages),
                Event::Cited(page) => add_new(&mut cited, vec![page]),
                Event::Tool { id, name, input } => {
                    out.step(&name, &input).await?;
                    calls.push((id, name, input));
                }
                Event::Done {
                    stop_reason,
                    content,
                } => end = Some((stop_reason, content)),
            }
        }
        let Some((stop_reason, content)) = end else {
            break;
        };
        match stop_reason.as_str() {
            "tool_use" => {
                let mut results = Vec::new();
                for (id, name, input) in calls {
                    let (output, failed) = match use_tool(&shared, &job, &name, &input, &mut cited).await {
                        Ok(text) => (text, false),
                        Err(e) => (format!("{e:#}"), true),
                    };
                    results.push(ToolResult { id, name, output, failed });
                }
                llm.carry_on(&mut convo, content, results);
            }
            // A long run of searches paused on the server: send it back as it is to carry on.
            "pause_turn" => llm.carry_on(&mut convo, content, Vec::new()),
            "refusal" => {
                let text = format!("{} declined to answer that", job.bot.name);
                shared.send(&job.chat, Update::Problem { text });
                break;
            }
            _ => break,
        }
    }
    out.finish().await?;
    let sources = if cited.is_empty() {
        found.into_iter().take(4).collect()
    } else {
        cited
    };
    if !sources.is_empty() {
        let bot = job.bot.name.clone();
        shared.send(&job.chat, Update::Sources { bot, sources });
    }
    Ok(())
}

fn add_new(list: &mut Vec<Source>, pages: Vec<Source>) {
    for page in pages {
        if !list.iter().any(|p| p.url == page.url) {
            list.push(page);
        }
    }
}

/// Runs one of Voxen's own tools for a bot and returns what to tell it.
async fn use_tool(
    shared: &Shared,
    job: &Job,
    name: &str,
    input: &Value,
    cited: &mut Vec<Source>,
) -> Result<String> {
    ensure!(
        !input.is_null(),
        "The tool input wasn't valid JSON. Try the call again."
    );
    let text = |key: &str| input[key].as_str().unwrap_or_default().trim().to_string();
    let xai = || {
        shared
            .xai
            .as_ref()
            .context("X search and pictures need an xAI key; the user can add one in Setup.")
    };
    match name {
        "search_x" => {
            let (summary, posts) = xai()?.search_x(&text("query")).await?;
            let links: Vec<&str> = posts.iter().map(|p| p.url.as_str()).collect();
            let reply = format!("{summary}\n\nPosts:\n{}", links.join("\n"));
            add_new(cited, posts);
            Ok(reply)
        }
        "create_image" => {
            let prompt = text("prompt");
            ensure!(!prompt.is_empty(), "describe the picture");
            let (url, bytes) = xai()?.image(&prompt).await?;
            let saved = save_picture(shared, &bytes);
            let bot = job.bot.name.clone();
            shared.send(&job.chat, Update::Image { bot, url, prompt });
            Ok(match saved {
                Some(path) => format!("The picture is in the chat now, saved at {}.", path.display()),
                None => "The picture is in the chat now.".into(),
            })
        }
        _ => local_tool(shared, job, name, input),
    }
}

/// The tools that run on this Mac: memory and tasks.
fn local_tool(shared: &Shared, job: &Job, name: &str, input: &Value) -> Result<String> {
    let text = |key: &str| input[key].as_str().unwrap_or_default().to_string();
    match name {
        "remember" => lock(&shared.memory).remember(&text("fact")),
        "forget" => lock(&shared.memory).forget(&text("words")),
        "schedule" => {
            let task = Task::new(&job.chat, &job.bot.name, input, Local::now())?;
            lock(&shared.tasks).add(task)
        }
        "list_tasks" => Ok(lock(&shared.tasks).said()),
        "change_task" => {
            let number = input["number"].as_u64().unwrap_or(0) as usize;
            lock(&shared.tasks).change(number, input, Local::now())
        }
        "cancel_task" => {
            let number = input["number"].as_u64().unwrap_or(0) as usize;
            lock(&shared.tasks).cancel(number)
        }
        other => bail!("There's no tool called {other}."),
    }
}

/// Saves a picture a bot made, named by the time. Returns where, if it could.
fn save_picture(shared: &Shared, bytes: &[u8]) -> Option<PathBuf> {
    let dir = shared.pictures.as_ref()?;
    let kind = if bytes.starts_with(b"\x89PNG") {
        "png"
    } else if bytes.starts_with(b"RIFF") {
        "webp"
    } else {
        "jpg"
    };
    let path = dir.join(format!("{}.{kind}", Local::now().format("%Y-%m-%d at %H.%M.%S")));
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// Where a reply goes: on screen, and out loud sentence by sentence on a call.
struct Out<'a> {
    shared: &'a Shared,
    job: &'a Job,
    mouth: Option<Mouth<'a>>,
    pending: String,
    /// Whether anything was said or shown yet.
    said: bool,
}

impl Out<'_> {
    async fn text(&mut self, text: &str) -> Result<()> {
        self.said = true;
        let update = Update::Text {
            bot: self.job.bot.name.clone(),
            text: text.to_string(),
        };
        self.shared.send(&self.job.chat, update);
        match &mut self.mouth {
            Some(mouth) => {
                self.pending.push_str(text);
                while let Some(sentence) = turn::take_sentence(&mut self.pending) {
                    mouth.say(&sentence).await?;
                    keep(&self.job.heard, &format!("{sentence} "));
                }
            }
            None => keep(&self.job.heard, text),
        }
        Ok(())
    }

    /// Shows what the bot does with a tool. On a call, a slow tool gets a few words first,
    /// so the line never goes quiet.
    async fn step(&mut self, tool: &str, input: &Value) -> Result<()> {
        let update = Update::Step {
            bot: self.job.bot.name.clone(),
            text: tools::step(tool, input),
        };
        self.shared.send(&self.job.chat, update);
        if !self.said
            && self.mouth.is_some()
            && let Some(line) = tools::filler(tool)
        {
            self.text(&format!("{line} ")).await?;
        }
        Ok(())
    }

    /// Says whatever is left of the reply.
    async fn finish(&mut self) -> Result<()> {
        let rest = std::mem::take(&mut self.pending);
        let rest = rest.trim();
        if let Some(mouth) = &mut self.mouth
            && !rest.is_empty()
        {
            mouth.say(rest).await?;
            keep(&self.job.heard, rest);
        }
        Ok(())
    }
}

fn keep(heard: &Mutex<String>, text: &str) {
    if let Ok(mut heard) = heard.lock() {
        heard.push_str(text);
    }
}

/// Says one bot's sentences out loud.
struct Mouth<'a> {
    shared: &'a Shared,
    chat: &'a str,
    voices: &'a Voices,
    speaker: &'a Speaker,
    bot: &'a Bot,
    started: Instant,
    previous: String,
    first: bool,
}

impl Mouth<'_> {
    async fn say(&mut self, sentence: &str) -> Result<()> {
        let (shared, chat, speaker, bot, started) =
            (self.shared, self.chat, self.speaker, self.bot, self.started);
        let first = &mut self.first;
        self.voices
            .speak(&bot.voice, bot.speed, sentence, &self.previous, |samples| {
                if std::mem::take(first) {
                    let ms = started.elapsed().as_millis() as u64;
                    shared.send(
                        chat,
                        Update::FirstAudio {
                            bot: bot.name.clone(),
                            ms,
                        },
                    );
                }
                speaker.play(samples, tts::RATE);
            })
            .await?;
        self.previous = sentence.to_string();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = r#"
main = "Maya"
[[bot]]
name = "Maya"
role = "Helper"
personality = "Warm"
voice = "v1"
[[bot]]
name = "Sam"
role = "Researcher"
personality = "Calm"
voice = "v2"
[[group]]
name = "Pair"
bots = ["Maya", "Sam"]
"#;

    fn offline() -> (
        mpsc::UnboundedSender<Command>,
        mpsc::UnboundedReceiver<ChatUpdate>,
    ) {
        // Nothing listens on port 9, so every request fails at once, offline.
        start(Team::parse(TEAM).unwrap(), Brains::offline(), None, None, None)
    }

    async fn next(updates: &mut mpsc::UnboundedReceiver<ChatUpdate>, n: usize) -> Vec<ChatUpdate> {
        let mut got = Vec::new();
        while got.len() < n {
            got.push(updates.recv().await.unwrap());
        }
        got
    }

    #[tokio::test]
    async fn the_main_chat_answers_with_the_main_bot_and_reports_failures() {
        let (commands, mut updates) = offline();
        commands.send(Command::Typed("  Sam, hi ".into())).unwrap();
        let got: Vec<Update> = next(&mut updates, 4)
            .await
            .into_iter()
            .map(|u| u.update)
            .collect();
        assert_eq!(
            got[0],
            Update::You {
                text: "Sam, hi".into()
            }
        );
        assert_eq!(got[1], Update::Start { bot: "Maya".into() }); // Sam isn't in the main chat
        assert_eq!(got[2], Update::Done { bot: "Maya".into() });
        assert!(
            matches!(&got[3], Update::Problem { text } if text.starts_with("Maya couldn't answer"))
        );
    }

    #[tokio::test]
    async fn group_chats_and_new_bots() {
        let (commands, mut updates) = offline();
        commands.send(Command::Open("group:Pair".into())).unwrap();
        commands
            .send(Command::Typed("Sam, go first".into()))
            .unwrap();
        let got = next(&mut updates, 2).await;
        assert_eq!(
            got[1],
            ChatUpdate {
                chat: "group:Pair".into(),
                update: Update::Start { bot: "Sam".into() }
            }
        );
        let _ = next(&mut updates, 2).await; // Sam's failed reply ends the round

        let ava = Bot {
            name: "Ava".into(),
            role: "Planner".into(),
            personality: "Upbeat".into(),
            voice: "v3".into(),
            speed: 1.0,
            color: None,
            provider: "openai".into(),
            model: String::new(),
        };
        commands.send(Command::AddBot(ava)).unwrap();
        let got = next(&mut updates, 1).await;
        assert!(matches!(&got[0].update, Update::Team { bots, .. } if bots.len() == 3));
    }

    #[test]
    fn group_members_answer_in_turn() {
        let members: Vec<String> = ["Maya", "Sam", "Leo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(after(&members, "sam"), vec!["Leo", "Maya"]);
        assert!(after(&members[..1], "Maya").is_empty());
    }

    #[test]
    fn updates_reach_the_window_as_flat_json() {
        let update = ChatUpdate {
            chat: "main".into(),
            update: Update::Cut {
                bot: "Sam".into(),
                after_ms: Some(280),
            },
        };
        assert_eq!(
            serde_json::to_value(update).unwrap(),
            serde_json::json!({"chat": "main", "kind": "cut", "bot": "Sam", "after_ms": 280})
        );
    }

    #[tokio::test]
    async fn local_tools_remember_and_schedule() {
        let (updates, _rx) = mpsc::unbounded_channel();
        let shared = Shared {
            team: RwLock::new(Team::parse(TEAM).unwrap()),
            brains: Brains::offline(),
            voices: None,
            speaker: None,
            xai: None,
            memory: Mutex::default(),
            tasks: Mutex::default(),
            pictures: None,
            timezone: None,
            updates,
        };
        let job = Job {
            chat: "main".into(),
            bot: shared.team().main_bot().clone(),
            prompt: String::new(),
            content: Vec::new(),
            heard: Arc::default(),
            started: Instant::now(),
            speak: false,
        };
        let mut cited = Vec::new();
        let run = |name: &str, input: Value| {
            let (shared, job) = (&shared, &job);
            let name = name.to_string();
            async move { use_tool(shared, job, &name, &input, &mut Vec::new()).await }
        };
        run("remember", json!({"fact": "Likes tea"})).await.unwrap();
        assert_eq!(lock(&shared.memory).facts, ["Likes tea"]);
        let soon = (Local::now() + chrono::Duration::hours(1)).format("%Y-%m-%d %H:%M");
        let set = run("schedule", json!({"task": "Stretch", "at": soon.to_string(), "repeat": "never"}))
            .await
            .unwrap();
        assert!(set.contains("Maya: Stretch"), "{set}");
        assert!(run("search_x", json!({"query": "rust"})).await.unwrap_err().to_string().contains("xAI key"));
        assert!(use_tool(&shared, &job, "remember", &Value::Null, &mut cited).await.is_err());
    }
    /// Runs one typed turn in the main chat against the real API and returns its updates.
    async fn live_turn(said: &str) -> Vec<Update> {
        dotenvy::dotenv().ok();
        let brains = Brains::new();
        assert!(brains.ready("anthropic"), "put ANTHROPIC_API_KEY in .env (Setup in the app does it)");
        let (commands, mut updates) = start(Team::parse(TEAM).unwrap(), brains, None, None, None);
        commands.send(Command::Typed(said.into())).unwrap();
        let mut got = Vec::new();
        // Done comes before any problem with the reply, so wait a moment past it.
        let mut wait = Duration::from_secs(180);
        while let Ok(Some(update)) = tokio::time::timeout(wait, updates.recv()).await {
            if matches!(update.update, Update::Done { .. }) {
                wait = Duration::from_secs(1);
            }
            got.push(update.update);
        }
        got
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs ANTHROPIC_API_KEY"]
    async fn live_reply_searches_the_web_and_shows_sources() {
        let got = live_turn("What's the biggest news story today? One sentence.").await;
        let ok = |test: fn(&Update) -> bool| got.iter().any(test);
        assert!(!ok(|u| matches!(u, Update::Problem { .. })), "{got:#?}");
        assert!(ok(|u| matches!(u, Update::Step { text, .. } if text.starts_with("Searching the web"))), "{got:#?}");
        assert!(ok(|u| matches!(u, Update::Text { .. })), "{got:#?}");
        assert!(ok(|u| matches!(u, Update::Sources { sources, .. } if !sources.is_empty())), "{got:#?}");
    }

    #[tokio::test]
    #[ignore = "calls the real API; needs ANTHROPIC_API_KEY"]
    async fn live_reply_runs_our_own_tools() {
        let got = live_turn("Please remember that my favourite colour is teal.").await;
        assert!(!got.iter().any(|u| matches!(u, Update::Problem { .. })), "{got:#?}");
        assert!(
            got.iter().any(|u| matches!(u, Update::Step { text, .. } if text.starts_with("Remembering"))),
            "{got:#?}"
        );
        // The reply carries on after the tool runs.
        assert!(got.iter().any(|u| matches!(u, Update::Text { .. })), "{got:#?}");
    }
}
