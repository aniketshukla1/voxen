//! Voxen, the Mac app: your main bot, the bots you make, and group chats where they talk to
//! each other, with voice one click away.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tokio::sync::mpsc;
use voice_bots::audio::{self, Mic};
use voice_bots::call::{self, Command};
use voice_bots::claude::Claude;
use voice_bots::team::{Bot, Group, Team};
use voice_bots::tts::Voices;

/// The keys Voxen uses, in the order the setup screen shows them.
const KEY_NAMES: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "DEEPGRAM_API_KEY",
    "ELEVENLABS_API_KEY",
];

/// What the window needs when it opens.
#[derive(Clone, Serialize)]
struct Status {
    /// Text chat works: the Claude key is set and the team loaded.
    chat: bool,
    /// Voice works too: listening and voice keys are set and a speaker was found.
    voice: bool,
    /// Why chat or voice is off, if one is.
    problem: Option<String>,
    keys: Keys,
    main: String,
    bots: Vec<Bot>,
    groups: Vec<Group>,
}

/// Which keys are set.
#[derive(Clone, Serialize)]
struct Keys {
    claude: bool,
    deepgram: bool,
    elevenlabs: bool,
}

#[derive(Serialize)]
struct VoiceOption {
    id: String,
    name: String,
}

/// The window's handle on the conversations.
struct CallState {
    root: PathBuf,
    status: Status,
    commands: Option<mpsc::UnboundedSender<Command>>,
    names: Vec<String>,
    /// For listing voices when you make a bot.
    voices: Option<Voices>,
    mic: Mutex<Option<Mic>>,
}

impl CallState {
    fn off(root: &Path, status: Status) -> Self {
        Self {
            root: root.to_path_buf(),
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
            .ok_or("Chat is off: add your Claude key in Setup.")?;
        commands
            .send(command)
            .map_err(|_| "Voxen's conversations have stopped.".to_string())
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
            status, open, send, stop, clear, voice, add_bot, add_group, voices, save_keys
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("Voxen couldn't start: {e}");
        std::process::exit(1);
    }
}

fn key_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

/// Loads the team and keys and starts the conversations. Anything missing is reported in the
/// window instead of stopping the app, so it always opens.
async fn connect(app: tauri::AppHandle, root: &Path) -> CallState {
    let keys = Keys {
        claude: key_set(KEY_NAMES[0]),
        deepgram: key_set(KEY_NAMES[1]),
        elevenlabs: key_set(KEY_NAMES[2]),
    };
    let team = match Team::load(root.join("team.toml")) {
        Ok(team) => team,
        Err(e) => {
            return CallState::off(
                root,
                Status {
                    chat: false,
                    voice: false,
                    problem: Some(format!("{e:#}")),
                    keys,
                    main: String::new(),
                    bots: Vec::new(),
                    groups: Vec::new(),
                },
            );
        }
    };
    let mut status = Status {
        chat: false,
        voice: false,
        problem: None,
        keys,
        main: team.main.clone(),
        bots: team.bots.clone(),
        groups: team.groups.clone(),
    };
    let Ok(claude) = Claude::from_env() else {
        status.problem = Some("Add your Claude key in Setup to start chatting.".into());
        return CallState::off(root, status);
    };
    let names = team.names();
    let voices = Voices::from_env().ok();
    let speaker = voices.as_ref().and_then(|_| audio::speaker().ok());
    status.chat = true;
    status.voice = speaker.is_some() && status.keys.deepgram;
    let (commands, mut updates) = call::start(team, claude, voices, speaker);
    tauri::async_runtime::spawn(async move {
        while let Some(update) = updates.recv().await {
            app.emit("call", update).ok();
        }
    });
    CallState {
        root: root.to_path_buf(),
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
        .ok_or("Add your ElevenLabs key in Setup to choose from your voices.")?;
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
        return Err("Voice needs your Deepgram and ElevenLabs keys. Add them in Setup.".into());
    }
    let mic = if on {
        let key = std::env::var(KEY_NAMES[1]).map_err(|e| e.to_string())?;
        let commands = state.commands.clone().ok_or("Chat is off.")?;
        Some(call::listen(key, state.names.clone(), commands).map_err(|e| format!("{e:#}"))?)
    } else {
        None // dropping the Mic closes it and stops listening
    };
    *state.mic.lock().map_err(|e| e.to_string())? = mic;
    state.tell(Command::Speak(on))
}

/// Saves the keys you pasted into `.env` (readable only by you) and restarts so they take effect.
#[tauri::command]
fn save_keys(
    keys: HashMap<String, String>,
    app: tauri::AppHandle,
    state: State<'_, CallState>,
) -> Result<(), String> {
    let path = state.root.join(".env");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = merge_env(&current, &keys)?;
    write_private(&path, &updated).map_err(|e| format!("Couldn't save the keys: {e}"))?;
    app.restart()
}

/// Puts each key into the `.env` text, replacing its line or adding one. Empty values are skipped.
fn merge_env(current: &str, keys: &HashMap<String, String>) -> Result<String, String> {
    let mut lines: Vec<String> = current.lines().map(str::to_string).collect();
    for name in KEY_NAMES {
        let Some(value) = keys.get(name).map(|v| v.trim()).filter(|v| !v.is_empty()) else {
            continue;
        };
        if value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\''))
        {
            return Err(format!("{name} has characters a key can't contain."));
        }
        let line = format!("{name}={value}");
        match lines
            .iter()
            .position(|l| l.trim_start().starts_with(&format!("{name}=")))
        {
            Some(i) => lines[i] = line,
            None => lines.push(line),
        }
    }
    if let Some(unknown) = keys.keys().find(|k| !KEY_NAMES.contains(&k.as_str())) {
        return Err(format!("{unknown} isn't a key Voxen uses."));
    }
    Ok(lines.join("\n") + "\n")
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn keys_replace_their_line_or_get_added() {
        let current = "# my keys\nANTHROPIC_API_KEY=old\n# CLAUDE_MODEL=claude-opus-5-5\n";
        let merged = merge_env(
            current,
            &keys(&[
                ("ANTHROPIC_API_KEY", " new "),
                ("DEEPGRAM_API_KEY", "dg"),
                ("ELEVENLABS_API_KEY", ""),
            ]),
        )
        .unwrap();
        assert_eq!(
            merged,
            "# my keys\nANTHROPIC_API_KEY=new\n# CLAUDE_MODEL=claude-opus-5-5\nDEEPGRAM_API_KEY=dg\n"
        );
    }

    #[test]
    fn odd_keys_are_refused() {
        assert!(merge_env("", &keys(&[("ANTHROPIC_API_KEY", "a b")])).is_err());
        assert!(
            merge_env("", &keys(&[("PATH", "/bin")]))
                .unwrap_err()
                .contains("isn't a key")
        );
    }
}
