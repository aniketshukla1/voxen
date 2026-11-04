//! The team on the call: who answers, how each bot is told to behave, and hand-offs.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub struct Bot {
    pub name: String,
    /// ElevenLabs voice id.
    pub voice: String,
    #[serde(default = "normal_speed")]
    pub speed: f32,
    pub role: String,
    pub personality: String,
}

fn normal_speed() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct Team {
    lead: String,
    #[serde(rename = "bot")]
    pub bots: Vec<Bot>,
}

/// Words people say before a name ("hey Sam") that don't change who is called.
const FILLERS: [&str; 7] = ["hey", "hi", "hello", "ok", "okay", "so", "and"];

impl Team {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        Self::parse(&text).with_context(|| format!("in {path}"))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let team: Team = toml::from_str(text)?;
        ensure!(!team.bots.is_empty(), "add at least one [[bot]]");
        for (i, bot) in team.bots.iter().enumerate() {
            ensure!(
                !team.bots[..i]
                    .iter()
                    .any(|b| b.name.eq_ignore_ascii_case(&bot.name)),
                "two bots are called {}",
                bot.name
            );
            // ElevenLabs accepts speeds from 0.7 to 1.2.
            ensure!(
                (0.7..=1.2).contains(&bot.speed),
                "{}'s speed must be between 0.7 and 1.2",
                bot.name
            );
        }
        ensure!(
            team.bot(&team.lead).is_some(),
            "lead {} is not one of the bots",
            team.lead
        );
        Ok(team)
    }

    pub fn bot(&self, name: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| b.name.eq_ignore_ascii_case(name))
    }

    pub fn lead(&self) -> &Bot {
        self.bot(&self.lead).unwrap_or(&self.bots[0]) // parse() checked both
    }

    /// Who answers `said`: a bot called by name at the start ("Sam, …", "hey Sam") or at
    /// the end ("…, Sam?"), else whoever spoke last, else the lead. A name in the middle
    /// ("ask Sam later") isn't a call.
    pub fn pick(&self, said: &str, last: Option<&str>) -> &Bot {
        let first = said
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .find(|w| !w.is_empty() && !FILLERS.iter().any(|f| f.eq_ignore_ascii_case(w)));
        let end = said
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .rsplit_once(',')
            .map(|(_, tail)| tail.trim())
            .filter(|tail| !tail.contains(' '));
        first
            .and_then(|w| self.bot(w))
            .or_else(|| end.and_then(|w| self.bot(w)))
            .or_else(|| last.and_then(|n| self.bot(n)))
            .unwrap_or_else(|| self.lead())
    }

    /// The system prompt for `bot`.
    pub fn instructions(&self, bot: &Bot) -> String {
        let mut text = format!(
            "You are {name}, one of the voices on a small team talking with the user on a live \
             voice call. Your role: {role}. Your personality: {personality}.\n\n\
             Everything you write is spoken aloud, so talk like a person on a call: usually one \
             to three short sentences, with no lists, markdown, emoji or links, and never start \
             with your own name. You get the call so far as a transcript with each speaker's \
             name; reply with only what {name} says next.",
            name = bot.name,
            role = bot.role,
            personality = bot.personality,
        );
        let mates: Vec<&Bot> = self.bots.iter().filter(|b| b.name != bot.name).collect();
        if let Some(example) = mates.first() {
            let list: Vec<String> = mates
                .iter()
                .map(|b| format!("{} ({})", b.name, b.role))
                .collect();
            text.push_str(&format!(
                "\n\nYour teammates on the call: {}. If a teammate fits the request better, say \
                 one short hand-over line (for example \"That's one for {n}. {n}?\") and call the \
                 hand_off tool with their name. Otherwise answer yourself.",
                list.join(", "),
                n = example.name,
            ));
        }
        text
    }

    /// The hand_off tool for `bot`, or nothing when it's alone on the call.
    pub fn tools(&self, bot: &Bot) -> Vec<Value> {
        let mates: Vec<&str> = self
            .bots
            .iter()
            .filter(|b| b.name != bot.name)
            .map(|b| b.name.as_str())
            .collect();
        if mates.is_empty() {
            return Vec::new();
        }
        vec![json!({
            "name": "hand_off",
            "description": "Pass the user to a teammate who fits the request better. Say your short hand-over line first.",
            "eager_input_streaming": true,
            "input_schema": {
                "type": "object",
                "properties": {"to": {"type": "string", "enum": mates}},
                "required": ["to"],
            },
        })]
    }

    /// The teammate a hand_off call names, if it's someone else on the team.
    pub fn hand_off_to(&self, from: &str, input: &Value) -> Option<&Bot> {
        self.bot(input["to"].as_str()?)
            .filter(|b| !b.name.eq_ignore_ascii_case(from))
    }
}

