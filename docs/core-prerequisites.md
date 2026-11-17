# Core prerequisites for Gmail

Gmail is implemented against the development reconciliation contract introduced
in `memcastle/memcastle@5772b582a7445b0099925b3fea09f853f79011a3`.
The plugin keeps base WIT contract `0.4.0`, extension `0.1.0`, and minimum Core
`>=0.6`. These are independent versions.

The following prerequisites must be resolved before claiming fully usable
default authentication, runtime custom-client support, and end-to-end validation.
They are Core/maintainer work, not behavior emulated inside this source.

## Compatible development binary

At initial implementation, installed and published development Core identified
as `0.5.2`, even though current Core source contained reconciliation support.
Such a binary does not satisfy `>=0.6`. Publish an appropriately versioned
development binary containing the extension. CI intentionally tests development
only; do not widen the plugin's compatibility range to bypass this check.

Acceptance: inspect, source conformance, reconciliation mining tests and archive
lifecycle tests all run on an actual compatible binary without changing manifests.

## Effective OAuth client configuration

Core currently takes the public client ID from `[permissions.oauth]` in the
installed source manifest. A Gmail `client_id` source option cannot change
authorization that has already happened.

Required Core behavior:

- A user-owned public client-ID override, preferably on a reusable source
  configuration, with the manifest's project-owned client as the default.
- Apply the effective ID to authorization, code exchange and refresh; preserve
  the manifest's endpoints, scopes and PKCE behavior.
- Bind credentials and consent to the effective client and selected source
  configuration. Tokens from different clients/accounts must not be reused.
- Client changes require fresh authorization. Queue/resume behavior must pin the
  appropriate identity and fail clearly rather than silently switch clients.
- Expose the public effective ID/configuration through the supported configuration
  interface without exposing tokens.

Acceptance: two configurations can use different clients; both exchange and
refresh successfully. Changing one client requires reauthentication without
invalidating or borrowing the other configuration's tokens.

There is deliberately no purported TOML/CLI override example here yet: Core has
not defined that interface. Once implemented, update the [OAuth guide](oauth.md)
with its real syntax and validate the documented procedure.

## Production-component OAuth conformance

Core's existing `[test] fixture_credential` injects a selected env/file credential,
not an OAuth access token. The conformance runner currently supplies no OAuth
provider. Consequently fake curl alone cannot make `host.access-token` succeed.

Provide a conformance-only opt-in that supplies a fixed synthetic access token
through the normal OAuth host function, requiring the normal OAuth declaration.
It must never resolve a developer sign-in, environment variable or credential file.
The proposed fixture value used by this repository is
`memcastle-source-test-access-token`; coordinate its final value with Core.
No new manifest field is declared until Core actually accepts it.

Acceptance: the exact production `source.wasm` passes fixtures with fake curl,
without network or real credentials. A missing token provider still fails; no
source option, selected-credential fallback, magic token branch or test-feature
build may bypass OAuth in the released component.

Then add disposable-palace production-Wasm mining coverage for repeated runs,
document-limited inventory continuation, query exclusions retained, confirmed
deletion/history retention, restoration at the same revision, pending retries,
and independent named miners. `memcastle plugin test` alone checks installation
lifecycle, not these mining semantics.

## Default client registration

The manifest contains an unmistakable nonfunctional development placeholder.
Maintainers must create the project's Desktop client, replace the public default
ID, and verify both browser PKCE authorization and subsequent refresh. See
[OAuth setup](oauth.md). Do not publish a release described as ready for default
sign-in while that placeholder remains.
