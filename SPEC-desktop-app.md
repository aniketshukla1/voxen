# Spec: desktop-app

Module of [CAPABILITY-MAP.md](CAPABILITY-MAP.md). Status: first version built 2026-11-04.

## Objective

A Mac app laid out like the chat apps people know (Grok, ChatGPT, Claude): a main bot you talk
to by default, bots you create for any use case, and group chats where bots talk to each other
while you jump in. Every reply shows as text; voice is a mode inside the same chat. Typing works
with only the Claude key; voice adds listening and spoken replies.

## Layout

- Sidebar: your main bot; "Your bots" with a + to create one (name, what it's for,
  personality, voice, speaking speed); "Group chats" with a + to start one (a name and two or
  more bots); whether chat and voice are ready.
- Each bot and each group has its own chat. The header shows who's in it.
- Conversation: your messages as bubbles on the right; each bot's reply on the left under its
  avatar and name, streaming in as it's written.
- Group chats: after you write, the bot you named answers first (or the first member), then
  each other member reacts in turn. Writing or talking again cuts the round short.
- Composer: a message box with a mic button (voice on or off) and send. Voice bar while voice
  is on: who is speaking or listening, Stop, End voice.
- Dark or light, following the Mac's appearance setting.

## Tech

Tauri 2 in `app/`, with plain HTML, CSS and JS in `app/ui/` (no framework). The window talks to
the conversation engine in `src/call.rs` through nine commands (`status`, `open`, `send`, `stop`,
`clear`, `voice`, `add_bot`, `add_group`, `voices`) and one event (`call`, carrying each update
with its chat id). New bots and groups are appended to `team.toml`.

## Success criteria

1. `cargo run -p voice-team` opens the window, with or without keys, and says what's missing.
2. With only the Claude key, each bot answers in its own chat, and in a group chat every member
   answers in turn, reacting to the others.
3. A bot or group created in the app appears in the sidebar at once and is still there after a
   restart.
4. With all three keys, voice on: you talk, the bot answers out loud, and its words show as text.
5. Esc or Stop cuts a bot off, and the message is marked as cut off.
6. With VoiceOver, new messages are read out in text mode and not read twice in voice mode.

## Not yet

Saved chat history between launches, editing or deleting bots, a settings screen for keys, a
bundled `.app` (keys and `team.toml` still come from the repo checkout), Windows.
