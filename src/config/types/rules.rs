use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn serialize_regex<S: Serializer>(regex: &Option<Regex>, serializer: S) -> Result<S::Ok, S::Error> {
    if let Some(regex) = regex {
        let regex_str = regex.to_string();
        serializer.serialize_str(&regex_str)
    } else {
        serializer.serialize_none()
    }
}

fn deserialize_regex<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Regex>, D::Error> {
    let regex_string = String::deserialize(deserializer)?;
    Regex::new(&regex_string).map(Some).map_err(|err| {
        <D::Error as serde::de::Error>::custom(format!("Invalid regex string! {err}"))
    })
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct WindowRulePattern {
    /// The workspace index the window is getting spawned on.
    #[serde(default)]
    workspace: Option<usize>,

    /// The window title regex to match on
    ///
    /// NOTE: The compositor checks before for a title since it's more specific than an app id.
    #[serde(
        default,
        serialize_with = "serialize_regex",
        deserialize_with = "deserialize_regex"
    )]
    title: Option<Regex>,

    /// The app id regex to match on.
    ///
    /// This is commonly known as the window CLASS, or WM_CLASS on X.org
    #[serde(
        default,
        serialize_with = "serialize_regex",
        deserialize_with = "deserialize_regex"
    )]
    app_id: Option<Regex>,
}

impl std::hash::Hash for WindowRulePattern {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let Some(workspace) = self.workspace {
            state.write_usize(workspace)
        }
        if let Some(title_regex) = &self.title {
            for byte in title_regex.as_str().bytes() {
                state.write_u8(byte)
            }
        }
        if let Some(app_id_regex) = &self.app_id {
            for byte in app_id_regex.as_str().bytes() {
                state.write_u8(byte)
            }
        }
    }
}

impl PartialEq for WindowRulePattern {
    fn eq(&self, other: &Self) -> bool {
        self.workspace == other.workspace
            && regex_matches(self.title.as_ref(), other.title.as_ref())
            && regex_matches(self.app_id.as_ref(), other.app_id.as_ref())
    }
}

impl Eq for WindowRulePattern {}

fn regex_matches(regex_1: Option<&Regex>, regex_2: Option<&Regex>) -> bool {
    regex_1.map(Regex::as_str) == regex_2.map(Regex::as_str)
}

impl WindowRulePattern {
    pub fn matches(&self, title: &str, app_id: &str, workspace: usize) -> bool {
        if self.workspace.as_ref().is_some_and(|ws| workspace == *ws) {
            return true;
        }

        if self
            .title
            .as_ref()
            .is_some_and(|regex| regex.is_match(title))
        {
            return true;
        }

        if self
            .app_id
            .as_ref()
            .is_some_and(|regex| regex.is_match(app_id))
        {
            return true;
        }

        false
    }
}

/// Initial settings/state for a window when mapping it
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowMapSettings {
    /// On which output should we map the window?
    pub output: Option<String>,

    /// The border settings of this window.
    ///
    /// This will override `config.decoration.border` for this window.
    pub border: Option<super::decoration::BorderConfig>,

    /// Whether to allow this window to draw client-side decorations
    pub allow_csd: Option<bool>,

    /// On which specific workspace of the output should we map the window?
    ///
    /// NOTE: This is the workspace *index*
    pub workspace: Option<usize>,
}

impl Default for WindowMapSettings {
    fn default() -> Self {
        Self {
            output: None,
            border: None,
            allow_csd: None,
            workspace: None,
        }
    }
}
