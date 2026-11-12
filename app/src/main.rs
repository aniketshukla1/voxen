//! Voxen, the Mac app: your main bot, the bots you make, and group chats where they talk to
//! each other, with voice one click away.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tokio::sync::mpsc;
use voice_bots::audio::{self, Mic};
use voice_bots::call::{self, Command};
use voice_bots::llm::{self, Brains, Provider};
use voice_bots::team::{Bot, Group, Team};
use voice_bots::tools::{Attachment, Memory};
use voice_bots::tts::Voices;
use voice_bots::xai::Xai;

const DEEPGRAM: &str = "DEEPGRAM_API_KEY";
const ELEVENLABS: &str = "ELEVENLABS_API_KEY";

/// Every key Settings can save: the two voice keys, each provider's key, and the address of
/// an OpenAI-compatible server of your own.
fn key_names() -> Vec<&'static str> {
    let mut names = vec![DEEPGRAM, ELEVENLABS];
    names.extend(llm::PROVIDERS.iter().filter_map(|p| p.key));
    names.push("CUSTOM_BASE_URL");
    names
}

/// What the window needs when it opens.
#[derive(Clone, Serialize)]
struct Status {
    /// Text chat works: the main bot's provider has its key and the team loaded.
    chat: bool,
    /// Voice works too: listening and voice keys are set and a speaker was found.
    voice: bool,
    /// Why chat or voice is off, if one is.
    problem: Option<String>,
    /// Which settings are filled in, by name.
    keys: BTreeMap<String, bool>,
    /// Every provider a bot can think with.
    providers: Vec<Brain>,
    main: String,
    bots: Vec<Bot>,
    groups: Vec<Group>,
    /// What the bots remember about you, for Settings.
    memory: Vec<String>,
    /// The voice models pulled into Ollama on this Mac, like "phonon".
    local_voices: Vec<String>,
    /// This build of Voxen.
    version: &'static str,
}

/// A provider, and whether it can be used now.
#[derive(Clone, Serialize)]
struct Brain {
    #[serde(flatten)]
    provider: &'static Provider,
    ready: bool,
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
            .ok_or("Voxen couldn't load your team; check team.toml.")?;
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
            status, open, send, attach, stop, clear, voice, add_bot, update_bot, set_main,
            remove_bot, add_group, update_group, remove_group, voices, models, save_keys, open_link,
            zoom, remember, forget, open_data, remove_keys
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
    let keys: BTreeMap<String, bool> = key_names().into_iter().map(|k| (k.to_string(), key_set(k))).collect();
    let brains = Brains::new();
    let providers: Vec<Brain> = llm::PROVIDERS
        .iter()
        .map(|provider| Brain { provider, ready: brains.ready(provider.id) })
        .collect();
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
                    providers,
                    main: String::new(),
                    bots: Vec::new(),
                    groups: Vec::new(),
                    memory: Vec::new(),
                    local_voices: Vec::new(),
                    version: env!("CARGO_PKG_VERSION"),
                },
            );
        }
    };
    let mut status = Status {
        chat: false,
        voice: false,
        problem: None,
        keys,
        providers,
        main: team.main.clone(),
        bots: team.bots.clone(),
        groups: team.groups.clone(),
        memory: Memory::load(Some(root.join("data").join("memory.md"))).facts,
        local_voices: Vec::new(),
        version: env!("CARGO_PKG_VERSION"),
    };
    let thinker = team.main_bot().provider.clone();
    status.chat = brains.ready(&thinker);
    if !status.chat {
        let name = llm::find(&thinker).map_or("your provider", |p| p.name);
        status.problem = Some(format!("Add your {name} key in Settings to start chatting."));
    }
    let names = team.names();
    // Bots speak through ElevenLabs, or through a voice model pulled into Ollama on this Mac.
    let voices = Voices::from_env();
    status.local_voices = voices.local_models().await;
    let speaker = (voices.eleven() || !status.local_voices.is_empty())
        .then(|| audio::speaker().ok())
        .flatten();
    status.voice = speaker.is_some() && key_set(DEEPGRAM);
    let speaking = speaker.is_some().then(|| voices.clone());
    let (commands, mut updates) = call::start(team, brains, speaking, speaker, Xai::from_env().ok());
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
        voices: Some(voices),
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

/// Files for the bots to look at with your next turn.
#[tauri::command]
fn attach(files: Vec<Attachment>, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Attach(files))
}

/// Opens a source link in your browser. Only web links, so a reply can't open anything else.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web links open.".into());
    }
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open the link: {e}"))
}

/// Adds something every bot should know about you, from Settings › Memory.
#[tauri::command]
fn remember(fact: String, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Remember(fact))
}

/// Forgets one fact exactly as saved, or everything when `fact` is missing.
#[tauri::command]
fn forget(fact: Option<String>, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::Forget(fact))
}

