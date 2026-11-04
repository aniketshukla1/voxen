//! The Voice Team Mac app: your main bot, the bots you make, and group chats where they talk
//! to each other, with voice one click away.

use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tokio::sync::mpsc;
use voice_bots::audio::{self, Mic};
use voice_bots::call::{self, Command};
use voice_bots::claude::Claude;
use voice_bots::team::{Bot, Group, Team};
use voice_bots::tts::Voices;

/// What the window needs when it opens.
#[derive(Clone, Serialize)]
struct Status {
    /// Text chat works: the Claude key is set and the team loaded.
    chat: bool,
    /// Voice works too: listening and voice keys are set and a speaker was found.
    voice: bool,
    /// Why chat or voice is off, if one is.
    problem: Option<String>,
    main: String,
    bots: Vec<Bot>,
    groups: Vec<Group>,
}

#[derive(Serialize)]
struct VoiceOption {
    id: String,
    name: String,
}

/// The window's handle on the conversations.
struct CallState {
    status: Status,
    commands: Option<mpsc::UnboundedSender<Command>>,
    names: Vec<String>,
    /// For listing voices when you make a bot.
    voices: Option<Voices>,
    mic: Mutex<Option<Mic>>,
}

impl CallState {
    fn off(status: Status) -> Self {
        Self {
            status,
            commands: None,
            names: Vec::new(),
            voices: None,
            mic: Mutex::new(None),
        }
    }

    fn tell(&self, command: Command) -> Result<(), String> {
        let commands = self
            .commands
            .as_ref()
            .ok_or("Chat is off: add your ANTHROPIC_API_KEY to .env, then reopen the app.")?;
        commands
            .send(command)
            .map_err(|_| "The app's conversations have stopped.".to_string())
    }
}

fn main() {
    // ponytail: keys and team come from the repo checkout; bundle them as app resources when it ships.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    dotenvy::from_path(root.join(".env")).ok();
    let result = tauri::Builder::default()
        .setup(move |app| {
            let state = tauri::async_runtime::block_on(connect(app.handle().clone(), &root));
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status, open, send, stop, clear, voice, add_bot, add_group, voices
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("Voice Team couldn't start: {e}");
        std::process::exit(1);
    }
}

/// Loads the team and keys and starts the conversations. Anything missing is reported in the
/// window instead of stopping the app, so it always opens.
async fn connect(app: tauri::AppHandle, root: &Path) -> CallState {
    let team = match Team::load(root.join("team.toml")) {
        Ok(team) => team,
        Err(e) => {
            return CallState::off(Status {
                chat: false,
                voice: false,
                problem: Some(format!("{e:#}")),
                main: String::new(),
                bots: Vec::new(),
                groups: Vec::new(),
            });
        }
    };
    let mut status = Status {
        chat: false,
        voice: false,
        problem: None,
        main: team.main.clone(),
        bots: team.bots.clone(),
        groups: team.groups.clone(),
    };
    let Ok(claude) = Claude::from_env() else {
        status.problem = Some("Add your ANTHROPIC_API_KEY to .env, then reopen the app.".into());
        return CallState::off(status);
    };
    let names = team.names();
    let voices = Voices::from_env().ok();
    let speaker = voices.as_ref().and_then(|_| audio::speaker().ok());
    status.chat = true;
    status.voice = speaker.is_some() && std::env::var("DEEPGRAM_API_KEY").is_ok();
    if !status.voice {
        status.problem = Some(
            "Voice needs DEEPGRAM_API_KEY and ELEVENLABS_API_KEY in .env. Typing works now.".into(),
        );
    }
    let (commands, mut updates) = call::start(team, claude, voices, speaker);
    tauri::async_runtime::spawn(async move {
        while let Some(update) = updates.recv().await {
            app.emit("call", update).ok();
        }
    });
    CallState {
        status,
        commands: Some(commands),
        names,
        voices: Voices::from_env().ok(),
        mic: Mutex::new(None),
    }
}

#[tauri::command]
fn status(state: State<'_, CallState>) -> Status {
    state.status.clone()
}

#[tauri::command]
fn open(chat: String, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Open(chat))
}

#[tauri::command]
fn send(text: String, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Typed(text))
}

#[tauri::command]
fn stop(state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Stop)
}

#[tauri::command]
fn clear(state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Clear)
}

#[tauri::command]
fn add_bot(bot: Bot, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::AddBot(bot))
}

#[tauri::command]
fn add_group(group: Group, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::AddGroup(group))
}

/// The ElevenLabs voices you can give a new bot.
#[tauri::command]
async fn voices(state: State<'_, CallState>) -> Result<Vec<VoiceOption>, String> {
    let voices = state
        .voices
        .as_ref()
        .ok_or("Add ELEVENLABS_API_KEY to .env to choose from your voices.")?;
    let list = voices.list().await.map_err(|e| format!("{e:#}"))?;
    Ok(list
        .into_iter()
        .map(|(id, name)| VoiceOption { id, name })
        .collect())
}

/// Turns voice on (open the mic and say replies out loud) or off.
#[tauri::command]
async fn voice(on: bool, state: State<'_, CallState>) -> Result<(), String> {
    if on && !state.status.voice {
        return Err(state
            .status
            .problem
            .clone()
            .unwrap_or_else(|| "Voice is off.".into()));
    }
    let mic = if on {
        let key = std::env::var("DEEPGRAM_API_KEY").map_err(|e| e.to_string())?;
        let commands = state.commands.clone().ok_or("Chat is off.")?;
        Some(call::listen(key, state.names.clone(), commands).map_err(|e| format!("{e:#}"))?)
    } else {
        None // dropping the Mic closes it and stops listening
    };
    *state.mic.lock().map_err(|e| e.to_string())? = mic;
    state.tell(Command::Speak(on))
}
