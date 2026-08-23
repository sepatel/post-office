use std::str::FromStr;

use super::models::Action;

/// Structured rule actions are authored as `Action` but executed as `ParsedAction`;
/// the conversion lets the engine run either path through the same executor.
impl From<&Action> for ParsedAction {
    fn from(action: &Action) -> Self {
        match action {
            Action::Label { value } => ParsedAction::Label(value.clone()),
            Action::RemoveLabel { value } => ParsedAction::RemoveLabel(value.clone()),
            Action::Archive => ParsedAction::Archive,
            Action::Trash => ParsedAction::Trash,
            Action::Spam => ParsedAction::Spam,
            Action::MarkRead => ParsedAction::MarkRead,
            Action::MarkUnread => ParsedAction::MarkUnread,
            Action::Star => ParsedAction::Star,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedAction {
    Archive,
    Trash,
    Spam,
    MarkRead,
    MarkUnread,
    Star,
    Label(String),
    RemoveLabel(String),
}

impl FromStr for ParsedAction {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s.eq_ignore_ascii_case("ARCHIVE") {
            return Ok(ParsedAction::Archive);
        }
        if s.eq_ignore_ascii_case("TRASH") {
            return Ok(ParsedAction::Trash);
        }
        if s.eq_ignore_ascii_case("SPAM") {
            return Ok(ParsedAction::Spam);
        }
        if s.eq_ignore_ascii_case("MARK_READ") {
            return Ok(ParsedAction::MarkRead);
        }
        if s.eq_ignore_ascii_case("MARK_UNREAD") {
            return Ok(ParsedAction::MarkUnread);
        }
        if s.eq_ignore_ascii_case("STAR") {
            return Ok(ParsedAction::Star);
        }

        s.strip_prefix("LABEL:")
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(|name| ParsedAction::Label(name.to_string()))
            .or_else(|| {
                s.strip_prefix("REMOVE_LABEL:")
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
                    .map(|name| ParsedAction::RemoveLabel(name.to_string()))
            })
            .ok_or(ParseError::InvalidFormat)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("LLM response does not match expected format")]
    InvalidFormat,
}
