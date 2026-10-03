use std::sync::LazyLock;

use regex::Regex;

use crate::gmail::models::{Message, MessagePayload};

/// Headers worth showing the model. Everything else — `Received`, `DKIM`,
/// `ARC-*`, authentication results, MIME boundaries — is transport noise that
/// costs tokens without informing a routing decision.
const PROMPT_HEADERS: &[&str] = &[
    "From",
    "To",
    "Cc",
    "Reply-To",
    "Subject",
    "Date",
    "List-Id",
    "List-Unsubscribe",
    "Auto-Submitted",
];

/// The canonical view of a message: the headers and body text a decision is
/// made from. Derived once and shared by the deterministic matcher and the LLM
/// prompt so both see exactly the same content.
#[derive(Debug, Clone)]
pub struct EmailView {
    headers: Vec<(String, String)>,
    pub body: String,
    pub label_ids: Vec<String>,
    pub has_attachments: bool,
    pub attachment_names: Vec<String>,
}

impl EmailView {
    pub fn from_message(message: &Message) -> Self {
        let payload = message.payload.as_ref();
        let mut attachment_names = Vec::new();
        if let Some(payload) = payload {
            collect_attachments(payload, &mut attachment_names);
        }
        let body = payload
            .and_then(body_text)
            .filter(|body| !body.trim().is_empty())
            .unwrap_or_else(|| message.snippet.clone());
        Self {
            headers: prompt_headers(payload),
            body,
            label_ids: message.label_ids.clone(),
            has_attachments: !attachment_names.is_empty(),
            attachment_names,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The header block exactly as it appears in the prompt.
    pub fn rendered_headers(&self) -> String {
        self.headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Picks the body text by walking the MIME tree: a non-attachment `text/plain`
/// leaf is preferred, then a non-attachment `text/html` leaf rendered to text.
/// A one-level scan misses `multipart/alternative` nested inside
/// `multipart/mixed`, which is the common shape once an email has any
/// attachment.
fn body_text(payload: &MessagePayload) -> Option<String> {
    find_leaf(payload, "text/plain")
        .or_else(|| find_leaf(payload, "text/html").map(|html| html_to_text(&html)))
}

fn find_leaf(payload: &MessagePayload, mime: &str) -> Option<String> {
    // A part with a filename is an attachment, whatever its MIME type; a `.txt`
    // attachment must never be mistaken for the message body.
    if has_filename(payload) {
        return None;
    }
    if payload.mime_type.eq_ignore_ascii_case(mime) {
        if let Some(decoded) = payload
            .body
            .as_ref()
            .and_then(|body| body.data.as_deref())
            .map(decode_base64url)
            .filter(|text| !text.trim().is_empty())
        {
            return Some(decoded);
        }
    }
    payload
        .parts
        .as_deref()
        .and_then(|parts| parts.iter().find_map(|part| find_leaf(part, mime)))
}

fn has_filename(payload: &MessagePayload) -> bool {
    payload
        .filename
        .as_deref()
        .is_some_and(|name| !name.is_empty())
}

fn collect_attachments(payload: &MessagePayload, out: &mut Vec<String>) {
    if let Some(name) = payload.filename.as_deref().filter(|name| !name.is_empty()) {
        out.push(name.to_string());
    }
    if let Some(parts) = payload.parts.as_deref() {
        for part in parts {
            collect_attachments(part, out);
        }
    }
}

fn prompt_headers(payload: Option<&MessagePayload>) -> Vec<(String, String)> {
    let Some(payload) = payload else {
        return Vec::new();
    };
    PROMPT_HEADERS
        .iter()
        .filter_map(|wanted| {
            payload
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case(wanted))
                .map(|header| (header.name.clone(), header.value.clone()))
        })
        .collect()
}

fn decode_base64url(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE
        .decode(data)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn html_to_text(html: &str) -> String {
    static DROP_SCRIPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>.*?</script>").unwrap());
    static DROP_STYLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<style\b[^>]*>.*?</style>").unwrap());
    static BREAKS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<(?:br|/p|/div|/tr|/li|/h[1-6]|/table|/blockquote)\b[^>]*>").unwrap()
    });
    static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\f\v]+").unwrap());
    static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n+").unwrap());

    let without_scripts = DROP_SCRIPT.replace_all(html, "");
    let without_styles = DROP_STYLE.replace_all(&without_scripts, "");
    let broken = BREAKS.replace_all(&without_styles, "\n");
    let stripped = TAGS.replace_all(&broken, "");
    let decoded = decode_entities(&stripped);
    let collapsed = SPACES.replace_all(&decoded, " ");
    BLANK_LINES.replace_all(&collapsed, "\n").trim().to_string()
}

fn decode_entities(input: &str) -> String {
    // `&amp;` is decoded last so `&amp;lt;` does not become `<`.
    const NAMED: &[(&str, &str)] = &[
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&nbsp;", " "),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
        ("&hellip;", "…"),
        ("&rsquo;", "’"),
        ("&lsquo;", "‘"),
        ("&ldquo;", "“"),
        ("&rdquo;", "”"),
        ("&amp;", "&"),
    ];
    static NUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&#(x?[0-9A-Fa-f]+);").unwrap());

    let mut out = input.to_string();
    for (from, to) in NAMED {
        out = out.replace(from, to);
    }
    NUMERIC
        .replace_all(&out, |captures: &regex::Captures| {
            let raw = &captures[1];
            let code = raw
                .strip_prefix(['x', 'X'])
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| raw.parse::<u32>().ok());
            code.and_then(char::from_u32)
                .map(String::from)
                .unwrap_or_else(|| captures[0].to_string())
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gmail::models::{Body, Header, Message};

    fn header(name: &str, value: &str) -> Header {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }

    fn payload(mime: &str, headers: Vec<Header>, body: Option<&str>) -> MessagePayload {
        MessagePayload {
            mime_type: mime.into(),
            headers,
            body: body.map(|data| Body {
                size: data.len() as u32,
                data: Some(base64_encode(data)),
            }),
            parts: None,
            filename: None,
        }
    }

    fn base64_encode(data: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE.encode(data)
    }

    fn message(payload: MessagePayload) -> Message {
        Message {
            id: "m1".into(),
            thread_id: "t1".into(),
            label_ids: vec![],
            snippet: "snippet fallback".into(),
            history_id: "1".into(),
            internal_date: "0".into(),
            size_estimate: 0,
            payload: Some(payload),
        }
    }

    #[test]
    fn only_prompt_headers_survive_and_they_keep_their_values() {
        let mut root = payload(
            "multipart/mixed",
            vec![
                header("Received", "from mail.example.com by mx"),
                header("DKIM-Signature", "v=1; a=rsa-sha256; ..."),
                header("From", "billing@example.com"),
                header("Subject", "Your invoice"),
                header("Date", "Mon, 1 Jan 2024 09:00:00 +0000"),
            ],
            None,
        );
        root.parts = Some(vec![payload("text/plain", vec![], Some("Body text"))]);

        let view = EmailView::from_message(&message(root));

        assert_eq!(view.rendered_headers(), "From: billing@example.com\nSubject: Your invoice\nDate: Mon, 1 Jan 2024 09:00:00 +0000");
        assert_eq!(view.body, "Body text");
    }

    #[test]
    fn plain_text_is_preferred_over_html() {
        let mut root = payload("multipart/alternative", vec![], None);
        root.parts = Some(vec![
            payload("text/plain", vec![], Some("Plain version")),
            payload("text/html", vec![], Some("<p>HTML version</p>")),
        ]);

        assert_eq!(
            EmailView::from_message(&message(root)).body,
            "Plain version"
        );
    }

    /// The reported shape: an HTML body lives inside a nested
    /// `multipart/alternative`, which a one-level scan cannot reach, so the old
    /// code fell back to the snippet.
    #[test]
    fn a_nested_html_body_is_reached() {
        let mut alternative = payload("multipart/alternative", vec![], None);
        alternative.parts = Some(vec![payload(
            "text/html",
            vec![],
            Some("<html><body><p>Hello&nbsp;<b>there</b></p></body></html>"),
        )]);
        let mut mixed = payload("multipart/mixed", vec![], None);
        mixed.parts = Some(vec![alternative]);

        assert_eq!(EmailView::from_message(&message(mixed)).body, "Hello there");
    }

    /// A `.txt` attachment is a `text/plain` part; it must not be mistaken for
    /// the body when an HTML body is also present.
    #[test]
    fn a_text_attachment_does_not_become_the_body() {
        let mut attachment = payload("text/plain", vec![], Some("attached notes"));
        attachment.filename = Some("notes.txt".into());
        let mut root = payload("multipart/mixed", vec![], None);
        root.parts = Some(vec![
            payload("text/html", vec![], Some("<p>Real body</p>")),
            attachment,
        ]);

        let view = EmailView::from_message(&message(root));

        assert_eq!(view.body, "Real body");
        assert!(view.has_attachments);
        assert_eq!(view.attachment_names, vec!["notes.txt".to_string()]);
    }

    #[test]
    fn html_is_cleaned_and_entities_decoded() {
        let html = "<style>.a{color:red}</style><script>alert(1)</script>\
                    <div>Hello &amp; welcome &#8212; today</div><br><div>Second line</div>";

        assert_eq!(html_to_text(html), "Hello & welcome — today\nSecond line");
    }

    #[test]
    fn an_empty_body_falls_back_to_the_snippet() {
        let view = EmailView::from_message(&message(payload("text/plain", vec![], None)));

        assert_eq!(view.body, "snippet fallback");
    }
}
