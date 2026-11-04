//! The team: your main bot, the bots you've made, your group chats, and how each bot is
//! told to behave.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Bot {
    pub name: String,
    /// What it's for, in a few words.
    pub role: String,
    pub personality: String,
    /// ElevenLabs voice id.
    pub voice: String,
    #[serde(default = "normal_speed")]
    pub speed: f32,
    /// The bot's colour in the app, as "#RRGGBB". Picked from the palette when missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A group chat: you and two or more bots, who take turns and react to each other.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Group {
    pub name: String,
    pub bots: Vec<String>,
}

fn normal_speed() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct Team {
    /// The bot you talk to by default.
    #[serde(alias = "lead")]
    pub main: String,
    #[serde(rename = "bot")]
    pub bots: Vec<Bot>,
    #[serde(rename = "group", default)]
    pub groups: Vec<Group>,
    /// Where new bots and groups are saved.
    #[serde(skip)]
    path: Option<PathBuf>,
}

/// Words people say before a name ("hey Sam") that don't change who is called.
const FILLERS: [&str; 7] = ["hey", "hi", "hello", "ok", "okay", "so", "and"];

impl Team {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut team = Self::parse(&text).with_context(|| format!("in {}", path.display()))?;
        team.path = Some(path.to_path_buf());
        Ok(team)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let team: Team = toml::from_str(text)?;
        ensure!(!team.bots.is_empty(), "add at least one [[bot]]");
        for (i, bot) in team.bots.iter().enumerate() {
            check_bot(bot)?;
            ensure!(
                !team.bots[..i].iter().any(|b| same(&b.name, &bot.name)),
                "two bots are called {}",
                bot.name
            );
        }
        ensure!(
            team.bot(&team.main).is_some(),
            "the main bot {} is not one of the bots",
            team.main
        );
        for group in &team.groups {
            team.check_group(group)?;
        }
        Ok(team)
    }

    pub fn names(&self) -> Vec<String> {
        self.bots.iter().map(|b| b.name.clone()).collect()
    }

    pub fn bot(&self, name: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| same(&b.name, name))
    }

    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| same(&g.name, name))
    }

    pub fn main_bot(&self) -> &Bot {
        self.bot(&self.main).unwrap_or(&self.bots[0]) // parse() checked both
    }

    /// Adds a bot and saves it to the team file.
    pub fn add_bot(&mut self, bot: Bot) -> Result<()> {
        check_bot(&bot)?;
        self.check_name_free(&bot.name)?;
        self.save("bot", &bot)?;
        self.bots.push(bot);
        Ok(())
    }

    /// Adds a group chat and saves it to the team file.
    pub fn add_group(&mut self, group: Group) -> Result<()> {
        ensure!(!group.name.trim().is_empty(), "give the group a name");
        self.check_name_free(&group.name)?;
        self.check_group(&group)?;
        self.save("group", &group)?;
        self.groups.push(group);
        Ok(())
    }

    fn check_name_free(&self, name: &str) -> Result<()> {
        ensure!(
            self.bot(name).is_none() && self.group(name).is_none(),
            "the name {name} is already taken"
        );
        Ok(())
    }

    fn check_group(&self, group: &Group) -> Result<()> {
        ensure!(
            group.bots.len() >= 2,
            "the group {} needs at least two bots",
            group.name
        );
        for name in &group.bots {
            ensure!(
                self.bot(name).is_some(),
                "the group {} lists {name}, who isn't one of your bots",
                group.name
            );
        }
        Ok(())
    }

    /// Appends one entry to the team file, keeping the file's comments.
    fn save(&self, table: &str, entry: &impl Serialize) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("saving to {}", path.display()))?;
        write!(file, "\n[[{table}]]\n{}", toml::to_string(entry)?)?;
        Ok(())
    }

    /// Who answers `said` among `members`: one called by name at the start ("Sam, …",
    /// "hey Sam") or at the end ("…, Sam?"), else whoever spoke last, else the first member.
    /// A name in the middle ("ask Sam later") isn't a call.
    pub fn pick(&self, said: &str, last: Option<&str>, members: &[String]) -> &Bot {
        let member = |name: &str| {
            members
                .iter()
                .any(|m| same(m, name))
                .then(|| self.bot(name))
                .flatten()
        };
        let first = said
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .find(|w| !w.is_empty() && !FILLERS.iter().any(|f| same(f, w)));
        let end = said
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .rsplit_once(',')
            .map(|(_, tail)| tail.trim())
            .filter(|tail| !tail.contains(' '));
        first
            .and_then(member)
            .or_else(|| end.and_then(member))
            .or_else(|| last.and_then(member))
            .or_else(|| members.first().and_then(|m| self.bot(m)))
            .unwrap_or_else(|| self.main_bot())
    }

    /// The system prompt for `bot`, chatting with the user alone or with `others` in a group.
    /// A `spoken` reply is read aloud, so it stays short and plain.
    pub fn instructions(&self, bot: &Bot, others: &[&Bot], spoken: bool) -> String {
        let mut text = format!(
            "You are {}. What you're for: {}. Your personality: {}.",
            bot.name, bot.role, bot.personality
        );
        if others.is_empty() {
            text.push_str(" You're chatting one-on-one with the user.");
        } else {
            let list: Vec<String> = others
                .iter()
                .map(|b| format!("{} ({})", b.name, b.role))
                .collect();
            text.push_str(&format!(
                " You're in a group chat with the user and {}. The bots take turns: react to what \
                 the others just said, add something new or disagree, and don't repeat them or \
                 speak for them. Keep it to one to three sentences.",
                list.join(", ")
            ));
        }
        text.push_str(if spoken {
            "\n\nEverything you write is spoken aloud, so talk like a person on a call: short \
             sentences, with no lists, markdown, emoji or links."
        } else {
            "\n\nWrite like a chat message: clear and to the point, in plain text with short \
             paragraphs and no markdown symbols."
        });
        text.push_str(
            " Never start with your own name. You get the conversation so far as a transcript \
             with each speaker's name; reply with only what you say next.",
        );
        text
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn check_bot(bot: &Bot) -> Result<()> {
    let name = bot.name.trim();
    ensure!(!name.is_empty(), "give the bot a name");
    ensure!(
        !same(name, "you"),
        "\"You\" is how you appear in chats; pick another name"
    );
    ensure!(!bot.voice.trim().is_empty(), "{name} needs a voice");
    // ElevenLabs accepts speeds from 0.7 to 1.2.
    ensure!(
        (0.7..=1.2).contains(&bot.speed),
        "{name}'s speed must be between 0.7 and 1.2"
    );
    if let Some(color) = &bot.color {
        let hex = color.strip_prefix('#').unwrap_or_default();
        ensure!(
            hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()),
            "{name}'s colour must look like #8B7CFF"
        );
    }
    Ok(())
}

