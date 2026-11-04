//! Turn-taking: when you've finished talking, when you've cut in, and where sentences end.

use crate::stt::Heard;

/// What the app should do after hearing something.
#[derive(Debug, PartialEq)]
pub enum Action {
    Wait,
    /// Your turn is over; reply to this.
    Reply(String),
    /// You started talking over the bot; stop it.
    Interrupt,
}

/// Collects what you say until your turn ends.
#[derive(Default)]
pub struct Turns {
    said: String,
}

impl Turns {
    /// `bot_talking` is true while a reply is being written or played.
    pub fn on_heard(&mut self, heard: Heard, bot_talking: bool) -> Action {
        match heard {
            Heard::SpeechStarted { .. } | Heard::Partial(_) if bot_talking => Action::Interrupt,
            Heard::Final {
                text, end_of_turn, ..
            } => {
                if !text.is_empty() {
                    if !self.said.is_empty() {
                        self.said.push(' ');
                    }
                    self.said.push_str(&text);
                }
                if end_of_turn {
                    self.finish()
                } else if bot_talking && !text.is_empty() {
                    Action::Interrupt
                } else {
                    Action::Wait
                }
            }
            Heard::UtteranceEnd => self.finish(),
            Heard::SpeechStarted { .. } | Heard::Partial(_) => Action::Wait,
        }
    }

    fn finish(&mut self) -> Action {
        let said = std::mem::take(&mut self.said);
        if said.is_empty() {
            Action::Wait
        } else {
            Action::Reply(said)
        }
    }
}

/// Pops the next complete sentence off a streaming buffer, so speech can start
/// before Claude finishes. A sentence ends at . ! ? or a newline followed by
/// whitespace, so "3.5" stays whole.
// ponytail: "Dr. Smith" splits after "Dr."; add an abbreviation list if it shows up in speech.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn final_words(text: &str, end_of_turn: bool) -> Heard {
        Heard::Final {
            text: text.into(),
            end_of_turn,
            end: 0.0,
        }
    }

    #[test]
    fn words_collect_until_the_pause() {
        let mut turns = Turns::default();
        assert_eq!(
            turns.on_heard(final_words("What's the capital", false), false),
            Action::Wait
        );
        assert_eq!(
            turns.on_heard(final_words("of Japan?", true), false),
            Action::Reply("What's the capital of Japan?".into())
        );
        assert_eq!(turns.on_heard(Heard::UtteranceEnd, false), Action::Wait);
    }

    #[test]
    fn utterance_end_is_the_backup_signal() {
        let mut turns = Turns::default();
        turns.on_heard(final_words("Hello", false), false);
        assert_eq!(
            turns.on_heard(Heard::UtteranceEnd, false),
            Action::Reply("Hello".into())
        );
    }

    #[test]
    fn talking_over_the_bot_interrupts_it() {
        let mut turns = Turns::default();
        assert_eq!(
            turns.on_heard(Heard::SpeechStarted { at: 1.0 }, true),
            Action::Interrupt
        );
        assert_eq!(
            turns.on_heard(Heard::Partial("wait".into()), true),
            Action::Interrupt
        );
        assert_eq!(
            turns.on_heard(final_words("wait", false), true),
            Action::Interrupt
        );
        assert_eq!(
            turns.on_heard(Heard::SpeechStarted { at: 1.0 }, false),
            Action::Wait
        );
        assert_eq!(
            turns.on_heard(final_words("", true), true),
            Action::Reply("wait".into())
        );
    }

    #[test]
    fn sentences_come_out_as_soon_as_they_end() {
        let mut buf = String::from("It costs 3.5 dollars. Want it?\nOK");
        assert_eq!(
            take_sentence(&mut buf).as_deref(),
            Some("It costs 3.5 dollars.")
        );
        assert_eq!(take_sentence(&mut buf).as_deref(), Some("Want it?"));
        assert_eq!(take_sentence(&mut buf), None);
        assert_eq!(buf.trim(), "OK");
    }

    #[test]
    fn a_full_stop_at_the_very_end_waits_for_more_text() {
        let mut buf = String::from("\n\nTokyo.");
        assert_eq!(take_sentence(&mut buf), None);
        buf.push_str(" Big city.");
        assert_eq!(take_sentence(&mut buf).as_deref(), Some("Tokyo."));
    }
}
