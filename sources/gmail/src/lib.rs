//! Gmail acquisition; Core owns inventory, ingestion and durable progress.

mod api;
mod config;
mod message;
mod sync;

use api::Gmail;

wit_bindgen::generate!({ path: "wit", world: "reconciled-source" });

use exports::memcastle::source::{adapter, reconciliation};
use memcastle::source::types::{
    Candidate, CanonicalDocument, Discovery, RawDocument, SourceError, SourceRef,
};
use reconciliation::{Acquisition, Decision, DiscoveryPage, KnownDocument};

struct Source;

impl adapter::Guest for Source {
    fn identify(
        locator: Option<String>,
        options: Vec<(String, String)>,
    ) -> Result<SourceRef, SourceError> {
        let options = config::Options::parse(&options)?;
        let mut api = api::Client::connect()?;
        config::identify(locator.as_deref(), options, &api.profile()?)
    }

    fn default_wing(_: SourceRef) -> String {
        "gmail".into()
    }
    fn default_room() -> String {
        "emails".into()
    }

    fn discover(_: SourceRef, _: String, _: u32) -> Result<Discovery, SourceError> {
        Err(failed("Gmail requires the Core reconciliation extension"))
    }

    fn read(_: SourceRef, _: Candidate) -> Result<Option<RawDocument>, SourceError> {
        Err(failed("Gmail requires reconciled acquisition"))
    }

    fn normalize(raw: RawDocument) -> Result<CanonicalDocument, SourceError> {
        message::normalize(raw)
    }
}

impl reconciliation::Guest for Source {
    fn discover(
        source: SourceRef,
        cursor: String,
        limit: u32,
    ) -> Result<DiscoveryPage, SourceError> {
        let mut api = api::Client::connect()?;
        let profile = config::verify(&source, &mut api)?;
        sync::discover(&mut api, &source, &profile, &cursor, limit, sync::now()?)
    }

    fn reconcile(source: SourceRef, known: KnownDocument) -> Result<Decision, SourceError> {
        let mut api = api::Client::connect()?;
        config::verify(&source, &mut api)?;
        message::reconcile(&mut api, &known)
    }

    fn acquire(source: SourceRef, candidate: Candidate) -> Result<Acquisition, SourceError> {
        let mut api = api::Client::connect()?;
        config::verify(&source, &mut api)?;
        if candidate.handle != candidate.external_id {
            return Err(failed("invalid Gmail candidate handle"));
        }
        message::acquire(&mut api, &source, &candidate.external_id)
    }
}

fn failed(message: &str) -> SourceError {
    SourceError::Failed(message.into())
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

export!(Source);