/// The conversation so far, one "Name: words" line per turn, for the next speaker.
// ponytail: the whole chat is resent every turn; trim it when chats get long.
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

    const TEAM: &str = r#"
main = "Maya"

[[bot]]
name = "Maya"
role = "Everyday helper"
personality = "Warm"
voice = "v1"

[[bot]]
name = "Sam"
role = "Researcher"
personality = "Calm"
voice = "v2"
speed = 1.1

[[bot]]
name = "Leo"
role = "Writer"
personality = "Playful"
voice = "v3"

[[group]]
name = "Brain trust"
bots = ["Maya", "Sam"]
"#;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn who_answers() {
        let team = Team::parse(TEAM).unwrap();
        let all = team.names();
        let pick = |said: &str, last: Option<&str>| team.pick(said, last, &all).name.clone();
        assert_eq!(pick("Sam, what's 15% of 80?", None), "Sam");
        assert_eq!(pick("hey sam what's up", Some("Maya")), "Sam");
        assert_eq!(pick("What do you think, Sam?", Some("Maya")), "Sam");
        assert_eq!(pick("And its population?", Some("Sam")), "Sam");
        assert_eq!(pick("Ask Sam about it later.", Some("Maya")), "Maya");
        assert_eq!(pick("Hello there", None), "Maya");
        assert_eq!(pick("Samuel called", None), "Maya");
    }

    #[test]
    fn only_members_answer() {
        let team = Team::parse(TEAM).unwrap();
        let group = names(&["Sam", "Leo"]);
        assert_eq!(team.pick("Maya, you there?", None, &group).name, "Sam");
        assert_eq!(team.pick("Leo, write a haiku", None, &group).name, "Leo");
    }

    #[test]
    fn prompts_fit_the_chat() {
        let team = Team::parse(TEAM).unwrap();
        let (maya, sam) = (team.bot("maya").unwrap(), team.bot("Sam").unwrap());
        let alone = team.instructions(maya, &[], false);
        assert!(alone.contains("one-on-one") && alone.contains("plain text"));
        let group = team.instructions(maya, &[sam], true);
        assert!(group.contains("group chat with the user and Sam (Researcher)"));
        assert!(group.contains("spoken aloud"));
        assert_eq!(sam.speed, 1.1);
        assert_eq!(maya.speed, 1.0);
    }

    #[test]
    fn new_bots_and_groups_are_saved() {
        let path =
            std::env::temp_dir().join(format!("voice-team-test-{}.toml", std::process::id()));
        std::fs::write(&path, TEAM).unwrap();
        let mut team = Team::load(&path).unwrap();
        let ava = Bot {
            name: "Ava".into(),
            role: "Travel planner".into(),
            personality: "Upbeat".into(),
            voice: "v4".into(),
            speed: 1.0,
            color: Some("#FF8A65".into()),
        };
        team.add_bot(ava.clone()).unwrap();
        let bad_color = Bot {
            name: "Rio".into(),
            color: Some("orange".into()),
            ..ava.clone()
        };
        assert!(
            team.add_bot(bad_color)
                .unwrap_err()
                .to_string()
                .contains("colour")
        );
        team.add_group(Group {
            name: "Trip".into(),
            bots: names(&["Ava", "Leo"]),
        })
        .unwrap();
        let taken = team
            .add_bot(Bot {
                name: "ava".into(),
                ..ava
            })
            .unwrap_err();
        let again = Team::load(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(taken.to_string().contains("taken"));
        assert_eq!(again.bot("Ava").unwrap().role, "Travel planner");
        assert_eq!(again.bot("Ava").unwrap().color.as_deref(), Some("#FF8A65"));
        assert_eq!(again.group("trip").unwrap().bots, names(&["Ava", "Leo"]));
    }

    #[test]
    fn bad_teams_are_refused() {
        let error = |text: String| Team::parse(&text).unwrap_err().to_string();
        assert!(error(TEAM.replace("name = \"Sam\"", "name = \"maya\"")).contains("two bots"));
        assert!(error(TEAM.replace("main = \"Maya\"", "main = \"Zed\"")).contains("main bot Zed"));
        assert!(error(TEAM.replace("speed = 1.1", "speed = 3.0")).contains("speed"));
        assert!(
            error(TEAM.replace("[\"Maya\", \"Sam\"]", "[\"Maya\", \"Zed\"]"))
                .contains("Zed, who isn't")
        );
        assert!(
            error(TEAM.replace("name = \"Leo\"", "name = \"You\"")).contains("pick another name")
        );
        let old_style = Team::parse(&TEAM.replace("main = ", "lead = ")).unwrap();
        assert_eq!(old_style.main_bot().name, "Maya");
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
