# Changelog

What changed in each Voxen release, newest first.

## [Unreleased]

### Added

- A Settings page (⌘, or the button at the bottom of the sidebar), with your keys in it.
- Settings › Bots: edit any bot, make another one the main bot, or remove one.
- Settings › Rooms: change who's in a room, or remove it.
- Settings › Voice: start with voice on, and see what voice still needs.
- Settings › Look: dark, light or matching the Mac, a bigger or smaller window, and reduced
  motion.

### Changed

- No setup dialog on first run. The empty room says what's missing, and its button opens
  Settings at Keys.

## [0.1.0] - 2026-11-10

The first public release.

### Added

- A Mac app where you talk to a team of bots, one at a time or in group chats, by voice or by
  typing. Every bot answers in its own ElevenLabs voice, and you can talk over it to cut it off.
- Bots and group chats you make in the app, saved in `team.toml`.
- A model for every bot, picked from 17 providers: Anthropic, OpenAI, Google, xAI, Mistral,
  DeepSeek, Perplexity, OpenRouter, Groq, Together, Fireworks, Cerebras, Moonshot, Qwen, Ollama,
  LM Studio or any OpenAI-compatible server. Switch it any time.
- Web search and page reading, with sources under each answer. Every reply decides on its own
  how much to think and research.
- Memory shared by every bot, saved in `data/memory.md`.
- Tasks each bot runs on its own schedule (every few minutes, hours, days, weeks, months or
  weekdays), managed by voice or chat.
- Attachments: pictures, PDFs and text files, by button, drag or paste.
- X search and pictures with Grok, with an optional xAI key.
- A living particle Orb for every bot that breathes, listens, thinks and speaks. Groups show
  their members' Orbs overlapping.
- Setup inside the app for every key.
- A terminal version: `cargo run --release`.

[0.1.0]: https://github.com/aniketshukla1/voxen/releases/tag/v0.1.0
