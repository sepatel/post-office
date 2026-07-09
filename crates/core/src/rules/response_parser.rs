use std::str::FromStr;

pub fn parse_llm_response(response: &str) -> Option<ParsedAction> {
    let first_line = response
        .trim()
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?;

    ParsedAction::from_str(first_line).ok()
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
        if s.eq_ignore_ascii_case("SKIP") {
            return Err(ParseError::Skip);
        }

        s.strip_prefix("LABEL:")
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(|name| ParsedAction::Label(name.to_string()))
            .ok_or(ParseError::InvalidFormat)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("LLM response does not match expected format")]
    InvalidFormat,

    #[error("LLM responded with SKIP")]
    Skip,
}
