//! Conversations with your bots: the chat you have open (your main bot, one of your bots, or a
//! group), who answers, replies streaming in, bots taking turns in a group, and cut-ins.
//! A front-end (the terminal, the app window) sends it commands and shows its updates.

use crate::audio::{self, Mic, Speaker};
use crate::claude::{Claude, Event};
use crate::stt::{self, Heard};
use crate::team::{self, Bot, Group, Team};
use crate::tts::{self, Voices};
use crate::turn::{self, Action, Turns};
use anyhow::Result;
use futures::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
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
    AddBot(Bot),
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
    /// The bot finished.
    Done { bot: String },
    /// You cut the bot off; `after_ms` is how long it took to notice you talking.
    Cut { bot: String, after_ms: Option<u64> },
    /// Time from the end of your turn to the bot's first sound.
    FirstAudio { bot: String, ms: u64 },
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
/// Without `voices` and `speaker` it still works, as text chat.
pub fn start(
    team: Team,
    claude: Claude,
    voices: Option<Voices>,
    speaker: Option<Speaker>,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<ChatUpdate>,
) {
    let (command_tx, commands) = mpsc::unbounded_channel();
    let (updates, update_rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        team: RwLock::new(team),
        claude,
        voices,
        speaker,
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
    claude: Claude,
    voices: Option<Voices>,
    speaker: Option<Speaker>,
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
}

async fn run(shared: Arc<Shared>, mut commands: mpsc::UnboundedReceiver<Command>) {
    let mut state = State {
        open: MAIN.to_string(),
        ..State::default()
    };
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => state.handle(&shared, command),
                None => break,
            },
            result = finished(&mut state.reply) => state.finish(&shared, result),
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
            Command::AddBot(bot) => shared.change_team(&self.open, |team| team.add_bot(bot)),
            Command::AddGroup(group) => {
                shared.change_team(&self.open, |team| team.add_group(group))
            }
            Command::Problem(text) => shared.send(&self.open, Update::Problem { text }),
        }
    }

    /// Who's in the open chat.
    fn members(&self, shared: &Shared) -> Vec<String> {
        let team = shared.team();
        if let Some(name) = self.open.strip_prefix("bot:")
            && let Some(bot) = team.bot(name)
        {
            return vec![bot.name.clone()];
        }
        if let Some(name) = self.open.strip_prefix("group:")
            && let Some(group) = team.group(name)
        {
            return group.bots.clone();
        }
        vec![team.main_bot().name.clone()]
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
        let members = self.members(shared);
        let entry = self.chats.entry(chat).or_default();
        let first = shared
            .team()
            .pick(&said, entry.last_bot.as_deref(), &members)
            .name
            .clone();
        entry.history.push(("You".into(), said));
        self.queue = after(&members, &first);
        self.start(shared, first, started);
    }

    fn start(&mut self, shared: &Arc<Shared>, name: String, started: Instant) {
        let chat = self.open.clone();
        let members = self.members(shared);
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
            let prompt = team.instructions(&bot, &others, self.speak);
            (bot, prompt)
        };
        let entry = self.chats.entry(chat.clone()).or_default();
        entry.last_bot = Some(bot.name.clone());
        let job = Job {
            chat: chat.clone(),
            transcript: team::transcript(&entry.history),
            prompt,
            heard: Arc::new(Mutex::new(String::new())),
            started,
            speak: self.speak,
            bot: bot.clone(),
        };
        shared.send(
            &chat,
            Update::Start {
                bot: bot.name.clone(),
            },
        );
        let heard = job.heard.clone();
        let task = tokio::spawn(reply(shared.clone(), job));
        self.reply = Some(Reply {
            chat,
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
            self.start(shared, next, Instant::now());
        }
    }
}

/// Everything one reply needs.
struct Job {
    chat: String,
    bot: Bot,
    prompt: String,
    transcript: String,
    heard: Arc<Mutex<String>>,
    started: Instant,
    speak: bool,
}

/// Streams a bot's reply from Claude, showing it as it arrives and, when `speak` is on,
/// saying it sentence by sentence.
async fn reply(shared: Arc<Shared>, job: Job) -> Result<()> {
    let events = shared
        .claude
        .stream(&job.prompt, &job.transcript, &[])
        .await?;
    let mut events = std::pin::pin!(events);
    let mut mouth = job
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
    let mut pending = String::new();
    while let Some(event) = events.next().await {
        match event? {
            Event::Text(text) => {
                let update = Update::Text {
                    bot: job.bot.name.clone(),
                    text: text.clone(),
                };
                shared.send(&job.chat, update);
                match &mut mouth {
                    Some(mouth) => {
                        pending.push_str(&text);
                        while let Some(sentence) = turn::take_sentence(&mut pending) {
                            mouth.say(&sentence).await?;
                            keep(&job.heard, &format!("{sentence} "));
                        }
                    }
                    None => keep(&job.heard, &text),
                }
            }
            Event::Done { stop_reason } if stop_reason == "refusal" => {
                let text = format!("{} declined to answer that", job.bot.name);
                shared.send(&job.chat, Update::Problem { text });
            }
            _ => {}
        }
    }
    if let Some(mouth) = &mut mouth {
        let rest = pending.trim();
        if !rest.is_empty() {
            mouth.say(rest).await?;
            keep(&job.heard, rest);
        }
    }
    Ok(())
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
        // Nothing listens on port 9, so every Claude request fails at once, offline.
        let claude = Claude::new("test", "http://127.0.0.1:9", "claude-opus-5-5");
        start(Team::parse(TEAM).unwrap(), claude, None, None)
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
}
