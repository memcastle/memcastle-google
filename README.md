# google

## Modules and root tasks

- Source: [gmail](sources/gmail/).

Use `mise tasks ls` for `source:gmail:build`, `source:gmail:test`, `source:gmail:lint`, and `source:gmail:format:check`.
The build task stages `source.wasm` for packaging; native tests do not replace host conformance.
Requires Rust 1.100+, `wasm32-wasip3`, wasm-tools 1.263.0, and upstream development Core containing `828f0f6`.
Stable 0.5.2 predates WASI 0.3 despite sharing the development version string; the compatibility range is transitional.
Run `python3 scripts/validate_component.py source sources/gmail/source.wasm` before host and archive checks.

[![CI][ci-badge]][ci-workflow] [![Codecov][codecov-badge]][codecov-project]

[ci-badge]: https://github.com/memcastle/memcastle-google/actions/workflows/ci.yaml/badge.svg?branch=main
[ci-workflow]: https://github.com/memcastle/memcastle-google/actions/workflows/ci.yaml
[codecov-badge]: https://codecov.io/gh/memcastle/memcastle-google/graph/badge.svg
[codecov-project]: https://codecov.io/gh/memcastle/memcastle-google

Google services integration for MemCastle

This is a versioned MemCastle provider plugin.
It includes source modules.

## Gmail

The `gmail` source mines messages and textual attachments using native Gmail
search, resumable history synchronization, and Core-managed deletion reconciliation.
See the [Gmail module](sources/gmail/README.md) for options and semantics, and the
[OAuth setup guide](docs/oauth.md) to create a Desktop client.

This implementation temporarily requires **>=0.5.2** and targets development Core
with source contract `0.5.0` and WASI 0.3 support until Core's release version is
bumped. Core supports custom OAuth clients and offline OAuth conformance; see the
[Core baseline](docs/core-prerequisites.md). The default OAuth client is awaiting
registration; users can configure their own client now.

## Install

If this plugin has a published release in the
[MemCastle plugin catalogue](https://memcastle.github.io/plugins.json),
install it by ID with a plugin-capable MemCastle Core binary:

```sh
memcastle plugin install google
```

Core retrieves and checks the release archive for you.

If the catalogue entry has no published release yet, build and package the project first (below).

After installation, explicitly enable the sources you want to use.

## Build and test locally

You'll need a plugin-capable MemCastle Core binary and Git.

Install Rust **1.100 or newer** with the `wasm32-wasip3` target for source modules.

For each source, run its tests, build the Wasm component and copy the compiled component
to `sources/<id>/source.wasm`.
See the module's README and the CI workflow for its crate-specific artifact name:

```sh
cargo generate-lockfile --manifest-path sources/<id>/Cargo.toml
cargo test --manifest-path sources/<id>/Cargo.toml --locked --lib
cargo build --manifest-path sources/<id>/Cargo.toml --locked --release --target wasm32-wasip3
```

From the project root, package and smoke-test the result:

```sh
memcastle plugin inspect --json
memcastle plugin package
memcastle plugin test dist/google-VERSION.tar.gz
```

Replace `VERSION` with the version printed by `memcastle plugin package`
or the name of the archive in `dist/`.

The package command reads `plugin.toml` and creates an archive with its own versioned `manifest.toml`
plus a matching `.sha256sum` file.
It does not rewrite `plugin.toml` or create a root-level `manifest.toml`.

A `VERSION` SemVer tag supplies a release version;
untagged Git commits get a development version.
Outside a Git checkout, pass `--version` when packaging.

## Project layout

| Path | Purpose |
| --- | --- |
| `plugin.toml` | Plugin identity, supported Core versions and module entries. |
| `sources/<id>/` | Rust/Wasm source modules, each with its own manifest and tests. |
| `dist/` | Generated release archive and checksum after packaging. |

## CI and releases

CI builds the modules that exist, packages the plugin and validates its archive against MemCastle Core.

The `core_versions` answer is `development` only; this plugin requires Core `>=0.5.2`
and its reconciliation extension.
Validation runs on pull requests and pushes to `main`.

When modules have native tests, their jobs upload available LCOV and JUnit results to Codecov using OIDC.
Enable this repository in Codecov to see coverage and test-result statuses.

Project and patch targets are `80%` and `80%`;
reporting sources and integrations use `80%` and `80%`.
Only modules listed in `codecov_covered_sources` or `codecov_covered_integrations`
have blocking component coverage checks.

Releases use gh-ship and git-cliff:
merging a releasable Conventional Commit into `main` opens a `release/next` PR.
Merging the release PR publishes a validated `google-VERSION.tar.gz` archive under the `VERSION` Git tag.

Configure the `release` environment with variable `APP_CLIENT_ID`
and secret `APP_PRIVATE_KEY` for an App installed on this repository.
The publish workflow uses this credential to check the draft release and upload its validated archive.
Grant contents, actions and pull requests write permission;
issues write is needed to create the release label.

Set squash-merge title to the PR title and body to blank, and enable the release environment.
The `prepare-release` workflow's `dry_run` input avoids pushing or committing.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for local development and release guidance.
Coding agents should also read [AGENTS.md](AGENTS.md).
