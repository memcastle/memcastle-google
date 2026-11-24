# Gmail OAuth setup

Gmail uses Core-managed browser authorization with PKCE and a loopback callback.
Core owns access/refresh tokens and renewal. The plugin only requests the current
access token when making Gmail API calls.

## Default client and your own client

The intended default is a project-owned Google **Desktop app** OAuth client.
At present its ID has not been supplied: the source manifest contains
`unconfigured-google-desktop-client.apps.googleusercontent.com`, which cannot
authenticate. Maintainers must replace it before a usable default-client release.

Users can select their own public client ID through Core, falling back to the
manifest's project client when no override is set. With the Gmail source installed
and enabled, configure the default source configuration:

```sh
memcastle source config set gmail --source gmail --oauth-client-id YOUR_CLIENT_ID.apps.googleusercontent.com
memcastle source auth gmail
```

Or define a named configuration in MemCastle's `config.toml`:

```toml
[[source_configs]]
id = "gmail-work"
source = "gmail"
oauth_client_id = "YOUR_CLIENT_ID.apps.googleusercontent.com"

[[miners]]
name = "work-mail"
source = "gmail"
source_config = "gmail-work"
locator = "me"

[miners.options]
query = "from:someone@example.com"
```

Authorize a named configuration with `memcastle source config auth gmail-work`.
Several miners can share that configuration and sign-in. Named configurations do
not inherit another configuration's client or credentials.

Core applies the effective client ID to sign-in, code exchange and refresh;
endpoints, scopes, callback and PKCE remain those declared by the plugin. Sign in
again after changing the client. To restore the manifest default, use
`memcastle source config set gmail --unset-oauth-client-id` (once a real project
default is available). Configuration views expose the public effective ID, not
tokens. No plugin rebuild or editing installed package files is necessary.

## Create a Google OAuth client

These steps apply to maintainers creating the default and users creating their own.
Google occasionally changes console navigation; see the official
[Gmail quickstart](https://developers.google.com/workspace/gmail/api/quickstart/python)
and [Desktop OAuth guide](https://developers.google.com/identity/protocols/oauth2/native-app).

1. Open [Google Cloud Console](https://console.cloud.google.com/). Create or select
   the project that will own the client and its Gmail API quota.
2. Under **APIs & Services → Library**, find **Gmail API** and enable it.
3. Open **Google Auth Platform → Branding** (or **Get started** for a new project).
   Configure app name, support email and developer contact details. For the
   project-owned public app, configure the applicable homepage/privacy-policy
   information and verified domains.
4. Under **Audience**, select **Internal** only for an eligible Workspace
   organization whose users are the intended audience. Otherwise select
   **External**. While the app is in Testing, add every account that will sign in
   under **Test users**.
5. Under **Data Access → Add or remove scopes**, select only:

   ```text
   https://www.googleapis.com/auth/gmail.readonly
   ```

   Save it. This permits search, bodies and attachments without granting send,
   modify, delete, Calendar or Drive access. `gmail.metadata` is insufficient:
   it cannot read message bodies and does not permit the search `q` parameter.
6. Under **Clients → Create client**, choose **Desktop app**, give it a useful
   name, and create it. Copy the public **Client ID** ending in
   `.apps.googleusercontent.com`. A Web application, service account, deprecated
   out-of-band flow, or device flow is not the setup used here.
7. Use that ID as the project's manifest default, or in Core's user-client
   override above. Downloaded Google JSON may also contain a client
   secret; do not put it in the plugin manifest, source options, or repository.
   Core's broker uses a public client with PKCE and no client secret.

Desktop clients use a loopback redirect to the local listener Core opens, with
Core's callback path (default `/callback`). Do not configure a hosted redirect
or paste an authorization code into a query option. A remote/headless daemon
needs a browser/callback arrangement supported by Core; this source does not
implement a separate sign-in flow.

## Consent, testing mode and refresh

`gmail.readonly` is a Google **restricted scope**. Public distribution is subject
to Google's applicable consent-screen verification and data-use requirements;
see [Gmail scopes](https://developers.google.com/workspace/gmail/api/auth/scopes).
An external app in Testing generally issues refresh tokens that expire after
seven days for this scope. Reauthentication during development is therefore
expected; moving to a production app and satisfying applicable verification is
part of maintaining a default client for general use.

The authorization endpoint declares `access_type=offline&prompt=consent` so a
deliberate sign-in can obtain a refresh token. Tokens remain in Core's configured
credential backend (platform keyring or its protected credential-file fallback),
not in plugin state, cursor JSON, document metadata, process arguments, or logs.

## Authenticate and verify

With a compatible Core, enabled Gmail source and real client ID:

```sh
memcastle source auth gmail
memcastle mine gmail me 'query=from:someone@example.com'
```

Complete the browser prompt with the intended mailbox. Confirm that the source
resolves to that mailbox, then run it again to check incremental behavior.
Maintainers must also observe a successful refresh after access-token expiry;
an initial sign-in alone does not prove unattended mining works. Validate this
for both the default client and the custom-client path.

To revoke access, remove the app from the Google account's third-party connections
and reauthenticate through Core when needed. A revoked/expired grant fails the
mining job without retiring stored content. Changing the OAuth client, scopes or
account is not a token migration: obtain fresh authorization.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Unknown/invalid client | Replace the development placeholder; use the Desktop client's public ID. |
| Access denied / app unavailable | Audience, test-user list, Workspace admin policy and consent-screen status. |
| Gmail API disabled | Enable Gmail API in the project owning that client. |
| Redirect mismatch / callback unreachable | Desktop client type, Core's actual loopback callback, and browser access to that listener. |
| No refresh token / sign-in stops working | Offline consent, Testing-mode expiry, revoked grants; sign in again and verify refresh. |
| Client demands a secret | Validate the Desktop registration and Core broker compatibility; do not move refresh management into the plugin. |
| Wrong mailbox | Sign in to the desired account; explicit locators must match its Gmail profile. |
| HTTP 429 / quota errors | Reduce polling/concurrency; retry. Search-intersection work uses Gmail API quota. |
| Fixture cannot obtain OAuth token | Use development Core and the manifest's `fixture_access_token = true`; no real account should be used for fixtures. |

Gmail-derived text and metadata are stored in the selected MemCastle palace.
Configured embedding/extraction services may receive mined text according to
Core's settings. Unsupported binary attachments are described, not archived;
supported textual attachments become part of the source document.
