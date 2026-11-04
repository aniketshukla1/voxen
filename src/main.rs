//! A voice call with your team: talk (or type), and the right bot answers out loud.

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle};
use voice_bots::audio::{self, Speaker};
use voice_bots::claude::{Claude, Event};
use voice_bots::stt::{self, Heard};
use voice_bots::team::{self, Bot, Team};
use voice_bots::tts::{self, Voices};
use voice_bots::turn::{self, Action, Turns};

/// Hand-offs allowed in a row, so two bots can't pass you back and forth forever.
const MAX_HANDOFFS: usize = 2;

/// Everything a reply needs, shared by all reply tasks.
struct Call {
    team: Team,
    claude: Claude,
    voices: Voices,
    speaker: Speaker,
}

/// A bot's reply in progress.
struct Reply {
    bot: String,
    handoffs: usize,
    /// Sentences already sent to the speaker, so a cut-off reply keeps what you heard.
    spoken: Arc<Mutex<String>>,
    task: JoinHandle<Result<Option<String>>>,
}

type History = Vec<(String, String)>;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let deepgram_key = std::env::var("DEEPGRAM_API_KEY")
        .context("DEEPGRAM_API_KEY is not set (copy .env.example to .env)")?;
    let team = Team::load("team.toml")?;
    let claude = Claude::from_env()?;
    let voices = Voices::from_env()?;
    let (_speaker_stream, speaker) = audio::speaker()?;
    let call = Arc::new(Call {
        team,
        claude,
        voices,
        speaker,
    });
    let names: Vec<String> = call.team.bots.iter().map(|b| b.name.clone()).collect();

    let (mic_tx, mut mic_rx) = mpsc::unbounded_channel();
    let (_mic_stream, mic_rate) = audio::mic(mic_tx)?;
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel();
    let sent = Arc::new(AtomicU64::new(0));
    {
        let (sent, names) = (sent.clone(), names.clone());
        tokio::spawn(async move {
            while let Err(e) = stt::listen(
                &deepgram_key,
                mic_rate,
                &names,
                &mut mic_rx,
                &heard_tx,
                &sent,
            )
            .await
            {
                eprintln!("(listening stopped: {e:#}; reconnecting)");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    println!(
        "On the call with {}. Talk with headphones on, or type and press Enter. Ctrl+C hangs up.",
        names.join(" and ")
    );
    let mut typed = BufReader::new(tokio::io::stdin()).lines();
    let mut turns = Turns::default();
    let mut history = History::new();
    let mut last_bot: Option<String> = None;
    let mut reply: Option<Reply> = None;
    let mut last_word_end = 0.0;
    let stream_now = || sent.load(Ordering::Relaxed) as f64 / f64::from(mic_rate);

    loop {
        tokio::select! {
            Some(heard) = heard_rx.recv() => {
                let talking = reply.is_some() || !call.speaker.idle();
                match &heard {
                    Heard::SpeechStarted { at } if talking => {
                        eprintln!("  (you cut in; heard after {:.0} ms)", (stream_now() - at) * 1000.0);
                    }
                    Heard::Final { end, text, .. } if !text.is_empty() => last_word_end = *end,
                    _ => {}
                }
                match turns.on_heard(heard, talking) {
                    Action::Interrupt => interrupt(&mut reply, &call.speaker, &mut history),
                    Action::Reply(said) => {
                        // Time from your last word, not from when the pause was detected.
                        let waited = Duration::from_secs_f64((stream_now() - last_word_end).max(0.0));
                        let started = Instant::now().checked_sub(waited).unwrap_or_else(Instant::now);
                        interrupt(&mut reply, &call.speaker, &mut history);
                        reply = Some(your_turn(&call, said, started, &mut history, &mut last_bot));
                    }
                    Action::Wait => {}
                }
            }
            line = typed.next_line() => match line? {
                Some(line) if !line.trim().is_empty() => {
                    interrupt(&mut reply, &call.speaker, &mut history);
                    let said = line.trim().to_string();
                    reply = Some(your_turn(&call, said, Instant::now(), &mut history, &mut last_bot));
                }
                Some(_) => {}
                None => break,
            },
            result = finished(&mut reply) => {
                let Some(done) = reply.take() else { continue };
                println!();
                let said = done.spoken.lock().map(|s| s.trim().to_string()).unwrap_or_default();
                if !said.is_empty() {
                    history.push((done.bot.clone(), said));
                }
                match result {
                    Ok(Ok(Some(next))) if done.handoffs < MAX_HANDOFFS => {
                        last_bot = Some(next.clone());
                        reply = Some(start(&call, &next, done.handoffs + 1, Instant::now(), &history));
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => eprintln!("({} couldn't answer: {e:#})", done.bot),
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => eprintln!("({} stopped: {e})", done.bot),
                }
            }
        }
    }
    Ok(())
}

/// Waits for the current reply to finish; never resolves when there isn't one.
async fn finished(reply: &mut Option<Reply>) -> Result<Result<Option<String>>, JoinError> {
    match reply {
        Some(r) => (&mut r.task).await,
        None => std::future::pending().await,
    }
}

/// Your turn ended: note what you said and start the right bot's reply.
fn your_turn(
    call: &Arc<Call>,
    said: String,
    started: Instant,
    history: &mut History,
    last_bot: &mut Option<String>,
) -> Reply {
    println!("You: {said}");
    let bot = call.team.pick(&said, last_bot.as_deref()).name.clone();
    history.push(("You".into(), said));
    *last_bot = Some(bot.clone());
    start(call, &bot, 0, started, history)
}

/// Starts `bot` replying to the call so far.
fn start(
    call: &Arc<Call>,
    bot: &str,
    handoffs: usize,
    started: Instant,
    history: &[(String, String)],
) -> Reply {
    let spoken = Arc::new(Mutex::new(String::new()));
    let task = tokio::spawn(speak_reply(
        call.clone(),
        bot.to_string(),
        team::transcript(history),
        spoken.clone(),
        started,
    ));
    Reply {
        bot: bot.to_string(),
        handoffs,
        spoken,
        task,
    }
}

/// You cut in: go quiet and stop the reply, keeping what you heard of it.
fn interrupt(reply: &mut Option<Reply>, speaker: &Speaker, history: &mut History) {
    speaker.stop();
    if let Some(cut) = reply.take() {
        cut.task.abort();
        println!("[cut off]");
        let heard = cut
            .spoken
            .lock()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if !heard.is_empty() {
            history.push((cut.bot, format!("{heard} [cut off]")));
        }
    }
}

/// Streams the bot's reply from Claude and speaks it sentence by sentence as it arrives.
/// Returns the teammate it handed off to, if any.
async fn speak_reply(
    call: Arc<Call>,
    name: String,
    transcript: String,
    spoken: Arc<Mutex<String>>,
    started: Instant,
) -> Result<Option<String>> {
    let Some(bot) = call.team.bot(&name) else {
        bail!("no bot called {name}")
    };
    let events = call
        .claude
        .stream(
            &call.team.instructions(bot),
            &transcript,
            &call.team.tools(bot),
        )
        .await?;
    let mut events = std::pin::pin!(events);
    let mut voice = Voice {
        call: &call,
        bot,
        spoken: &spoken,
        started,
        previous: String::new(),
        first: true,
    };
    let (mut pending, mut hand_off) = (String::new(), None);
    while let Some(event) = events.next().await {
        match event? {
            Event::Text(text) => {
                pending.push_str(&text);
                while let Some(sentence) = turn::take_sentence(&mut pending) {
                    voice.say(&sentence).await?;
                }
            }
            Event::Tool { name, input } if name == "hand_off" => {
                hand_off = call
                    .team
                    .hand_off_to(&bot.name, &input)
                    .map(|b| b.name.clone());
            }
            Event::Tool { .. } => {}
            Event::Done { stop_reason } => {
                if stop_reason == "refusal" {
                    eprintln!("({} declined to answer that)", bot.name);
                }
                if stop_reason != "tool_use" {
                    hand_off = None; // a cut-off or declined turn never runs its tools
                }
            }
        }
    }
    let rest = pending.trim();
    if !rest.is_empty() {
        voice.say(rest).await?;
    }
    Ok(hand_off)
}

/// Speaks one bot's sentences and keeps track of what it said.
struct Voice<'a> {
    call: &'a Call,
    bot: &'a Bot,
    spoken: &'a Mutex<String>,
    started: Instant,
    previous: String,
    first: bool,
}

impl Voice<'_> {
    async fn say(&mut self, sentence: &str) -> Result<()> {
        let (call, bot, started) = (self.call, self.bot, self.started);
        if self.first {
            print!("{}: ", bot.name);
        }
        print!("{sentence} ");
        std::io::stdout().flush().ok();
        let first = &mut self.first;
        call.voices
            .speak(&bot.voice, bot.speed, sentence, &self.previous, |samples| {
                if std::mem::take(first) {
                    eprint!("({} ms to first audio) ", started.elapsed().as_millis());
                }
                call.speaker.play(samples, tts::RATE);
            })
            .await?;
        if let Ok(mut spoken) = self.spoken.lock() {
            spoken.push_str(sentence);
            spoken.push(' ');
        }
        self.previous = sentence.to_string();
        Ok(())
    }
}
