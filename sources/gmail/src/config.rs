use crate::{
    SourceError, SourceRef,
    api::{Gmail, Profile},
    digest, failed,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Options {
    pub query: String,
    pub include_spam_trash: bool,
    pub sweep_interval_hours: u32,
}

impl Options {
    pub fn parse(pairs: &[(String, String)]) -> Result<Self, SourceError> {
        let mut values = BTreeMap::new();
        for (key, value) in pairs {
            if !matches!(
                key.as_str(),
                "query" | "include_spam_trash" | "sweep_interval_hours"
            ) {
                return Err(SourceError::InvalidInput(format!(
                    "unknown Gmail option: {key}"
                )));
            }
            if values.insert(key.as_str(), value.as_str()).is_some() {
                return Err(SourceError::InvalidInput(format!(
                    "duplicate Gmail option: {key}"
                )));
            }
        }
        let include_spam_trash = values
            .get("include_spam_trash")
            .unwrap_or(&"false")
            .parse()
            .map_err(|_| {
                SourceError::InvalidInput("include_spam_trash must be true or false".into())
            })?;
        let sweep_interval_hours: u32 = values
            .get("sweep_interval_hours")
            .unwrap_or(&"24")
            .parse()
            .map_err(|_| {
                SourceError::InvalidInput("sweep_interval_hours must be a positive integer".into())
            })?;
        if sweep_interval_hours == 0 {
            return Err(SourceError::InvalidInput(
                "sweep_interval_hours must be positive".into(),
            ));
        }
        Ok(Self {
            query: values.get("query").unwrap_or(&"").to_string(),
            include_spam_trash,
            sweep_interval_hours,
        })
    }

    pub fn pairs(&self) -> Vec<(String, String)> {
        vec![
            ("query".into(), self.query.clone()),
            (
                "include_spam_trash".into(),
                self.include_spam_trash.to_string(),
            ),
            (
                "sweep_interval_hours".into(),
                self.sweep_interval_hours.to_string(),
            ),
        ]
    }

    pub fn binding(&self, source: &SourceRef) -> String {
        digest(
            &serde_json::to_vec(&(source.account.as_ref(), &source.locator, self))
                .expect("serializable options"),
        )
    }
}

pub fn identify(
    locator: Option<&str>,
    options: Options,
    profile: &Profile,
) -> Result<SourceRef, SourceError> {
    let mailbox = profile.email_address.to_ascii_lowercase();
    if !mailbox.contains('@') || mailbox.chars().any(char::is_control) {
        return Err(failed("Gmail returned an invalid mailbox identity"));
    }
    let requested = locator
        .unwrap_or("me")
        .strip_prefix("gmail:")
        .unwrap_or(locator.unwrap_or("me"));
    if requested != "me" && !requested.eq_ignore_ascii_case(&mailbox) {
        return Err(SourceError::InvalidInput(
            "Gmail locator does not match the authenticated mailbox".into(),
        ));
    }
    Ok(SourceRef {
        source: "gmail".into(),
        account: Some(mailbox.clone()),
        locator: format!("gmail:{mailbox}"),
        options: options.pairs(),
    })
}

pub fn verify(source: &SourceRef, api: &mut impl Gmail) -> Result<Profile, SourceError> {
    let profile = api.profile()?;
    let canonical = identify(
        Some(&source.locator),
        Options::parse(&source.options)?,
        &profile,
    )?;
    if source.source != "gmail"
        || source.account != canonical.account
        || source.locator != canonical.locator
    {
        return Err(failed(
            "Gmail source identity no longer matches the signed-in mailbox",
        ));
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_query_and_filter_independent_identity() {
        let q = "  {from:a@example.com to:b} newer_than:1d \"a+b\"  ";
        let options = Options::parse(&[("query".into(), q.into())]).unwrap();
        assert_eq!(options.query, q);
        let profile = Profile {
            email_address: "Me@Example.com".into(),
            history_id: "1".into(),
        };
        let a = identify(None, options, &profile).unwrap();
        let b = identify(
            Some("gmail:me@example.com"),
            Options::parse(&[]).unwrap(),
            &profile,
        )
        .unwrap();
        assert_eq!((a.account, a.locator), (b.account, b.locator));
        assert!(
            identify(
                Some("other@example.com"),
                Options::parse(&[]).unwrap(),
                &profile
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_bad_options() {
        for pairs in [
            vec![("oops", "x")],
            vec![("query", "a"), ("query", "b")],
            vec![("include_spam_trash", "yes")],
            vec![("sweep_interval_hours", "0")],
        ] {
            assert!(
                Options::parse(
                    &pairs
                        .into_iter()
                        .map(|(k, v)| (k.into(), v.into()))
                        .collect::<Vec<_>>()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn account_check_precedes_object_access() {
        use crate::api::tests::Fake;
        use serde_json::json;
        let profile = Profile {
            email_address: "me@example.com".into(),
            history_id: "10".into(),
        };
        let source = identify(None, Options::parse(&[]).unwrap(), &profile).unwrap();
        let mut api = Fake::new(vec![(
            "/profile",
            Ok(json!({"emailAddress":"other@example.com","historyId":"20"})),
        )]);
        assert!(verify(&source, &mut api).is_err());
        api.done();
        let mut api = Fake::new(vec![(
            "/profile",
            Ok(json!({"emailAddress":"me@example.com","historyId":"20"})),
        )]);
        assert_eq!(verify(&source, &mut api).unwrap().history_id, "20");
        api.done();
    }
}
