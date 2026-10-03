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

/// Which body-normalization heuristics to apply. Kept as data rather than a
/// hardcoded pipeline so the offline harness can measure each one's effect
/// against the historical decisions before it becomes the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BodyCleanup {
    /// Drop quoted reply history, which describes an earlier message in the
    /// thread rather than the one being routed.
    pub trim_quotes: bool,
    /// Drop a trailing signature block marked with the RFC 3676 `-- `
    /// delimiter. Delimiter-less signatures are left alone: there is no
    /// reliable boundary to cut on.
    pub trim_signature: bool,
}

impl BodyCleanup {
    pub const NONE: Self = Self {
        trim_quotes: false,
        trim_signature: false,
    };

    pub const FULL: Self = Self {
        trim_quotes: true,
        trim_signature: true,
    };
}

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
        Self::from_message_with(message, BodyCleanup::FULL)
    }

    pub fn from_message_with(message: &Message, cleanup: BodyCleanup) -> Self {
        let payload = message.payload.as_ref();
        let mut attachment_names = Vec::new();
        if let Some(payload) = payload {
            collect_attachments(payload, &mut attachment_names);
        }
        let body = match payload
            .and_then(body_text)
            .filter(|body| !body.trim().is_empty())
        {
            Some(body) => clean_body(&body, cleanup),
            // The snippet is already a short, client-generated preview; do not
            // run heuristics that expect a full message on it.
            None => message.snippet.clone(),
        };
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

fn clean_body(body: &str, cleanup: BodyCleanup) -> String {
    // Each step is conservative: if it would remove everything (e.g. a forward
    // whose entire body is quoted history), the earlier body is kept. Some
    // content is always better than none when routing.
    let mut current = body.trim().to_string();
    if cleanup.trim_quotes {
        let trimmed = trim_quoted_reply(&current);
        if !trimmed.trim().is_empty() {
            current = trimmed.trim().to_string();
        }
    }
    if cleanup.trim_signature {
        let trimmed = trim_signature(&current);
        if !trimmed.trim().is_empty() {
            current = trimmed.trim().to_string();
        }
    }
    current
}

/// Cuts a body at the first line that introduces quoted history. The boundary
/// is the earliest of a `>`-quoted line and the common reply separators; a
/// thread's earlier messages say nothing about how the newest one should route.
fn trim_quoted_reply(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = (0..lines.len()).find(|&index| is_reply_boundary(index, &lines)) else {
        return body.to_string();
    };
    lines[..start].join("\n")
}

fn is_reply_boundary(index: usize, lines: &[&str]) -> bool {
    let trimmed = lines[index].trim();
    if trimmed.starts_with('>') {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("-----original message-----")
        || lower.starts_with("__________")
        || (lower.starts_with("on ") && lower.ends_with("wrote:"))
    {
        return true;
    }
    // Outlook replies open with a `From:` / `Sent:` header block.
    lower.starts_with("from:")
        && lines[index..]
            .iter()
            .take(4)
            .any(|candidate| candidate.trim().to_ascii_lowercase().starts_with("sent:"))
}

/// Drops a trailing RFC 3676 signature: the last line that is exactly the
/// `-- ` delimiter and everything below it.
fn trim_signature(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines
        .iter()
        .rposition(|line| matches!(line.trim_end(), "--" | "-- "))
    else {
        return body.to_string();
    };
    lines[..start].join("\n")
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

    fn body_of(text: &str, cleanup: BodyCleanup) -> String {
        let view = EmailView::from_message_with(
            &message(payload("text/plain", vec![], Some(text))),
            cleanup,
        );
        view.body
    }

    #[test]
    fn quoted_reply_history_is_cut() {
        let body = "Please review the estimate.\n\nOn Mon, Jan 1 2024, Alice wrote:\n> can you send it?\n> thanks";

        assert_eq!(
            body_of(body, BodyCleanup::FULL),
            "Please review the estimate."
        );
    }

    #[test]
    fn a_dash_quoted_reply_is_cut() {
        let body = "Sounds good.\n\n> original request\n> more context";

        assert_eq!(body_of(body, BodyCleanup::FULL), "Sounds good.");
    }

    #[test]
    fn an_outlook_header_block_is_cut() {
        let body =
            "Approved.\n\nFrom: Bob\nSent: Tuesday\nTo: Alice\nSubject: Re: thing\n\nold thread";

        assert_eq!(body_of(body, BodyCleanup::FULL), "Approved.");
    }

    #[test]
    fn an_original_message_divider_is_cut() {
        let body = "See below.\n\n-----Original Message-----\nfrom the past";

        assert_eq!(body_of(body, BodyCleanup::FULL), "See below.");
    }

    #[test]
    fn a_trailing_signature_is_cut() {
        let body = "Let me know.\n\n-- \nAlice\nEngineer, Acme";

        assert_eq!(body_of(body, BodyCleanup::FULL), "Let me know.");
    }

    /// A `-- ` in the middle of prose is not a signature delimiter; only a line
    /// that is exactly the delimiter counts.
    #[test]
    fn an_inline_double_dash_is_left_alone() {
        let body = "The value is 5 -- maybe more.\nSecond line.";

        assert_eq!(body_of(body, BodyCleanup::FULL), body);
    }

    #[test]
    fn cleanup_none_leaves_the_body_intact() {
        let body = "Hello.\n\n> quoted\n\n-- \nsig";

        assert_eq!(body_of(body, BodyCleanup::NONE), body);
    }

    /// A forward whose body is nothing but quoted history must keep that history
    /// rather than routing on an empty body.
    #[test]
    fn an_all_quoted_body_is_preserved() {
        let body = "> the only content\n> in this message";

        assert_eq!(body_of(body, BodyCleanup::FULL), body);
    }

    /// A body that is only a signature is likewise preserved.
    #[test]
    fn an_all_signature_body_is_preserved() {
        let body = "-- \nAlice\nEngineer";

        assert_eq!(body_of(body, BodyCleanup::FULL), body);
    }

    /// The snippet is already a short client preview; cleanup must not touch it.
    #[test]
    fn the_snippet_is_never_cleaned() {
        let mut msg = message(payload("text/plain", vec![], None));
        msg.snippet = "Preview ... -- > not a body".into();

        assert_eq!(EmailView::from_message(&msg).body, msg.snippet);
    }
}
