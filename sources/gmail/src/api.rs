//! Small, bounded Gmail REST transport. Credentials travel only through stdin.
use crate::{SourceError, config::Options, failed};
use serde::Deserialize;
use serde_json::Value;

const ORIGIN: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const MAX_RESPONSE: usize = 12 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    Status(u16),
    Failed(&'static str),
}

impl From<Error> for SourceError {
    fn from(error: Error) -> Self {
        match error {
            Error::Status(status) => failed(&format!(
                "Gmail API returned HTTP {status}; progress was not advanced"
            )),
            Error::Failed(message) => failed(message),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    pub history_id: String,
}

pub trait Gmail {
    fn get(&mut self, path: &str, params: &[(&str, String)]) -> Result<Value, Error>;

    fn profile(&mut self) -> Result<Profile, SourceError> {
        let value = self.get("/profile", &[])?;
        let profile: Profile =
            serde_json::from_value(value).map_err(|_| failed("invalid Gmail profile response"))?;
        decimal(&profile.history_id)?;
        Ok(profile)
    }
}

pub struct Client {
    token: String,
    requests: usize,
}

impl Client {
    pub fn connect() -> Result<Self, SourceError> {
        let token = crate::memcastle::source::host::access_token()
            .map_err(|_| failed("Gmail OAuth access token unavailable; sign in through Core"))?;
        if token.is_empty() || token.chars().any(char::is_control) {
            return Err(failed("invalid Gmail OAuth access token"));
        }
        Ok(Self { token, requests: 0 })
    }
}

impl Gmail for Client {
    fn get(&mut self, path: &str, params: &[(&str, String)]) -> Result<Value, Error> {
        self.requests += 1;
        if self.requests > 12 {
            return Err(Error::Failed("Gmail per-call request budget exceeded"));
        }
        let (args, input) = request(&self.token, path, params)?;
        let output =
            crate::memcastle::source::host::run_process("curl", &args, Some(input.as_bytes()))
                .map_err(|_| Error::Failed("cannot run Gmail HTTPS transport (curl required)"))?;
        if output.status != 0 {
            return Err(Error::Failed("Gmail HTTPS request failed; retry the job"));
        }
        response(&output.stdout)
    }
}

fn request(
    token: &str,
    path: &str,
    params: &[(&str, String)],
) -> Result<(Vec<String>, String), Error> {
    if !path.starts_with('/')
        || path
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || b"/-_".contains(&b)))
    {
        return Err(Error::Failed("invalid Gmail API path"));
    }
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(Error::Failed("invalid Gmail access token"));
    }
    let mut url = format!("{ORIGIN}{path}");
    for (i, (key, value)) in params.iter().enumerate() {
        url.push(if i == 0 { '?' } else { '&' });
        url.push_str(&encode(key));
        url.push('=');
        url.push_str(&encode(value));
    }
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let input = format!(
        "url = \"{}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Accept: application/json\"\n",
        quote(&url),
        quote(token)
    );
    let args = [
        "-q",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--connect-timeout",
        "5",
        "--max-time",
        "10",
        "--max-filesize",
        "12582912",
        "--write-out",
        "\n%{http_code}",
        "--config",
        "-",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    Ok((args, input))
}

fn response(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > MAX_RESPONSE + 4 {
        return Err(Error::Failed("Gmail response exceeds size limit"));
    }
    let split = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or(Error::Failed("incomplete Gmail HTTP response"))?;
    let status: u16 = std::str::from_utf8(&bytes[split + 1..])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(Error::Failed("invalid Gmail HTTP status"))?;
    if !(200..300).contains(&status) {
        return Err(Error::Status(status));
    }
    serde_json::from_slice(&bytes[..split])
        .map_err(|_| Error::Failed("invalid Gmail JSON response"))
}

pub fn encode(value: &str) -> String {
    let mut out = String::new();
    use std::fmt::Write;
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            write!(&mut out, "%{b:02X}").expect("string write");
        }
    }
    out
}

