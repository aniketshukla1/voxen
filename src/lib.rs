//! Our own voice SDK: clients for Claude, Deepgram and ElevenLabs, Mac audio in and
//! out, and the turn-taking that makes a call feel like a conversation.

pub mod audio;
pub mod call;
pub mod claude;
pub mod stt;
pub mod team;
pub mod tts;
pub mod turn;
