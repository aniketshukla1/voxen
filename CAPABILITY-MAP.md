# Capability map: voice team

A voice-first assistant for anyone who would rather talk than type. You talk to a
team of named AI people, each with their own voice. They discuss, take your
interruptions and get things done. Chat is optional. Native apps and a phone
number, no web app.

Status: approved 2026-11-04, with the backend in Rust.

| Module id | Responsibility | Depends on |
|---|---|---|
| `voice-loop` | Hear you, detect the end of your turn, stream a spoken reply, stop when you talk | — |
| `team` | 2–4 named bots, each with its own voice, role and personality; call by name; hand-offs | voice-loop |
| `phone-line` | A phone number that reaches your team | voice-loop |
| `actions` | Search, reminders, calendar, email; read back and wait for a spoken "yes" before anything final | team |
| `huddle` | Bots discuss out loud, you cut in, one bot sums up | team |
| `memory` | Remembers you, your people and your preferences between calls | team |
| `accounts` | Sign-in, each person's team settings, usage limits | — |
| `mobile-app` | iPhone and Android; keeps listening with the screen locked during a call | voice-loop, accounts |
| `desktop-app` | Mac and Windows | voice-loop, accounts |

Build order: voice-loop → team (the MVP) → desktop-app (first version) → phone-line →
actions, huddle, memory → accounts → mobile-app

## Feels-real targets (every module)

1. A bot starts speaking within about 1 second of you finishing.
2. A bot goes quiet within about 0.3 seconds of you starting to talk.
3. With your eyes closed, you can tell every bot apart.
4. A huddle lasts under about 20 seconds unless you ask for more.
5. Nothing irreversible happens without your spoken "yes".

## Not doing (for now)

- Web app.
- Wake word while the app is closed. iPhone doesn't allow it; an open call covers it.
- Animated avatars. Voice is the selling point.
- Cloning real people's voices. Consent and legal risk.

## Decisions

- 2026-11-04: Backend in Rust, on our own SDK: our own clients for every provider,
  no vendor SDKs. Accepted cost: several extra days of plumbing up front.
- 2026-11-04: UI first. `desktop-app` moved up: a Mac chat app (Tauri) laid out like Grok,
  ChatGPT and Claude, with text always shown and voice as a mode in the same chat. Text chat
  needs only the Claude key. See [SPEC-desktop-app.md](SPEC-desktop-app.md).
- 2026-11-04: Grok-style structure. A main bot you talk to by default, bots you create for any
  use case (each with its own chat, voice and personality, saved to `team.toml`), and group
  chats where the bots take turns reacting to each other. That covers the first part of
  `huddle`; one bot summing up comes later. Bots are yours alone until `accounts` exists.
