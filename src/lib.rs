//! Our own voice SDK: clients for Claude, Deepgram, ElevenLabs and xAI, Mac audio in and
//! out, the turn-taking that makes a call feel like a conversation, and the bots' tools.

pub mod audio;
pub mod call;
pub mod claude;
pub mod llm;
pub mod stt;
pub mod team;
pub mod tools;
pub mod tts;
pub mod turn;
pub mod xai;
