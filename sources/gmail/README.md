# Gmail source

Mine Gmail messages and textual attachments using Gmail's native search syntax.
The source uses the `memcastle:source@0.5.0` contract with reconciliation extension
`0.1.0`, targeting **development Core** with a temporary minimum of **>=0.5.2**
until Core's release version is bumped. Older binaries reporting `0.5.2` may lack
the required contract/runtime; use the development build with WASI 0.3 support.

## Status and prerequisites

The [Core baseline](../../docs/core-prerequisites.md) supports reconciliation,
offline production-component conformance, and runtime OAuth client overrides.
The manifest's default client ID is currently an **unconfigured development placeholder**.
For online use, configure your own real Desktop OAuth client using the guide below.

Install `curl` on the machine running the daemon. Core runs it through the
consented process permission; no shell, filesystem, environment-variable, or
selected-token grants are needed. Sign-in and token renewal belong to Core.

## Configure and mine

After configuring a working client as described in the [OAuth guide](../../docs/oauth.md),
install/enable this plugin's Gmail source and sign in:

```sh
memcastle source auth gmail
memcastle mine gmail me 'query=from:someone@example.com after:2026/01/01'
```

The locator can be omitted, `me`, an email address, or `gmail:email@example.com`.
It is resolved to the signed-in mailbox; an explicit different mailbox is rejected.
Mailbox identity uses the Gmail profile's email address, so an account address
rename requires configuring the new identity deliberately.

| Option | Default | Meaning |
| --- | --- | --- |
| `query` | empty | Native Gmail search value, passed unchanged to `messages.list.q`. |
| `include_spam_trash` | `false` | Include spam/trash in the API search universe. Set `true` for searches such as `in:anywhere` that need them. |
| `sweep_interval_hours` | `24` | Positive integer: interval between full searches, including when history is unchanged. |

Examples: `query=has:attachment`, `query=to:team@example.com newer_than:1m`,
or combinations of Gmail operators. Quote the entire `query=...` argument when
it contains spaces. Unknown/duplicate options and invalid booleans/intervals fail.
Queries are not trimmed, rewritten, parsed, or augmented with hidden operators.
Gmail validates syntax. Gmail API search has documented differences from the web
UI, including alias expansion and thread-wide search; see [Google's search guide](https://developers.google.com/workspace/gmail/api/guides/filtering).

For saved miners, keep the miner name and mailbox stable when changing its query.
Filters stay outside storage identity. Core isolates named miners and restarts
discovery on option changes while retaining inventory. Arbitrary query changes
have no inferred breadth ordering; Core may require `--allow-broaden`.

## Incremental sync and reconciliation

- Bootstrap captures a history baseline before paginated search. It then catches
  up history, avoiding a gap during the initial scan.
- Later runs use Gmail history. Changed IDs are intersected with pages from the
  **same unmodified query**; Gmail history itself has no arbitrary-query filter.
  This costs search requests when the mailbox changes, but does not download every
  matching message on every poll. Pages and bounded pending work resume through
  Core checkpoints, including pages with no matching candidates.
- A periodic full search finds messages that start matching without any history
  event, such as an old message aging into `older_than:1y`. Completion depends on
  miner scheduling and document budget; short-lived matches can be missed.
- Expired Gmail history starts a new search without dropping inventory. API
  page-token expiry can replay a scan; deterministic revisions make replay safe.
- Core walks its known-document inventory before discovery, including on full
  runs, and counts those checks toward the document budget. Large inventories or
  small budgets can delay discovery. The plugin does not maintain a second inventory.

**Retirement is deletion-only.** A confirmed message-specific 404 under the
verified signed-in mailbox retires that document's current content through Core.
Previously mined messages that leave the query are retained and can still be
refreshed. Narrowing a query does not erase already mined knowledge. Search
omissions, expired history, partial results, auth failures, rate limits and
network/server errors never retire content. Core retains history and restores a
retired document if it becomes accessible again.

## Content and attachments

Each Gmail message is one source document, identified by Gmail message ID.
Metadata includes sender/recipients, subject, Date, internal date, labels,
message/thread IDs and reply/reference headers. The occurrence timestamp is
Gmail's internal date. Thread context is preserved through those identifiers;
nonmatching messages from the same thread are not fetched implicitly.

MIME alternatives prefer plain text, with HTML fallback. HTML is converted to
text without loading external resources. Text, Markdown, HTML, VTT and SRT
attachments are extracted, whether inline or separately fetched. PDF, Office,
audio, archives and other binaries are **metadata-only**: filename, MIME type,
size and identifiers remain available, with a searchable omission summary.
The current source contract has no binary-artifact storage channel.

Extraction limits are 1 MiB per decoded text part, 4 MiB extracted text total,
8 separately fetched textual parts, 256 MIME parts, and 20 nested MIME levels.
Unsupported/oversized parts get explicit status; unknown charsets are reported
as unsupported. Malformed base64/declared encodings and temporary attachment
fetch failures fail acquisition so a retry can recover, rather than indexing a
silently incomplete success. HTTP responses are capped at 12 MiB, and each API
request has a 5-second connection / 10-second total timeout. Revisions hash
deterministic content and relevant metadata, excluding transport/progress data.

## Development

Use Rust 1.100 or newer, Python 3 for the offline transport fixture, and the
`wasm32-wasip3` target. Bindings use wit-bindgen 0.63 with `sync: true`: Rust
methods remain synchronous while acquisition exports use Core's async WIT types.

```sh
cargo fmt --all -- --check
cargo test --locked --lib
cargo clippy --locked --target wasm32-wasip3 -- -D warnings
cargo build --locked --release --target wasm32-wasip3
cp target/wasm32-wasip3/release/gmail.wasm source.wasm
```

Commit `Cargo.lock`. Native tests use injected REST responses and exercise the
checked-in offline curl fixture. They require no Google account or live network.
The fake transport rejects all unexpected endpoints and never forwards requests.

Run production-component conformance with the fake transport:

```sh
PATH="$PWD/fixtures/bin:$PATH" memcastle source test .
```

The manifest enables `[test] fixture_access_token = true`; Core supplies
`memcastle-source-test-access-token` through the ordinary OAuth host function.
No real sign-in is resolved, and no test-token special case exists in the plugin.
The setting affects only the conformance runner, not installed execution.

Build this component before packaging the containing provider plugin; see the
root [contributing guide](../../CONTRIBUTING.md).
