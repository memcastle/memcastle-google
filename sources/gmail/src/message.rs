//! Deterministic extraction; normalization consumes only the acquired text/metadata.
use crate::memcastle::source::types::{Segment, SourceKind};
use crate::reconciliation::{Acquisition, Decision, KnownDocument};
use crate::{
    CanonicalDocument, RawDocument, SourceError, SourceRef,
    api::{self, Error, Gmail},
    digest, failed,
};
use base64::{
    Engine,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const VERSION: u32 = 1;
const PART_BYTES: usize = 1024 * 1024;
const TOTAL_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    #[serde(default)]
    size: u64,
    data: Option<String>,
    attachment_id: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(default)]
    part_id: String,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    headers: Vec<Header>,
    #[serde(default)]
    body: Body,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    thread_id: String,
    history_id: String,
    internal_date: String,
    #[serde(default)]
    label_ids: Vec<String>,
    payload: Part,
}

#[derive(Serialize, Deserialize)]
struct Attachment {
    filename: String,
    mime_type: String,
    part_id: String,
    attachment_id: Option<String>,
    size: u64,
    status: String,
}

#[derive(Serialize, Deserialize)]
struct Metadata {
    extraction_version: u32,
    mailbox: String,
    message_id: String,
    thread_id: String,
    history_id: String,
    internal_date: String,
    headers: BTreeMap<String, String>,
    labels: Vec<String>,
    attachments: Vec<Attachment>,
}

fn get_message(api: &mut impl Gmail, id: &str, format: &str) -> Result<Option<Value>, SourceError> {
    let path = format!("/messages/{}", api::id(id)?);
    match api.get(&path, &[("format", format.into())]) {
        Ok(value) => {
            if value.get("id").and_then(Value::as_str) != Some(id) {
                return Err(failed("Gmail response does not match requested message"));
            }
            Ok(Some(value))
        }
        // Callers have just checked the authenticated profile. A specific object's
        // 404 is deletion evidence, unlike a search omission or history 404.
        Err(Error::Status(404)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn reconcile(api: &mut impl Gmail, known: &KnownDocument) -> Result<Decision, SourceError> {
    let Some(value) = get_message(api, &known.external_id, "minimal")? else {
        return Ok(Decision::Retire("Gmail message no longer exists".into()));
    };
    let history = value
        .get("historyId")
        .and_then(Value::as_str)
        .ok_or_else(|| failed("Gmail message omitted historyId"))?;
    api::decimal(history)?;
    let metadata = serde_json::from_str::<Metadata>(&known.metadata).ok();
    if known.pending
        || known.retired
        || metadata
            .as_ref()
            .is_none_or(|m| m.extraction_version != VERSION || m.history_id != history)
    {
        Ok(Decision::Refresh(known.external_id.clone()))
    } else {
        Ok(Decision::Unchanged)
    }
}

pub fn acquire(
    api: &mut impl Gmail,
    source: &SourceRef,
    id: &str,
) -> Result<Acquisition, SourceError> {
    let Some(value) = get_message(api, id, "full")? else {
        return Ok(Acquisition::Retire("Gmail message no longer exists".into()));
    };
    let mut message: Message =
        serde_json::from_value(value).map_err(|_| failed("invalid full Gmail message"))?;
    api::decimal(&message.history_id)?;
    api::id(&message.thread_id)?;
    let timestamp: i64 = message
        .internal_date
        .parse()
        .map_err(|_| failed("invalid Gmail internal date"))?;
    let occurred_at = chrono::DateTime::from_timestamp_millis(timestamp)
        .ok_or_else(|| failed("Gmail date out of range"))?
        .to_rfc3339();
    let mut headers = BTreeMap::new();
    for header in &message.payload.headers {
        let name = header.name.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "from"
                | "to"
                | "cc"
                | "bcc"
                | "reply-to"
                | "subject"
                | "date"
                | "message-id"
                | "in-reply-to"
                | "references"
        ) {
            let entry = headers.entry(name).or_insert_with(String::new);
            if !entry.is_empty() {
                entry.push_str(", ");
            }
            entry.push_str(&decode_header(&header.value));
        }
    }
    let mut extraction = Extraction {
        api,
        message_id: id,
        segments: vec![],
        attachments: vec![],
        parts: 0,
        fetches: 0,
        bytes: 0,
        bodies: 0,
    };
    extraction.walk(&message.payload, 0, false)?;
    message.label_ids.sort();
    message.label_ids.dedup();
    let metadata = Metadata {
        extraction_version: VERSION,
        mailbox: source
            .account
            .clone()
            .ok_or_else(|| failed("Gmail mailbox missing"))?,
        message_id: message.id,
        thread_id: message.thread_id,
        history_id: message.history_id,
        internal_date: message.internal_date,
        headers,
        labels: message.label_ids,
        attachments: extraction.attachments,
    };
    let body = serde_json::to_string(&extraction.segments)
        .map_err(|_| failed("cannot encode Gmail message text"))?;
    let mut revision_metadata =
        serde_json::to_value(&metadata).map_err(|_| failed("cannot encode Gmail metadata"))?;
    revision_metadata
        .as_object_mut()
        .expect("metadata object")
        .remove("history_id");
    let revision =
        digest(&serde_json::to_vec(&(&body, revision_metadata)).expect("serializable revision"));
    Ok(Acquisition::Document(RawDocument {
        external_id: id.into(),
        revision,
        body,
        metadata: serde_json::to_string(&metadata).expect("serializable metadata"),
        occurred_at: Some(occurred_at),
    }))
}

fn decode_header(value: &str) -> String {
    let wire = format!("X: {value}\r\n");
    mailparse::parse_header(wire.as_bytes())
        .map(|(h, _)| h.get_value())
        .unwrap_or_else(|_| value.to_string())
}

struct Extraction<'a, A> {
    api: &'a mut A,
    message_id: &'a str,
    segments: Vec<String>,
    attachments: Vec<Attachment>,
    parts: usize,
    fetches: usize,
    bytes: usize,
    bodies: usize,
}

fn attached(part: &Part) -> bool {
    !part.filename.is_empty()
        || part.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("content-disposition")
                && h.value
                    .split(';')
                    .next()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("attachment"))
        })
}