/// Shows the folder where Voxen keeps what the bots remember, your tasks and pictures. It
/// takes no path, so nothing else can be opened through it.
#[tauri::command]
fn open_data(state: State<'_, CallState>) -> Result<(), String> {
    let data = state.root.join("data");
    let folder = if data.is_dir() { data } else { state.root.clone() };
    std::process::Command::new("open")
        .arg(folder)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open the folder: {e}"))
}

/// Makes everything in the window bigger or smaller, from Settings › Look.
#[tauri::command]
fn zoom(scale: f64, window: tauri::WebviewWindow) -> Result<(), String> {
    window.set_zoom(scale.clamp(0.5, 2.0)).map_err(|e| e.to_string())
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

/// Changes one of your bots: its brain, voice, colour or what it's for.
#[tauri::command]
fn update_bot(bot: Bot, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::UpdateBot(bot))
}

/// Makes another bot the main one, the bot you talk to by default.
#[tauri::command]
fn set_main(name: String, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::SetMain(name))
}

/// Removes a bot (not the main one) and takes it out of its group chats.
#[tauri::command]
fn remove_bot(name: String, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::RemoveBot(name))
}

/// Changes who is in a group chat.
#[tauri::command]
fn update_group(group: Group, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::UpdateGroup(group))
}

#[tauri::command]
fn remove_group(name: String, state: State<'_, CallState>) -> Result<(), String> {
    state.tell(Command::RemoveGroup(name))
}

/// The models a provider offers right now.
#[tauri::command]
async fn models(provider: String) -> Result<Vec<String>, String> {
    let brain = Brains::new().llm(&provider, "list").map_err(|e| format!("{e:#}"))?;
    brain.models().await.map_err(|e| format!("{e:#}"))
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
        .ok_or("Add your ElevenLabs key, or pull a voice with Ollama, in Settings to choose from your voices.")?;
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
        return Err("Voice needs your Deepgram key, and ElevenLabs or a voice model in Ollama to speak with. See Settings › Voice.".into());
    }
    let mic = if on {
        let key = std::env::var(DEEPGRAM).map_err(|e| e.to_string())?;
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
    restart_without(&app, keys.keys())
}

/// Takes keys out of `.env` and restarts without them, so Voxen stops using those services.
#[tauri::command]
fn remove_keys(names: Vec<String>, app: tauri::AppHandle, state: State<'_, CallState>) -> Result<(), String> {
    let known = key_names();
    if let Some(unknown) = names.iter().find(|n| !known.contains(&n.as_str())) {
        return Err(format!("{unknown} isn't a key Voxen uses."));
    }
    let path = state.root.join(".env");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    write_private(&path, &drop_env(&current, &names)).map_err(|e| format!("Couldn't save the keys: {e}"))?;
    restart_without(&app, &names)
}

/// Starts Voxen again and quits this one. This one loaded `.env` into its environment when it
/// started, and a restart inherits that, so the keys just changed are left out: the new Voxen
/// reads them fresh from `.env`, or not at all once they're removed.
fn restart_without<'a>(app: &tauri::AppHandle, names: impl IntoIterator<Item = &'a String>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Couldn't restart Voxen: {e}"))?;
    let mut again = std::process::Command::new(exe);
    again.args(std::env::args_os().skip(1));
    for name in names {
        again.env_remove(name);
    }
    again.spawn().map_err(|e| format!("Couldn't restart Voxen: {e}"))?;
    app.exit(0);
    Ok(())
}

/// The `.env` text without the lines that set `names`.
fn drop_env(current: &str, names: &[String]) -> String {
    let kept: Vec<&str> = current
        .lines()
        .filter(|line| !names.iter().any(|n| line.trim_start().starts_with(&format!("{n}="))))
        .collect();
    kept.join("\n") + "\n"
}

/// Puts each key into the `.env` text, replacing its line or adding one. Empty values are skipped.
fn merge_env(current: &str, keys: &HashMap<String, String>) -> Result<String, String> {
    let mut lines: Vec<String> = current.lines().map(str::to_string).collect();
    let names = key_names();
    for name in names.iter().copied() {
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
    if let Some(unknown) = keys.keys().find(|k| !names.contains(&k.as_str())) {
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
    fn removed_keys_lose_only_their_lines() {
        let current = "# my keys\nANTHROPIC_API_KEY=a\nOPENAI_API_KEY=o\n  CUSTOM_BASE_URL=http://x\n";
        let names = ["OPENAI_API_KEY".to_string(), "CUSTOM_BASE_URL".to_string()];
        assert_eq!(drop_env(current, &names), "# my keys\nANTHROPIC_API_KEY=a\n");
        assert_eq!(drop_env("", &names), "\n");
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
