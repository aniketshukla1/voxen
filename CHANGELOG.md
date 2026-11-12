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
- Settings › Memory: see what every bot knows about you, add to it, or forget things.
- Settings › Privacy & data, Shortcuts and About: where everything is kept (with Show in
  Finder), what leaves your Mac, every shortcut, and the version with its links.
- Company logos in Connections, in one colour to match Voxen's black and white.
- Bots can speak with voices that run on your Mac. Pull Phonon into Ollama
  (`ollama pull phonon`) and give any bot one of its 38 voices, with no key. Voice works with
  Ollama's voices, ElevenLabs, or both.

### Changed

- No setup dialog on first run. The empty room says what's missing, and its button opens
  Settings at Keys.
- Settings has a new look: an icon for every section and row, and Keys is now Connections.
  It shows whether chat and voice are ready, which bots use each service, and whether each
  key works (Voxen asks each provider for its models). You connect, replace or disconnect a
  key in place. Look shows each theme as a small window, and Voice ticks off what it needs.
- Picking an Ollama model lists only the models that can chat. Ollama says what each model
  can do, so its voice models stay out.

### Fixed

- A key you replace in Settings now takes effect after Save and restart. Before, Voxen kept
  using the old one until you quit it.
- Bots on Ollama work. Voxen assumed `llama3.2`, so a bot set to Ollama failed unless you had
  that model. Now it uses a chat model you've pulled, and Connections lists them. A model that
  isn't pulled says how to get it. The bot dialog stays open with the reason when a change
  can't be saved, where before it closed as if the change had worked.

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