fn plain_body(part: &Part) -> bool {
    !attached(part)
        && ((part.mime_type.eq_ignore_ascii_case("text/plain")
            && (part.body.data.as_ref().is_some_and(|s| !s.is_empty())
                || part.body.attachment_id.is_some()))
            || part.parts.iter().any(plain_body))
}

fn html_body(part: &Part) -> bool {
    !attached(part)
        && (part.mime_type.eq_ignore_ascii_case("text/html") || part.parts.iter().any(html_body))
}

impl<A: Gmail> Extraction<'_, A> {
    fn walk(
        &mut self,
        part: &Part,
        depth: usize,
        attachments_only: bool,
    ) -> Result<(), SourceError> {
        if self.parts >= 256 {
            return Ok(());
        }
        self.parts += 1;
        if depth > 20 || self.parts == 256 {
            self.record(part, "mime_limit", None);
            return Ok(());
        }
        let mime = part.mime_type.to_ascii_lowercase();
        if mime.starts_with("multipart/") {
            let preferred = if mime == "multipart/alternative" {
                part.parts
                    .iter()
                    .position(plain_body)
                    .or_else(|| part.parts.iter().position(html_body))
                    .or_else(|| part.parts.iter().position(|p| !attached(p)))
            } else {
                None
            };
            if let Some(preferred) = preferred {
                let before = self.bodies;
                self.walk(&part.parts[preferred], depth + 1, attachments_only)?;
                let fallback = if !attachments_only && self.bodies == before {
                    part.parts
                        .iter()
                        .enumerate()
                        .find(|(i, p)| *i != preferred && html_body(p))
                        .map(|(i, _)| i)
                } else {
                    None
                };
                if let Some(fallback) = fallback {
                    self.walk(&part.parts[fallback], depth + 1, false)?;
                }
                for (i, child) in part.parts.iter().enumerate() {
                    if i != preferred && Some(i) != fallback {
                        self.walk(child, depth + 1, true)?;
                    }
                }
            } else {
                for child in &part.parts {
                    self.walk(child, depth + 1, attachments_only)?;
                }
            }
            return Ok(());
        }
        let attachment = attached(part);
        if attachments_only && !attachment {
            return Ok(());
        }
        let extension = part
            .filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let text = matches!(
            mime.as_str(),
            "text/plain"
                | "text/html"
                | "text/markdown"
                | "text/vtt"
                | "application/x-subrip"
                | "text/srt"
        ) || (matches!(mime.as_str(), "application/octet-stream" | "")
            && matches!(
                extension.as_str(),
                "txt" | "md" | "html" | "htm" | "vtt" | "srt"
            ));
        if !text {
            self.record(part, "unsupported", None);
            return Ok(());
        }
        if part.body.size > PART_BYTES as u64 || self.bytes >= TOTAL_BYTES {
            self.record(part, "size_limit", None);
            return Ok(());
        }
        let encoded = if let Some(data) = &part.body.data {
            data.clone()
        } else if let Some(id) = &part.body.attachment_id {
            if self.fetches >= 8 {
                self.record(part, "fetch_limit", None);
                return Ok(());
            }
            self.fetches += 1;
            let path = format!(
                "/messages/{}/attachments/{}",
                api::id(self.message_id)?,
                api::attachment_id(id)?
            );
            let value = self.api.get(&path, &[])?;
            value
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| failed("Gmail attachment response omitted data"))?
                .to_string()
        } else if part.body.size == 0 {
            String::new()
        } else {
            return Err(failed("Gmail text part omitted content"));
        };
        if encoded.len() > PART_BYTES.div_ceil(3) * 4 + 4 {
            self.record(part, "size_limit", None);
            return Ok(());
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&encoded)
            .or_else(|_| URL_SAFE.decode(&encoded))
            .map_err(|_| failed("malformed Gmail base64url text"))?;
        if bytes.len() > PART_BYTES {
            self.record(part, "size_limit", None);
            return Ok(());
        }
        let content_type = part
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.as_str())
            .unwrap_or("");
        let parsed = mailparse::parse_content_type(content_type);
        let charset = parsed
            .params
            .get("charset")
            .map(String::as_str)
            .unwrap_or("utf-8");
        let Some(encoding) = encoding_rs::Encoding::for_label(charset.as_bytes()) else {
            self.record(part, "unsupported_charset", None);
            return Ok(());
        };
        let (decoded, _, malformed) = encoding.decode(&bytes);
        if malformed {
            return Err(failed(
                "Gmail text does not match its declared character set",
            ));
        }
        let html = mime == "text/html" || matches!(extension.as_str(), "html" | "htm");
        let text = if html {
            html2text::from_read(decoded.as_bytes(), 100)
                .map_err(|_| failed("cannot convert Gmail HTML to text"))?
        } else {
            decoded.into_owned()
        };
        if self.bytes + text.len() > TOTAL_BYTES {
            self.record(part, "size_limit", None);
            return Ok(());
        }
        self.bytes += text.len();
        if attachment {
            self.record(part, "extracted", Some(text));
        } else if !text.trim().is_empty() {
            self.bodies += 1;
            self.segments.push(text);
        }
        Ok(())
    }

    fn record(&mut self, part: &Part, status: &str, text: Option<String>) {
        let name = decode_header(&part.filename);
        self.segments.push(match text {
            Some(text) => format!("Attachment: {name} ({})\n{text}", part.mime_type),
            None => format!(
                "Attachment/part: {name} ({}, {} bytes; {status})",
                part.mime_type, part.body.size
            ),
        });
        self.attachments.push(Attachment {
            filename: name,
            mime_type: part.mime_type.clone(),
            part_id: part.part_id.clone(),
            attachment_id: part.body.attachment_id.clone(),
            size: part.body.size,
            status: status.into(),
        });
    }
}

