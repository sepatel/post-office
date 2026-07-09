use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Condition {
    #[serde(rename = "from")]
    From {
        operator: Operator,
        value: String,
    },

    #[serde(rename = "to")]
    To {
        operator: Operator,
        value: String,
    },

    #[serde(rename = "subject")]
    Subject {
        operator: Operator,
        value: String,
    },

    #[serde(rename = "body")]
    Body {
        operator: Operator,
        value: String,
    },

    #[serde(rename = "has_attachment")]
    HasAttachment { value: bool },

    #[serde(rename = "is_unread")]
    IsUnread { value: bool },

    #[serde(rename = "label")]
    Label {
        operator: Operator,
        value: String,
    },

    #[serde(rename = "date_after")]
    DateAfter { value: String },

    #[serde(rename = "date_before")]
    DateBefore { value: String },

    #[serde(rename = "and")]
    And { conditions: Vec<Condition> },

    #[serde(rename = "or")]
    Or { conditions: Vec<Condition> },

    #[serde(rename = "not")]
    Not {
        condition: Box<Condition>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Contains,
    Equals,
    Regex,
    NotContains,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    #[serde(rename = "label")]
    Label { value: String },

    #[serde(rename = "archive")]
    Archive,

    #[serde(rename = "trash")]
    Trash,

    #[serde(rename = "spam")]
    Spam,

    #[serde(rename = "mark_read")]
    MarkRead,

    #[serde(rename = "mark_unread")]
    MarkUnread,

    #[serde(rename = "star")]
    Star,
}
