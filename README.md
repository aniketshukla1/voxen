# voice-bots

A voice call with a team of AI people. Talk (or type), and the right bot answers
out loud in its own voice. Plan: [CAPABILITY-MAP.md](CAPABILITY-MAP.md).

## Setup (about 10 minutes)

1. Create three API keys:
   - Claude: console.anthropic.com → API keys
   - Listening: console.deepgram.com → API keys
   - Voices: elevenlabs.io → Developers → API keys
2. `cp .env.example .env` and paste the keys into `.env`.
3. Check each key on its own: `cargo test -- --ignored`
4. Put headphones on (through speakers the bots hear themselves and stop).
5. `cargo run --release`. macOS asks for microphone access the first time.

Say "Maya, what's the capital of Japan?", then "Sam, what's 15% of 80?".
Talk over a bot to stop it. Typing a line and pressing Enter works too.

## Change the team

Edit [team.toml](team.toml): names, ElevenLabs voice ids, speeds, roles. No code changes.

## Code

`src/lib.rs` is our own SDK: clients for Claude, Deepgram and ElevenLabs, audio,
turn-taking and the team. `src/main.rs` is the app. `cargo test` runs offline.
