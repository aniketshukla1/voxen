# Spec: team

Module of [CAPABILITY-MAP.md](CAPABILITY-MAP.md). Builds on [SPEC-voice-loop.md](SPEC-voice-loop.md);
the tech stack, commands, structure, style, testing and boundaries there apply here too.
Status: draft, waiting for approval.

## Objective

Two to four named bots on one call, so it feels like talking to a small team of real
people. Each bot has a name, a voice, a speaking speed, a role and a personality.

- Say a bot's name ("Sam, …", "hey Sam") and that bot answers.
- No name: the bot you last talked to answers (the lead, at the start).
- A bot can hand off: "That's Sam's area. Sam?" Then Sam carries on in his own voice.
- Only one voice speaks at a time.
- All bots share one conversation, so each knows what the others said.
- "Team, …" is reserved for `huddle` (a later module). Until then the lead answers.

Hand-offs use a `hand_off` tool that our Claude client offers each bot. The bot speaks
its hand-off line, calls the tool, and the named bot answers next.

## Configuration

`team.toml`. Changing bots never needs a code change.

```toml
lead = "Maya"

[[bot]]
name = "Maya"
voice = "ElevenLabs voice id"
speed = 1.0
role = "Everyday helper and team lead"
personality = "Warm, quick, a little funny"

[[bot]]
name = "Sam"
voice = "ElevenLabs voice id"
speed = 1.1
role = "Researcher: facts, comparisons, numbers"
personality = "Calm and precise"
```

One extra crate: `toml`.

## Project structure (additions)

```
team.toml       your bots
src/team.rs     loads the team, picks who answers, builds each bot's instructions
```

## Testing strategy (additions)

Unit tests for picking who answers (name at the start, "hey Sam", a name mentioned
mid-sentence that is not a call, no name), for hand-off parsing, and for config errors
(duplicate names, unknown lead).

## Success criteria

1. "Sam, what's 15% of 80?" is answered by Sam, in Sam's voice.
2. A follow-up with no name goes to the bot you last talked to.
3. A research question to Maya makes her hand off to Sam, who answers. You never hear
   two voices at once.
4. A bot can refer to what another bot said earlier in the call.
5. Editing `team.toml` (names, voices, speeds, roles) changes the team with no code change.
6. Eyes-closed test: you can tell every bot apart.

## Not in this module

Bots discussing together (`huddle`), doing tasks (`actions`), remembering you between
calls (`memory`).

## Open questions

1. Default names: Maya and Sam, or your own picks?
2. Should the lead greet you when the call starts?
3. A name said mid-sentence ("ask Sam later") is not a call. Which bot should answer then?
