//! Provider progress only: no durable known-document inventory lives here.
use crate::reconciliation::DiscoveryPage;
use crate::{
    Candidate, SourceError, SourceRef,
    api::{self, Error, Gmail, Profile},
    config::Options,
    failed,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_CURSOR: usize = 128 * 1024;
const BATCH: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    binding: String,
    last_sweep: u64,
    mode: Mode,
    pending: VecDeque<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "phase", deny_unknown_fields)]
enum Mode {
    Sweep {
        baseline: String,
        page: Option<String>,
    },
    History {
        progress: History,
    },
    Filter {
        ids: Vec<String>,
        page: Option<String>,
        after: History,
    },
    Idle {
        history: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct History {
    start: String,
    target: String,
    page: Option<String>,
    offset: usize,
    done: bool,
}

pub fn now() -> Result<u64, SourceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| failed("system clock predates Unix epoch"))
}

impl Cursor {
    fn new(binding: String, profile: &Profile) -> Self {
        Self {
            version: 1,
            binding,
            last_sweep: 0,
            mode: Mode::Sweep {
                baseline: profile.history_id.clone(),
                page: None,
            },
            pending: VecDeque::new(),
        }
    }

    fn encode(&self) -> Result<String, SourceError> {
        let text = serde_json::to_string(self).map_err(|_| failed("cannot encode Gmail cursor"))?;
        if text.len() > MAX_CURSOR {
            return Err(failed("Gmail cursor exceeds size limit"));
        }
        Ok(text)
    }

    fn validate(&self, binding: &str) -> Result<(), SourceError> {
        let bad = || {
            SourceError::CursorInvalid(
                "invalid or foreign Gmail cursor; restart with --full".into(),
            )
        };
        if self.version != 1 || self.binding != binding || self.pending.len() > BATCH {
            return Err(bad());
        }
        for id in &self.pending {
            api::id(id).map_err(|_| bad())?;
        }
        let token = |p: &Option<String>| -> Result<(), SourceError> {
            if p.as_ref().is_some_and(|s| s.is_empty() || s.len() > 8192) {
                Err(bad())
            } else {
                Ok(())
            }
        };
        let history = |h: &History| -> Result<(), SourceError> {
            api::decimal(&h.start).map_err(|_| bad())?;
            api::decimal(&h.target).map_err(|_| bad())?;
            token(&h.page)?;
            if greater(&h.start, &h.target) || h.offset > 1_000_000 {
                return Err(bad());
            }
            Ok(())
        };
        match &self.mode {
            Mode::Sweep { baseline, page } => {
                api::decimal(baseline).map_err(|_| bad())?;
                token(page)?;
            }
            Mode::History { progress } => history(progress)?,
            Mode::Filter { ids, page, after } => {
                if ids.len() > BATCH {
                    return Err(bad());
                }
                for id in ids {
                    api::id(id).map_err(|_| bad())?;
                }
                token(page)?;
                history(after)?;
            }
            Mode::Idle { history } => {
                api::decimal(history).map_err(|_| bad())?;
            }
        }
        Ok(())
    }
}

fn greater(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    (a.len(), a) > (b.len(), b)
}

fn next_page(value: &Value, current: Option<&str>) -> Result<Option<String>, SourceError> {
    match value.get("nextPageToken") {
        None => Ok(None),
        Some(Value::String(s))
            if !s.is_empty() && s.len() <= 8192 && Some(s.as_str()) != current =>
        {
            Ok(Some(s.clone()))
        }
        _ => Err(failed("invalid/nonadvancing Gmail page token")),
    }
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], SourceError> {
    match value.get(key) {
        None => Ok(&[]),
        Some(Value::Array(v)) => Ok(v),
        _ => Err(failed("invalid Gmail listing response")),
    }
}

fn message_ids(value: &Value) -> Result<Vec<String>, SourceError> {
    let mut ids = Vec::new();
    for message in array(value, "messages")? {
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| failed("Gmail listing omitted message ID"))?;
        ids.push(api::id(id)?.to_string());
    }
    if ids.len() > 100 {
        return Err(failed("Gmail search page exceeds requested size"));
    }
    // Gmail list order is not a chronological cursor. Preserve provider order within a page.
    let mut seen = BTreeSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    Ok(ids)
}

fn history_mode(progress: History) -> Mode {
    if progress.done {
        Mode::Idle {
            history: progress.target,
        }
    } else {
        Mode::History { progress }
    }
}

