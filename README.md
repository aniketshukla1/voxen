# Voxen

Your team of voices. Talk to one of your bots, or to all of them in a group chat, and they
answer out loud in their own voices. Plan: [CAPABILITY-MAP.md](CAPABILITY-MAP.md).
Identity: [docs/brand-guidelines.md](docs/brand-guidelines.md).

## Open the app

```
cargo run -p voxen
```

The window opens even without keys and walks you through setup: paste your keys there and
Voxen restarts with them. Typing needs a key for the provider your main bot thinks with
(Claude to start with); voice also needs Deepgram and ElevenLabs.

## Keys (about 10 minutes)

1. Create the API keys:
   - Claude (text and voice): console.anthropic.com → API keys
   - Listening (voice): console.deepgram.com → API keys
   - Voices (voice): elevenlabs.io → Developers → API keys
   - Optional, for X search and pictures: console.x.ai → API keys
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

## Each bot thinks with the brain you pick

Every bot can use a different provider and model: Anthropic Claude, OpenAI, Google Gemini,
xAI Grok, Mistral, DeepSeek, Perplexity, OpenRouter (hundreds of models), Groq, Together,
Fireworks, Cerebras, Moonshot Kimi, Alibaba Qwen, Ollama or LM Studio on your Mac, or any
OpenAI-compatible server. Pick it when you make a bot, or click a bot (or Change voice) to
switch it any time; the model list loads live from the provider. Add the provider's key in
Setup first. Web search works with Claude, OpenAI, Gemini, Grok and Perplexity; memory and
reminders work with every provider except Perplexity.

## What the bots can do

| Ask | What happens |
|---|---|
| "What's the weather in Lisbon tomorrow?" | Searches the web and reads pages; the sources show under the answer. |
| "Remember I'm vegetarian." | Every bot knows it in later chats. Saved in `data/memory.md`, which you can edit. |
| "Remind me to call Mum at 6" / "Every morning at 8, brief me on the news" | The bot does it at that time, out loud. Saved in `data/tasks.json`. |
| Attach a picture, PDF or text file (button, drag or paste) | The bots look at it with your next message, typed or spoken. |
| "Compare these three laptops for me" | Like Grok, every reply decides how much to think and research: quick questions stay quick, hard ones get deep research. |
| "What are people on X saying about…" / "Make a picture of…" | Grok's X search and Grok Imagine. Needs an xAI key (optional, in Setup); pictures are saved in `data/pictures/`. |

`data/` is personal and never committed.

## Code

`src/lib.rs` is our own SDK: clients for Claude, Deepgram, ElevenLabs and xAI, audio,
turn-taking, the team, the bots' tools (`src/tools.rs`), and the call engine
(`src/call.rs`) both front-ends share.
`app/` is the Mac app (Tauri, with the window in `app/ui/`); `src/main.rs` is the terminal
version. `cargo test` runs offline.
