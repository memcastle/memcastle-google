# Core baseline for Gmail

The Core requirements tracked in [memcastle#388](https://github.com/memcastle/memcastle/issues/388)
are available in development Core. Gmail targets the baseline at
`memcastle/memcastle@828f0f6e6d937fe29b379e1a63b0985c40ab8aa7`:

- Source WIT package `memcastle:source@0.5.0`, with async acquisition functions.
- Reconciliation capability `0.1.0`, independently versioned from the WIT package.
- Rust 1.100 or newer, `wasm32-wasip3` / WASI 0.3, wit-bindgen 0.63, and Core's
  compatible Wasmtime runtime. Rust uses stackful `sync: true` bindings.
- Core-managed OAuth client-ID overrides on source configurations.
- Synthetic OAuth access tokens in the production-component conformance runner.

The minimum remains temporarily `>=0.5.2` until Core's release version is bumped.
CI tests development only. Older stable binaries reporting the same version may
lack these capabilities; use the development binary. Raise the minimum to the
release containing this baseline once Core bumps its version.

## Effective OAuth client configuration

The manifest supplies the default public client ID. Set `oauth_client_id` on a
source configuration, or use `memcastle source config set --oauth-client-id`, to
select a user-owned public client without rebuilding the plugin.

Core applies the effective ID to authorization, code exchange and refresh while
preserving the declared endpoints, scopes, callback and PKCE behavior. Named
configurations have separate authorization slots. Changing a client requires a
fresh sign-in; queued/resumed jobs cannot silently switch OAuth identity.

See the [OAuth setup guide](oauth.md) for creation and configuration examples.

## Production-component conformance

`[test] fixture_access_token = true` supplies exactly
`memcastle-source-test-access-token` through the normal `host.access-token`
function. The normal OAuth declaration is still required. This is separate from
`fixture_credential`, which supplies an env/file credential rather than OAuth.

The checked-in fake curl accepts the synthetic token and never contacts Gmail.
The production component is exercised without developer credentials or test-only
code paths. Normalization has no token access, and the setting does not change
installed execution. CI runs this conformance check before archive lifecycle
validation.

Native tests additionally exercise incremental paging/replay, expired-history
recovery, query exclusions retained, pending refresh, confirmed deletion,
restoration at the same revision, and deterministic message/attachment extraction.
Archive lifecycle tests validate installation; they do not replace mining tests.

## Remaining default-client registration

The manifest still contains an explicit nonfunctional development placeholder.
Maintainers must register the project's Desktop client and replace the public
default ID before a release with ready-to-use default sign-in. Users can already
select their own client through Core's override. Live Google authorization and
later refresh must be verified with a real registration; offline conformance
does not validate Google's consent or token endpoints.