pub fn discover(
    api: &mut impl Gmail,
    source: &SourceRef,
    profile: &Profile,
    text: &str,
    limit: u32,
    now: u64,
) -> Result<DiscoveryPage, SourceError> {
    if limit == 0 {
        return Err(SourceError::InvalidInput(
            "Gmail discovery limit must be positive".into(),
        ));
    }
    api::decimal(&profile.history_id)?;
    let options = Options::parse(&source.options)?;
    let binding = options.binding(source);
    if text.len() > MAX_CURSOR {
        return Err(SourceError::CursorInvalid("oversized Gmail cursor".into()));
    }
    let mut cursor = if text.trim() == "null" {
        Cursor::new(binding.clone(), profile)
    } else {
        serde_json::from_str::<Cursor>(text)
            .map_err(|_| SourceError::CursorInvalid("unrecognized Gmail cursor".into()))?
    };
    cursor.validate(&binding)?;

    if cursor.pending.is_empty() {
        if let Mode::Idle { history } = &cursor.mode {
            if now.saturating_sub(cursor.last_sweep)
                >= u64::from(options.sweep_interval_hours) * 3600
            {
                cursor.mode = Mode::Sweep {
                    baseline: profile.history_id.clone(),
                    page: None,
                };
            } else if greater(&profile.history_id, history) {
                cursor.mode = Mode::History {
                    progress: History {
                        start: history.clone(),
                        target: profile.history_id.clone(),
                        page: None,
                        offset: 0,
                        done: false,
                    },
                };
            }
        }
        match cursor.mode.clone() {
            Mode::Sweep { baseline, page } => match api::list(api, &options, page.as_deref()) {
                Ok(value) => {
                    let next = next_page(&value, page.as_deref())?;
                    cursor.pending = message_ids(&value)?.into();
                    cursor.mode = if next.is_some() {
                        Mode::Sweep {
                            baseline,
                            page: next,
                        }
                    } else {
                        cursor.last_sweep = now;
                        history_mode(History {
                            start: baseline.clone(),
                            target: profile.history_id.clone(),
                            page: None,
                            offset: 0,
                            done: !greater(&profile.history_id, &baseline),
                        })
                    };
                }
                Err(Error::Status(400 | 404)) if page.is_some() => {
                    cursor.mode = Mode::Sweep {
                        baseline: profile.history_id.clone(),
                        page: None,
                    };
                }
                Err(error) => return Err(error.into()),
            },
            Mode::History { progress } => {
                let mut params = vec![
                    ("startHistoryId", progress.start.clone()),
                    ("maxResults", "20".into()),
                ];
                if let Some(page) = &progress.page {
                    params.push(("pageToken", page.clone()));
                }
                match api.get("/history", &params) {
                    Err(Error::Status(404)) => {
                        cursor.mode = Mode::Sweep {
                            baseline: profile.history_id.clone(),
                            page: None,
                        };
                    }
                    Err(Error::Status(400)) if progress.page.is_some() => {
                        cursor.mode = Mode::Sweep {
                            baseline: profile.history_id.clone(),
                            page: None,
                        };
                    }
                    Err(error) => return Err(error.into()),
                    Ok(value) => {
                        api::decimal(
                            value
                                .get("historyId")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    failed("Gmail history response omitted historyId")
                                })?,
                        )?;
                        let mut ids = BTreeSet::new();
                        let mut beyond = false;
                        for record in array(&value, "history")? {
                            let record_id = record
                                .get("id")
                                .and_then(Value::as_str)
                                .ok_or_else(|| failed("invalid Gmail history record"))?;
                            api::decimal(record_id)?;
                            if greater(record_id, &progress.target) {
                                beyond = true;
                                continue;
                            }
                            for key in ["messagesAdded", "labelsAdded", "labelsRemoved"] {
                                for change in array(record, key)? {
                                    let id = change
                                        .pointer("/message/id")
                                        .and_then(Value::as_str)
                                        .ok_or_else(|| failed("invalid Gmail history change"))?;
                                    ids.insert(api::id(id)?.to_string());
                                }
                            }
                        }
                        let total = ids.len();
                        if progress.offset > total {
                            return Err(failed(
                                "Gmail history page changed during replay; restart with --full",
                            ));
                        }
                        let batch: Vec<_> =
                            ids.into_iter().skip(progress.offset).take(BATCH).collect();
                        let mut after = progress.clone();
                        if progress.offset + batch.len() < total {
                            after.offset += batch.len();
                        } else {
                            after.offset = 0;
                            after.page = next_page(&value, progress.page.as_deref())?;
                            after.done = beyond || after.page.is_none();
                        }
                        cursor.mode = if batch.is_empty() {
                            history_mode(after)
                        } else {
                            Mode::Filter {
                                ids: batch,
                                page: None,
                                after,
                            }
                        };
                    }
                }
            }
            Mode::Filter {
                mut ids,
                page,
                after,
            } => match api::list(api, &options, page.as_deref()) {
                Ok(value) => {
                    let next = next_page(&value, page.as_deref())?;
                    let matches = message_ids(&value)?;
                    cursor.pending = matches.into_iter().filter(|id| ids.contains(id)).collect();
                    ids.retain(|id| !cursor.pending.contains(id));
                    cursor.mode = if ids.is_empty() || next.is_none() {
                        history_mode(after)
                    } else {
                        Mode::Filter {
                            ids,
                            page: next,
                            after,
                        }
                    };
                }
                Err(Error::Status(400 | 404)) if page.is_some() => {
                    cursor.mode = Mode::Filter {
                        ids,
                        page: None,
                        after,
                    };
                }
                Err(error) => return Err(error.into()),
            },
            Mode::Idle { .. } => {}
        }
    }

    let mut candidates = Vec::new();
    while candidates.len() < limit as usize {
        let Some(id) = cursor.pending.pop_front() else {
            break;
        };
        candidates.push(Candidate {
            external_id: id.clone(),
            handle: id,
            cursor_after: cursor.encode()?,
        });
    }
    Ok(DiscoveryPage {
        candidates,
        exhausted: cursor.pending.is_empty() && matches!(cursor.mode, Mode::Idle { .. }),
        cursor_after: Some(cursor.encode()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::Fake;
    use serde_json::json;

    fn context() -> (SourceRef, Profile) {
        let profile = Profile {
            email_address: "me@example.com".into(),
            history_id: "10".into(),
        };
        (
            crate::config::identify(
                None,
                Options::parse(&[("query".into(), "older_than:1y".into())]).unwrap(),
                &profile,
            )
            .unwrap(),
            profile,
        )
    }
    #[test]
    fn pagination_checkpoints_resume_without_skipping() {
        let (source, profile) = context();
        let mut api = Fake::new(vec![
            (
                "/messages",
                Ok(json!({"messages":[{"id":"a"},{"id":"b"}],"nextPageToken":"p2"})),
            ),
            ("/messages", Ok(json!({"messages":[{"id":"c"}]}))),
        ]);
        let first = discover(&mut api, &source, &profile, "null", 1, 100).unwrap();
        assert_eq!(first.candidates[0].external_id, "a");
        let second = discover(
            &mut api,
            &source,
            &profile,
            &first.candidates[0].cursor_after,
            1,
            100,
        )
        .unwrap();
        assert_eq!(second.candidates[0].external_id, "b");
        let last = discover(
            &mut api,
            &source,
            &profile,
            &second.candidates[0].cursor_after,
            1,
            100,
        )
        .unwrap();
        assert_eq!(last.candidates[0].external_id, "c");
        assert!(last.exhausted);
        let idle = discover(
            &mut api,
            &source,
            &profile,
            &last.cursor_after.unwrap(),
            1,
            101,
        )
        .unwrap();
        assert!(idle.exhausted && idle.candidates.is_empty());
        api.done();
        assert_eq!(api.calls[0].1[0], ("q".into(), "older_than:1y".into()));
    }
    #[test]
    fn bootstrap_catches_changes_and_filters_history_with_exact_query() {
        let (source, mut profile) = context();
        let mut api = Fake::new(vec![
            ("/messages", Ok(json!({"nextPageToken":"p2"}))),
            ("/messages", Ok(json!({}))),
            (
                "/history",
                Ok(
                    json!({"historyId":"12","history":[{"id":"11","messagesAdded":[{"message":{"id":"new"}},{"message":{"id":"outside"}}],"labelsAdded":[{"message":{"id":"new"}}]}]}),
                ),
            ),
            (
                "/messages",
                Ok(json!({"nextPageToken":"p2","messages":[{"id":"old"}]})),
            ),
            ("/messages", Ok(json!({"messages":[{"id":"new"}]}))),
        ]);
        let first = discover(&mut api, &source, &profile, "null", 1, 100).unwrap();
        assert!(!first.exhausted && first.candidates.is_empty());
        profile.history_id = "12".into();
        let mut cursor = first.cursor_after.unwrap();
        let mut found = vec![];
        for _ in 0..4 {
            let page = discover(&mut api, &source, &profile, &cursor, 1, 101).unwrap();
            cursor = page.cursor_after.unwrap();
            found.extend(page.candidates.into_iter().map(|c| c.external_id));
        }
        assert_eq!(found, vec!["new"]);
        assert!(
            discover(&mut api, &source, &profile, &cursor, 1, 102)
                .unwrap()
                .exhausted
        );
        api.done();
    }
    #[test]
    fn time_sweep_and_expired_history_do_not_need_new_messages() {
        let (source, mut profile) = context();
        let mut api = Fake::new(vec![
            ("/messages", Ok(json!({}))),
            ("/messages", Ok(json!({"messages":[{"id":"aged"}]}))),
            ("/history", Err(Error::Status(404))),
            ("/messages", Ok(json!({}))),
        ]);
        let first = discover(&mut api, &source, &profile, "null", 1, 100).unwrap();
        let aged = discover(
            &mut api,
            &source,
            &profile,
            &first.cursor_after.unwrap(),
            1,
            86501,
        )
        .unwrap();
        assert_eq!(aged.candidates[0].external_id, "aged");
        profile.history_id = "20".into();
        let expired = discover(
            &mut api,
            &source,
            &profile,
            &aged.cursor_after.unwrap(),
            1,
            86502,
        )
        .unwrap();
        assert!(!expired.exhausted && expired.candidates.is_empty());
        let restarted = discover(
            &mut api,
            &source,
            &profile,
            &expired.cursor_after.unwrap(),
            1,
            86503,
        )
        .unwrap();
        assert!(restarted.exhausted);
        api.done();
    }
    #[test]
    fn large_history_record_is_batched_without_advancing_past_unfinished_work() {
        let (source, mut profile) = context();
        let options = Options::parse(&source.options).unwrap();
        let mut cursor = Cursor::new(options.binding(&source), &profile);
        cursor.last_sweep = 100;
        cursor.mode = Mode::Idle {
            history: "10".into(),
        };
        profile.history_id = "20".into();
        let changes: Vec<_> = (0..201)
            .map(|n| json!({"message":{"id":format!("m{n:03}")}}))
            .collect();
        let history = json!({"historyId":"25","history":[{"id":"15","messagesAdded":changes},{"id":"25","messagesAdded":[{"message":{"id":"future"}}]}]});
        let mut api = Fake::new(vec![
            ("/history", Ok(history.clone())),
            ("/messages", Ok(json!({"messages":[{"id":"m000"}]}))),
            ("/history", Ok(history)),
            ("/messages", Ok(json!({"messages":[{"id":"m200"}]}))),
        ]);
        let start = cursor.encode().unwrap();
        let h1 = discover(&mut api, &source, &profile, &start, 1, 101).unwrap();
        let first = discover(
            &mut api,
            &source,
            &profile,
            h1.cursor_after.as_ref().unwrap(),
            1,
            101,
        )
        .unwrap();
        assert_eq!(first.candidates[0].external_id, "m000");
        assert!(!first.exhausted);
        let h2 = discover(
            &mut api,
            &source,
            &profile,
            &first.candidates[0].cursor_after,
            1,
            101,
        )
        .unwrap();
        let second = discover(
            &mut api,
            &source,
            &profile,
            h2.cursor_after.as_ref().unwrap(),
            1,
            101,
        )
        .unwrap();
        assert_eq!(second.candidates[0].external_id, "m200");
        assert!(second.exhausted);
        let end: Cursor = serde_json::from_str(&second.cursor_after.unwrap()).unwrap();
        assert!(matches!(end.mode,Mode::Idle{history} if history == "20"));
        api.done();
    }

    #[test]
    fn expired_search_token_restarts_safely_and_query_edit_rejects_old_cursor() {
        let (mut source, profile) = context();
        let mut api = Fake::new(vec![
            ("/messages", Ok(json!({"nextPageToken":"expired"}))),
            ("/messages", Err(Error::Status(400))),
            ("/messages", Ok(json!({"messages":[{"id":"replayed"}]}))),
        ]);
        let first = discover(&mut api, &source, &profile, "null", 1, 100).unwrap();
        let reset = discover(
            &mut api,
            &source,
            &profile,
            first.cursor_after.as_ref().unwrap(),
            1,
            101,
        )
        .unwrap();
        assert_ne!(first.cursor_after, reset.cursor_after);
        let last = discover(
            &mut api,
            &source,
            &profile,
            reset.cursor_after.as_ref().unwrap(),
            1,
            102,
        )
        .unwrap();
        assert_eq!(last.candidates[0].external_id, "replayed");
        source.options[0].1 = "different query".into();
        assert!(
            discover(
                &mut api,
                &source,
                &profile,
                last.cursor_after.as_ref().unwrap(),
                1,
                102
            )
            .is_err()
        );
        api.done();
    }

    #[test]
    fn failed_pages_do_not_yield_a_checkpoint_and_foreign_cursors_fail() {
        let (source, profile) = context();
        let mut api = Fake::new(vec![("/messages", Err(Error::Status(429)))]);
        assert!(discover(&mut api, &source, &profile, "null", 1, 100).is_err());
        assert!(discover(&mut api, &source, &profile, "{}", 1, 100).is_err());
        let mut cursor = Cursor::new("wrong".into(), &profile);
        assert!(
            discover(
                &mut api,
                &source,
                &profile,
                &cursor.encode().unwrap(),
                1,
                100
            )
            .is_err()
        );
        cursor.binding = Options::parse(&source.options).unwrap().binding(&source);
        cursor.pending.push_back("../bad".into());
        assert!(
            discover(
                &mut api,
                &source,
                &profile,
                &cursor.encode().unwrap(),
                1,
                100
            )
            .is_err()
        );
    }
}