pub fn normalize(raw: RawDocument) -> Result<CanonicalDocument, SourceError> {
    let metadata: Metadata = serde_json::from_str(&raw.metadata)
        .map_err(|_| failed("invalid acquired Gmail metadata"))?;
    if metadata.message_id != raw.external_id || metadata.extraction_version != VERSION {
        return Err(failed("Gmail normalization metadata mismatch"));
    }
    let texts: Vec<String> =
        serde_json::from_str(&raw.body).map_err(|_| failed("invalid acquired Gmail text"))?;
    let envelope = metadata
        .headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut segments = vec![Segment {
        text: format!(
            "{envelope}\nGmail thread: {}\nLabels: {}",
            metadata.thread_id,
            metadata.labels.join(", ")
        ),
    }];
    segments.extend(texts.into_iter().map(|text| Segment { text }));
    Ok(CanonicalDocument {
        title: Some(
            metadata
                .headers
                .get("subject")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "(no subject)".into()),
        ),
        room: None,
        name: None,
        kind: SourceKind::Other,
        uri: Some(format!(
            "https://mail.google.com/mail/?authuser={}#all/{}",
            api::encode(&metadata.mailbox),
            api::id(&metadata.message_id)?
        )),
        tags: vec!["gmail".into()],
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{Profile, tests::Fake},
        config::Options,
    };
    use serde_json::json;

    fn source() -> SourceRef {
        crate::config::identify(
            None,
            Options::parse(&[]).unwrap(),
            &Profile {
                email_address: "me@example.com".into(),
                history_id: "1".into(),
            },
        )
        .unwrap()
    }
    fn data(s: &str) -> String {
        URL_SAFE_NO_PAD.encode(s)
    }
    fn mail(payload: Value) -> Value {
        json!({"id":"a","threadId":"t","historyId":"10","internalDate":"1700000000000","labelIds":["Z","A"],"payload":payload})
    }
    fn raw(value: Value, attachments: Vec<(&'static str, Result<Value, Error>)>) -> RawDocument {
        let mut replies = vec![("/messages/a", Ok(value))];
        replies.extend(attachments);
        let mut api = Fake::new(replies);
        let Acquisition::Document(raw) = acquire(&mut api, &source(), "a").unwrap() else {
            panic!("expected document")
        };
        api.done();
        raw
    }
    #[test]
    fn mime_envelope_attachments_and_deterministic_revision() {
        let value = mail(
            json!({"mimeType":"multipart/mixed","headers":[{"name":"Subject","value":"=?UTF-8?B?UsOpc3Vtw6k=?="},{"name":"References","value":"<parent>"}],"parts":[
                {"mimeType":"multipart/alternative","parts":[{"mimeType":"text/plain","body":{"data":data("hello")}}, {"mimeType":"text/html","body":{"data":data("<b>duplicate</b>")}}]},
                {"mimeType":"text/vtt","filename":"meeting.vtt","body":{"attachmentId":"att","size":20}},
                {"mimeType":"application/pdf","filename":"doc.pdf","body":{"attachmentId":"pdf","size":1000}}
            ]}),
        );
        let first = raw(
            value.clone(),
            vec![(
                "/messages/a/attachments/att",
                Ok(json!({"data":data("WEBVTT\nhello meeting")})),
            )],
        );
        let mut reordered = value;
        reordered["labelIds"] = json!(["A", "Z"]);
        reordered["historyId"] = json!("11");
        let second = raw(
            reordered,
            vec![(
                "/messages/a/attachments/att",
                Ok(json!({"data":data("WEBVTT\nhello meeting")})),
            )],
        );
        assert_eq!(first.revision, second.revision);
        assert_eq!(
            first.occurred_at.as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
        let canonical = normalize(first).unwrap();
        assert_eq!(canonical.title.as_deref(), Some("Résumé"));
        let text = canonical
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("hello meeting")
                && text.contains("doc.pdf")
                && text.contains("unsupported")
                && text.contains("<parent>")
        );
        assert!(!text.contains("duplicate"));
    }
    #[test]
    fn html_charset_and_limits() {
        let value = mail(
            json!({"mimeType":"text/html","headers":[{"name":"Content-Type","value":"text/html; charset=windows-1252"}],"body":{"data":URL_SAFE_NO_PAD.encode(b"<style>hidden</style><script>evil</script><p>caf\xe9 &amp; tea</p>")}}),
        );
        let canonical = normalize(raw(value, vec![])).unwrap();
        let text = &canonical.segments[1].text;
        assert!(text.contains("café & tea"));
        assert!(!text.contains("hidden") && !text.contains("evil"));
        let limited = raw(
            mail(
                json!({"mimeType":"text/plain","filename":"big.txt","body":{"size":PART_BYTES+1,"attachmentId":"not-fetched"}}),
            ),
            vec![],
        );
        assert!(limited.body.contains("size_limit"));

        let alternative = raw(
            mail(json!({"mimeType":"multipart/alternative","parts":[
                {"mimeType":"text/plain","body":{"data":data("  \n")}},
                {"mimeType":"text/html","body":{"data":data("<p>usable fallback</p>")}}
            ]})),
            vec![],
        );
        assert!(
            normalize(alternative).unwrap().segments[1]
                .text
                .contains("usable fallback")
        );
    }
    #[test]
    fn transient_attachment_failure_and_bad_base64_fail_acquisition() {
        let value = mail(
            json!({"mimeType":"text/plain","filename":"a.txt","body":{"attachmentId":"att","size":5}}),
        );
        let mut api = Fake::new(vec![
            ("/messages/a", Ok(value)),
            ("/messages/a/attachments/att", Err(Error::Status(500))),
        ]);
        assert!(acquire(&mut api, &source(), "a").is_err());
        let mut api = Fake::new(vec![(
            "/messages/a",
            Ok(mail(
                json!({"mimeType":"text/plain","body":{"data":"!not-base64"}}),
            )),
        )]);
        assert!(acquire(&mut api, &source(), "a").is_err());
    }
    #[test]
    fn reconciliation_only_retires_confirmed_deletion_and_restores_live_entries() {
        let raw = raw(
            mail(json!({"mimeType":"text/plain","body":{"data":data("hello")}})),
            vec![],
        );
        let mut known = KnownDocument {
            pending: false,
            retired: false,
            external_id: "a".into(),
            revision: raw.revision,
            metadata: raw.metadata,
        };
        let minimal = || Ok(json!({"id":"a","historyId":"10"}));
        let mut api = Fake::new(vec![("/messages/a", minimal())]);
        assert!(matches!(
            reconcile(&mut api, &known).unwrap(),
            Decision::Unchanged
        ));
        known.retired = true;
        let mut api = Fake::new(vec![("/messages/a", minimal())]);
        assert!(matches!(
            reconcile(&mut api, &known).unwrap(),
            Decision::Refresh(_)
        ));
        known.retired = false;
        known.pending = true;
        let mut api = Fake::new(vec![("/messages/a", minimal())]);
        assert!(matches!(
            reconcile(&mut api, &known).unwrap(),
            Decision::Refresh(_)
        ));
        for status in [401, 403, 429, 500] {
            let mut api = Fake::new(vec![("/messages/a", Err(Error::Status(status)))]);
            assert!(reconcile(&mut api, &known).is_err());
        }
        let mut api = Fake::new(vec![("/messages/a", Err(Error::Status(404)))]);
        assert!(matches!(
            reconcile(&mut api, &known).unwrap(),
            Decision::Retire(_)
        ));
        let mut api = Fake::new(vec![("/messages/a", Err(Error::Status(404)))]);
        assert!(matches!(
            acquire(&mut api, &source(), "a").unwrap(),
            Acquisition::Retire(_)
        ));
    }

    #[test]
    fn conformance_documents_match_native_extraction() {
        let case: Value =
            serde_json::from_str(include_str!("../fixtures/gmail/case.json")).unwrap();
        let mut source = source();
        source.account = Some("reader@example.com".into());
        for (id, data) in [
            ("a", include_str!("../fixtures/responses/a.json")),
            ("b", include_str!("../fixtures/responses/b.json")),
        ] {
            let mut replies = vec![(
                if id == "a" {
                    "/messages/a"
                } else {
                    "/messages/b"
                },
                Ok(serde_json::from_str(data).unwrap()),
            )];
            if id == "b" {
                replies.push((
                    "/messages/b/attachments/transcript",
                    Ok(json!({"data":URL_SAFE_NO_PAD.encode("WEBVTT\nhello meeting\n")})),
                ));
            }
            let mut api = Fake::new(replies);
            let Acquisition::Document(raw) = acquire(&mut api, &source, id).unwrap() else {
                panic!("missing document")
            };
            let doc = normalize(raw).unwrap();
            let expected = case["documents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["external_id"] == id)
                .unwrap();
            assert_eq!(doc.title.as_deref(), expected["title"].as_str());
            assert_eq!(
                serde_json::to_value(
                    doc.segments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                expected["segments"]
            );
            api.done();
        }
    }
}