/// The call so far, one "Name: words" line per turn, for the next speaker.
// ponytail: the whole call is resent every turn; trim it when calls get long.
pub fn transcript(lines: &[(String, String)]) -> String {
    lines
        .iter()
        .map(|(who, said)| format!("{who}: {said}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = r#"
lead = "Maya"

[[bot]]
name = "Maya"
voice = "v1"
role = "Everyday helper and team lead"
personality = "Warm"

[[bot]]
name = "Sam"
voice = "v2"
speed = 1.1
role = "Researcher"
personality = "Calm"
"#;

    #[test]
    fn who_answers() {
        let team = Team::parse(TWO).unwrap();
        let pick = |said: &str, last: Option<&str>| team.pick(said, last).name.clone();
        assert_eq!(pick("Sam, what's 15% of 80?", None), "Sam");
        assert_eq!(pick("hey sam what's up", Some("Maya")), "Sam");
        assert_eq!(pick("What do you think, Sam?", Some("Maya")), "Sam");
        assert_eq!(pick("And its population?", Some("Sam")), "Sam");
        assert_eq!(pick("Ask Sam about it later.", Some("Maya")), "Maya");
        assert_eq!(pick("Hello there", None), "Maya");
        assert_eq!(pick("Samuel called", None), "Maya");
    }

    #[test]
    fn hand_offs_only_go_to_someone_else() {
        let team = Team::parse(TWO).unwrap();
        assert_eq!(
            team.hand_off_to("Maya", &json!({"to": "Sam"}))
                .map(|b| b.name.as_str()),
            Some("Sam")
        );
        assert!(team.hand_off_to("Sam", &json!({"to": "Sam"})).is_none());
        assert!(team.hand_off_to("Maya", &json!({"to": "Zed"})).is_none());
        assert!(team.hand_off_to("Maya", &json!({})).is_none());
    }

    #[test]
    fn each_bot_knows_its_teammates() {
        let team = Team::parse(TWO).unwrap();
        let maya = team.bot("maya").unwrap();
        assert!(team.instructions(maya).contains("Sam (Researcher)"));
        assert_eq!(
            team.tools(maya)[0]["input_schema"]["properties"]["to"]["enum"],
            json!(["Sam"])
        );
        assert_eq!(team.bot("Sam").unwrap().speed, 1.1);
        assert_eq!(maya.speed, 1.0);
    }

    #[test]
    fn a_solo_bot_has_no_hand_off() {
        let solo = Team::parse("lead = \"Maya\"\n[[bot]]\nname = \"Maya\"\nvoice = \"v\"\nrole = \"r\"\npersonality = \"p\"\n").unwrap();
        assert!(solo.tools(solo.lead()).is_empty());
        assert!(!solo.instructions(solo.lead()).contains("teammates"));
    }

    #[test]
    fn bad_teams_are_refused() {
        let dup = TWO.replace("name = \"Sam\"", "name = \"maya\"");
        assert!(
            Team::parse(&dup)
                .unwrap_err()
                .to_string()
                .contains("two bots")
        );
        let no_lead = TWO.replace("lead = \"Maya\"", "lead = \"Zed\"");
        assert!(
            Team::parse(&no_lead)
                .unwrap_err()
                .to_string()
                .contains("lead Zed")
        );
        let fast = TWO.replace("speed = 1.1", "speed = 3.0");
        assert!(
            Team::parse(&fast)
                .unwrap_err()
                .to_string()
                .contains("speed")
        );
    }

    #[test]
    fn transcript_has_one_line_per_turn() {
        let lines = vec![
            ("You".to_string(), "Hi".to_string()),
            ("Maya".to_string(), "Hello!".to_string()),
        ];
        assert_eq!(transcript(&lines), "You: Hi\nMaya: Hello!");
    }
}