pub fn id(value: &str) -> Result<&str, SourceError> {
    object_id(value, 256)
}

pub fn attachment_id(value: &str) -> Result<&str, SourceError> {
    // Gmail's opaque attachment identifiers can be much longer than message IDs.
    object_id(value, 8192)
}

fn object_id(value: &str, max: usize) -> Result<&str, SourceError> {
    if value.is_empty()
        || value.len() > max
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(failed("invalid Gmail object ID"));
    }
    Ok(value)
}

pub fn decimal(value: &str) -> Result<&str, SourceError> {
    if value.is_empty() || value.len() > 64 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(failed("invalid Gmail history ID"));
    }
    Ok(value)
}

pub fn list(api: &mut impl Gmail, options: &Options, token: Option<&str>) -> Result<Value, Error> {
    let mut params = vec![
        ("q", options.query.clone()),
        ("includeSpamTrash", options.include_spam_trash.to_string()),
        ("maxResults", "100".into()),
    ];
    if let Some(token) = token {
        params.push(("pageToken", token.into()));
    }
    api.get("/messages", &params)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;
    pub struct Fake {
        pub replies: VecDeque<(&'static str, Result<Value, Error>)>,
        pub calls: Vec<(String, Vec<(String, String)>)>,
    }
    impl Fake {
        pub fn new(replies: Vec<(&'static str, Result<Value, Error>)>) -> Self {
            Self {
                replies: replies.into(),
                calls: vec![],
            }
        }
        pub fn done(&self) {
            assert!(self.replies.is_empty(), "unconsumed fake responses");
        }
    }
    impl Gmail for Fake {
        fn get(&mut self, path: &str, params: &[(&str, String)]) -> Result<Value, Error> {
            self.calls.push((
                path.into(),
                params
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            ));
            let (expected, result) = self.replies.pop_front().expect("unexpected API request");
            assert_eq!(path, expected);
            result
        }
    }
    #[test]
    fn token_only_in_stdin_and_query_encoded_without_rewriting() {
        let (args, input) = request(
            "private\"token",
            "/messages",
            &[("q", " {from:a+b} \"résumé\" ".into())],
        )
        .unwrap();
        assert_eq!(args[0], "-q");
        assert!(!args.join(" ").contains("private"));
        assert!(input.contains("Bearer private\\\"token"));
        assert!(input.contains("q=%20%7Bfrom%3Aa%2Bb%7D%20%22r%C3%A9sum%C3%A9%22%20"));
        assert!(!args.iter().any(|a| a == "--location"));
        assert!(request("x\nurl=evil", "/messages", &[]).is_err());
        assert!(request("x", "//evil.example", &[]).is_err());
        assert!(request("x", "/messages/../profile", &[]).is_err());
    }
    #[test]
    fn statuses_and_bad_responses_are_safe() {
        assert_eq!(response(b"{\"id\":1}\n200").unwrap()["id"], 1);
        for status in [401, 403, 404, 429, 500] {
            let error = response(format!("secret body\n{status}").as_bytes()).unwrap_err();
            assert!(matches!(error, Error::Status(s) if s == status));
            assert!(!format!("{:?}", SourceError::from(error)).contains("secret"));
        }
        assert!(response(b"{}200").is_err());
        assert!(response(b"not json\n200").is_err());
        assert!(response(&vec![0; MAX_RESPONSE + 5]).is_err());
    }

    #[test]
    fn production_request_format_runs_against_offline_fixture() {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let (args, input) = request(
            "memcastle-source-test-access-token",
            "/messages",
            &[
                ("q", "from:author@example.com".into()),
                ("includeSpamTrash", "false".into()),
                ("maxResults", "100".into()),
            ],
        )
        .unwrap();
        let mut child = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/bin/curl"))
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "fixture request rejected");
        let value = response(&output.stdout).unwrap();
        assert_eq!(value["messages"][0]["id"], "a");
        assert_eq!(value["messages"][1]["id"], "b");
    }
}
