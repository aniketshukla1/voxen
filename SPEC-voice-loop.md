# Spec: voice-loop

Module of [CAPABILITY-MAP.md](CAPABILITY-MAP.md). Status: draft, waiting for approval.

## Objective

Talk to one bot from your Mac terminal. It hears you, knows when you have finished,
answers out loud and stops the moment you talk over it. Every later module runs on
this engine, so it has to feel fast from day one.

One turn, every step streaming:

```
mic → Deepgram (live speech-to-text, end-of-turn signal)
    → Claude (reply streams as text)
    → split into sentences as they arrive
    → ElevenLabs (each sentence streams back as audio)
    → speaker
```

When you start talking while the bot speaks, playback stops, the Claude and voice
streams are cancelled, and the conversation history keeps only the part you heard.
You can also type a line and press Enter instead of speaking. That serves as the
chat option, and it lets us test without a microphone.

## Tech stack

- Rust 1.98 (installed), `tokio` async runtime.
- Our own SDK: the library half of this crate (`src/lib.rs`). It holds our clients for
  Claude (HTTPS and server-sent events), Deepgram (WebSocket), ElevenLabs (WebSocket)
  and Mac audio in and out. No vendor SDKs.
- Starting crates: `tokio`, `reqwest` (rustls, streaming), `tokio-tungstenite`
  (rustls), `serde`, `serde_json`, `futures`, `cpal` (audio), `base64`, `anyhow`,
  `dotenvy` (loads `.env`). Anything else needs your OK first.
- Brain: Claude Opus 5.5 (`claude-opus-5-5`) at low effort, to keep replies quick.
  Requests use Anthropic's server-side fallback, which retries on another model if a
  safety check declines a request.
- Keys live in `.env` (gitignored): `ANTHROPIC_API_KEY`, `DEEPGRAM_API_KEY`,
  `ELEVENLABS_API_KEY`. `.env.example` lists the names with no values.

## Commands

```
cargo run --release              # talk to the bot (headphones on)
cargo test                       # all tests, no network
cargo test -- --ignored          # live smoke tests, needs the keys
cargo clippy -- -D warnings      # lint
cargo fmt                        # format
```

## Project structure

```
Cargo.toml
.env.example      key names only
src/lib.rs        our SDK: declares the modules below
src/claude.rs     Claude Messages API client, streaming
src/stt.rs        Deepgram live speech-to-text client
src/tts.rs        ElevenLabs streaming voice client
src/audio.rs      mic capture and speaker playback
src/turn.rs       turn-taking: listening → thinking → speaking, interruptions
src/main.rs       the app: loads keys, wires the parts, prints per-turn timings
```

## Code style

Small pure functions with a doc comment, tests in the same file, no `unwrap()` outside
tests, `anyhow::Result` for errors until the SDK has outside users.

```rust
/// Pops the next complete sentence off a streaming buffer, so speech can start
/// before Claude finishes. A sentence ends at . ! ? or a newline followed by
/// whitespace, so "3.5" stays whole.
pub fn take_sentence(buf: &mut String) -> Option<String> {
    loop {
        let end = buf
            .char_indices()
            .zip(buf.chars().skip(1))
            .find(|((_, c), next)| matches!(c, '.' | '!' | '?' | '\n') && next.is_whitespace())
            .map(|((i, c), _)| i + c.len_utf8())?;
        let sentence = buf.drain(..end).collect::<String>().trim().to_string();
        if !sentence.is_empty() {
            return Some(sentence);
        }
    }
}
```

## Testing strategy

- Unit tests next to the code for every parser and decision: sentence splitting,
  Claude stream events, Deepgram messages, and the turn state machine (including
  interruptions).
- Provider tests replay recorded messages kept as strings in the test, so
  `cargo test` never touches the network.
- One live smoke test per provider, marked `#[ignore]`, run with the keys.
- The feels-real targets are checked by hand. The app prints each turn's timing
  (you stopped → first audio) and how fast an interruption went quiet.

## Boundaries

- Always: stream every stage; print per-turn timings; keep keys in `.env` only;
  `cargo fmt`, `cargo clippy` and `cargo test` clean before calling a step done.
- Ask first: a new crate, a different provider or model, anything that costs money
  beyond normal API use.
- Never: vendor SDKs; secrets in code or git; saving audio or transcripts to disk;
  deleting a failing test.

## Success criteria

1. With the three keys in `.env`, `cargo run --release` lets you speak and hear a
   spoken answer.
2. Over 10 turns, the median time from you finishing to first audio is 1.0 s or less.
   The model is the biggest lever if it's slower (see open question 1).
3. Talking over the bot silences it within 0.3 s, and the app prints how long it took.
4. Follow-ups keep context: "What's the capital of Japan?" then "And its population?"
5. A typed line gets the same spoken answer as speech.
6. `cargo test` passes offline and `cargo clippy -- -D warnings` is clean.

## Not in this module

Phone, apps, wake word, multiple bots (that's `team`), saving anything to disk.

## Open questions

1. Speed or smarts: if Opus 5.5 misses the 1-second target, do we try Haiku 5.5
   (about 40× cheaper and faster) for chat turns? Your call after we measure.
2. Headphones are required in this version, because through speakers the mic hears
   the bot and it interrupts itself. Should echo cancellation (macOS voice
   processing) go in this module or wait?
3. Three accounts are needed: Anthropic, Deepgram, ElevenLabs. All have free credit
   or a free tier to start. OK, or use Deepgram's own voices to save one signup?
