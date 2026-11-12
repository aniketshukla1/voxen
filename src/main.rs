//! The terminal front-end: talk (or type) to your main bot, and it answers out loud.

use anyhow::{Context, Result};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};
use voice_bots::audio;
use voice_bots::call::{self, Command, Update};
use voice_bots::llm::Brains;
use voice_bots::team::Team;
use voice_bots::tts::Voices;
use voice_bots::xai::Xai;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let deepgram_key = std::env::var("DEEPGRAM_API_KEY")
        .context("DEEPGRAM_API_KEY is not set (copy .env.example to .env)")?;
    let team = Team::load("team.toml")?;
    let (names, main) = (team.names(), team.main_bot().name.clone());
    let (commands, mut updates) = call::start(
        team,
        Brains::new(),
        Some(Voices::from_env()),
        Some(audio::speaker()?),
        Xai::from_env().ok(),
    );
    commands.send(Command::Speak(true)).ok();
    let _mic = call::listen(deepgram_key, names, commands.clone())?;

    println!(
        "On a call with {main}. Talk with headphones on, or type and press Enter. Ctrl+C hangs up."
    );
    let mut typed = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = typed.next_line() => match line? {
                Some(line) => {
                    commands.send(Command::Typed(line)).ok();
                }
                None => break,
            },
            Some(update) = updates.recv() => show(update.update),
        }
    }
    Ok(())
}

/// Prints one update.
fn show(update: Update) {
    match update {
        Update::You { text } => println!("You: {text}"),
        Update::Start { bot } => print!("{bot}: "),
        Update::Text { text, .. } => print!("{text}"),
        Update::Done { .. } => println!(),
        Update::Cut {
            after_ms: Some(ms), ..
        } => println!(" [cut off; heard you after {ms} ms]"),
        Update::Cut { after_ms: None, .. } => println!(" [cut off]"),
        Update::FirstAudio { ms, .. } => eprint!(" ({ms} ms to first audio) "),
        Update::Step { text, .. } => eprint!(" ({text}) "),
        Update::Sources { sources, .. } => {
            for page in sources {
                println!("  - {} <{}>", page.title, page.url);
            }
        }
        Update::Image { url, .. } => println!("\n[picture: {url}]"),
        Update::Task { bot, task } => println!("\n[{bot}, scheduled: {task}]"),
        Update::Problem { text } => eprintln!("({text})"),
        Update::Hearing { .. } | Update::Team { .. } | Update::Levels { .. } | Update::Memory { .. } => {}
    }
    std::io::stdout().flush().ok();
}
