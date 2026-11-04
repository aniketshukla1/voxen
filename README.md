# voice-bots

A voice call with a team of AI people. Talk (or type), and the right bot answers
out loud in its own voice. Plan: [CAPABILITY-MAP.md](CAPABILITY-MAP.md).

## Open the app

```
cargo run -p voice-team
```

The window opens even without keys and says what's missing. Typing needs only the Claude key;
the mic button (voice) needs all three.

## Keys (about 10 minutes)

1. Create the API keys:
   - Claude (text and voice): console.anthropic.com → API keys
   - Listening (voice): console.deepgram.com → API keys
   - Voices (voice): elevenlabs.io → Developers → API keys
2. `cp .env.example .env` and paste the keys into `.env`, then reopen the app.
3. Check each key on its own: `cargo test -- --ignored`
4. For voice, put headphones on (through speakers the bots hear themselves and stop).
   macOS asks for microphone access the first time.

Say "Maya, what's the capital of Japan?", then "Sam, what's 15% of 80?". Talk over a bot,
press Esc or click Stop to cut it off.

The terminal version still works: `cargo run --release`.

## Your bots

Maya is the main bot. Create more bots and group chats with the + buttons in the sidebar.
In a group chat the bots take turns and react to each other. Everything is saved in
[team.toml](team.toml), which you can also edit by hand.

## Code

`src/lib.rs` is our own SDK: clients for Claude, Deepgram and ElevenLabs, audio,
turn-taking, the team, and the call engine (`src/call.rs`) both front-ends share.
`app/` is the Mac app (Tauri, with the window in `app/ui/`); `src/main.rs` is the terminal
version. `cargo test` runs offline.
