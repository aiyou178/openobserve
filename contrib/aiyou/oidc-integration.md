# OIDC operator guide

This public fork adds an independently implemented OpenID Connect relying party
to OpenObserve OSS. It is based on upstream v1.1.2
(`d16d693ba99bc20621a58b948ecbcad7ec7fa882`), with fork release v1.1.2.3.
It does not enable private Enterprise modules or replace the upstream license.
See [the fork README](../../README.md) and [中文说明](../../README.zh-CN.md).

## Provider requirements

Configure one trusted issuer and a pre-registered confidential client:

- Discovery: `${issuer}/.well-known/openid-configuration`.
- Authorization Code flow, mandatory PKCE S256, state and nonce.
- `response_mode=query`. This RP does not implement `form_post` or silent fallback.
- `client_secret_basic` for the server-side, form-urlencoded token request,
  including the exact redirect URI and `code_verifier`.
- RS256 ID Tokens with `kid`, exact `iss`, string `aud` matching the client ID,
  valid `exp`/`iat`, stable `sub`, and the transaction's nonce.
- A discoverable JWKS endpoint and Bearer UserInfo endpoint. UserInfo `sub` must
  match the ID Token; email and groups are not used to grant access.

The RP requests `openid profile` for both explicit bindings and automatic provisioning.
Discovery lists provider capabilities, not the scopes granted to this client, so
advertising `email` does not add it to authorization requests. Providers without
email claims remain supported. The RP does not request `offline_access`, retain
provider tokens in sessions, refresh tokens, or implement provider-wide logout.
An email claim or `email_verified=true` is not required for admission.

The issuer must have no trailing slash, query, or fragment. Every discovered
endpoint must have the same origin and be below the issuer's path prefix, with
no query. Redirects are denied; TLS verification is always enabled. Each outbound
request has a 10-second deadline, a 3-second connection timeout, and a 64-KiB
response limit. Providers using different hosts for protocol endpoints are not
supported by this configuration model.

## Login UI ownership

The RP frontend is this repository's `web/` application. Both local packaging
helpers build it and embed `web/dist/` in the binary. Authentication and consent
pages belong to the identity provider and are built and published independently.
The RP redirects to the configured provider and does not bundle a provider-specific
sign-in application or environment-specific runtime configuration.

## Server configuration

The feature is off by default. Enable it with:

```bash
export ZO_OIDC_ENABLED=true
export ZO_OIDC_CONFIG_FILE=/run/secrets/oidc.json
```

Copy [oidc.example.json](oidc.example.json) to that private server path and replace
the placeholders. The example enables automatic provisioning into the existing
`default` organization with explicit OSS Admin approval. Change the organization
IDs before enabling SSO. Omitting `auto_provision` disables automatic creation;
only optional explicit `bindings` can then admit identities. The JSON and
separate client-secret file must never be included in
web assets, source archives, images, logs, or the repository. Use private
secret-manager mounts, restrictive permissions such as 0600, and atomic
replacement. The server rereads configuration and the secret rather than
embedding them in the frontend.

| Field | Meaning |
| --- | --- |
| `issuer` | Exact trusted provider issuer, e.g. `https://idp.example.com/oidc` |
| `client_id` | Registered confidential client ID |
| `client_secret_file` | Server-only file containing the client secret |
| `public_url` | Browser-visible OpenObserve origin and optional base path, without a trailing slash |
| `allow_insecure_loopback` | Default `false`; only for disposable loopback HTTP development |
| `idle_seconds` | Default 1800; accepted range 60–86400 (24 hours) |
| `absolute_seconds` | Default 28800; accepted range 60–604800 (seven days); must be at least `idle_seconds` |
| `auto_provision` | Optional first-login provisioning policy; omit to disable it |
| `auto_provision.default_organizations` | 1–100 distinct existing active organization IDs; never `_meta` |
| `auto_provision.allow_oss_admin` | Must be `true` to approve organization Admin capabilities |
| `bindings` | Optional explicit external-to-local bindings; maximum 1000; take precedence for listed subjects |

HTTPS is required for issuer and public URL except the explicitly enabled
loopback development case below. Neither URL may contain credentials, a query,
or a fragment. `public_url`'s path must match `ZO_BASE_URI` exactly after trimming
the final slash.

For `public_url=https://observe.example.com`, register precisely:

```text
https://observe.example.com/auth/oidc/callback
```

For `public_url=https://observe.example.com/observe` and `ZO_BASE_URI=/observe`,
register precisely:

```text
https://observe.example.com/observe/auth/oidc/callback
```

Register only actual exact callbacks at the provider; do not use wildcards,
prefix matching, or a frontend-only callback. Preserve the external scheme,
host, and path when configuring your reverse proxy. The configured URL, not
untrusted request headers, determines the redirect URI and trusted Origin.

## Automatic first-login provisioning

Configure one policy for all identities authenticated by the trusted issuer:

```json
"auto_provision": {
  "default_organizations": ["default"],
  "allow_oss_admin": true
}
```

No per-user bindings are required. After successful ID Token and matching UserInfo
validation, the server atomically creates an external non-root user, Admin
memberships in the configured existing active organizations, and an immutable
`(issuer, sub)` to `users.id` mapping. A missing, inactive, deleted, or reserved
organization rolls the whole creation back. This OSS edition does not implement
read-only or Enterprise RBAC roles; enabling this policy admits every authenticated
subject from the configured client at the documented Admin privilege level.

Only a syntactically valid UserInfo email with `email_verified=true` initializes
the local email; otherwise a stable identifier under `@oidc.invalid` is generated
from the issuer and subject. Optional profile names initialize display fields.
An existing email is never linked or overwritten, even if verified; an email
collision denies creation. Use the explicit binding mode only when intentionally
linking an existing account. Provider groups never grant roles.

The persistent mapping, not email, resolves subsequent logins, including after a
provider email change or server restart. Concurrent logins create one account.
Default memberships are assigned only at creation: changing defaults affects
future users, and logging in never restores a removed membership. Local Admin
memberships, including organizations subsequently granted by an administrator,
are rechecked on each request; inactive/deleted organizations and `_meta` are
excluded. Removing all memberships denies access. Deleting a local account leaves
the mapping as a tombstone, so neither login nor a replacement account with the
same email can resurrect its access. Removing `auto_provision` denies all sessions
created in that mode; existing configured bindings remain independent.

Provisioned accounts have no usable native password and no enabled static machine
token or RUM credential. User and membership caches are refreshed after login.
The `oidc_identities` table is initialized in the primary metadata database on
first use, like the existing OIDC transaction/session table. Preserve both tables
and ordinary user/organization metadata in backups; database write credentials
need the same table-creation permissions required by the existing OIDC store.

## Optional binding to an existing identity

Create the local user and organization memberships through the normal authorized
OpenObserve administration workflow before adding a binding. Obtain the user's
immutable `users.id` through your authorized metadata administration interface.
Do not edit existing user records to make an example match.

An entry in `bindings` has this shape:

```json
{
  "issuer": "https://idp.example.com/oidc",
  "subject": "STABLE_OPAQUE_SUBJECT_FROM_YOUR_PROVIDER",
  "email": "existing-user@example.com",
  "user_id": "EXISTING_IMMUTABLE_LOCAL_USER_ID",
  "allow_oss_admin": true,
  "organizations": ["EXISTING_ORGANIZATION_ID"]
}
```

`subject` is the stable, opaque `sub` issued to this client. The external
`(issuer, sub)` is the identity key; email/profile/group claims cannot create or
merge accounts. The configured `email` locates an existing local record and its
immutable `user_id` must also match. Deleting and recreating a user with the same
email does not preserve access. Duplicate external identities are rejected.

**Approve the actual OSS privilege level before setting `allow_oss_admin=true`.**
This OSS edition has Admin semantics, including write and user-management
capabilities inside admitted organizations. It does not enforce Enterprise
Viewer/read-only, stream-level ACLs, or Enterprise RBAC. Only current local Admin
memberships are admitted; Root users and Viewer/Editor/User/service-account
memberships are denied. Claimed groups do not grant roles. Explicit bindings
do not create accounts or memberships. Automatic provisioning
creates users only under the separate policy above; neither mode creates
organizations or Root accounts.

Each SSO request reads current user and active organization membership from the
primary metadata writer. In explicit-binding mode it also intersects memberships
with the current binding's organization allowlist. The organization-list API
is filtered on the server;
root/global organization management APIs are denied. Changes apply at the next
request's authorization decision; already running requests may finish. This is
not a guarantee of immediate cross-system revocation.

## Sessions, persistence, and logout

The RP creates independent opaque `o2_oidc_session` and browser-correlation
cookies. They are host-only, HttpOnly, SameSite=Lax, and Secure with an HTTPS
public URL. The session has bounded idle and absolute expiry; it does not
contain local passwords, provider access tokens, or ID Tokens. Local logout
revokes this RP session and clears local browser state, not the provider's
session or sessions in other applications.

When enabled, the RP lazily creates only the `oss_oidc_records` metadata table
through the shared SeaORM writer. Disabled mode does not create it. All replicas
must share the same metadata database and configuration; clustered SQLite is
not supported. Handles are SHA-256 indexed. Nonce, verifier, callback and return
path stay server-side in five-minute transactions. A conditional SQL update
consumes a browser-bound transaction once, including across replicas; a failed
exchange burns it. Expiry, session touch and revocation use the primary writer.
Metadata backups contain short-lived transaction secrets and require protection.
Expired rows are removed when creating new transactions; a fully idle deployment
requires operator metadata maintenance, not a promised background cleanup.

Browser routes below are relative to the configured `ZO_BASE_URI`:

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/auth/oidc/bootstrap` | Server-authenticated feature/session/profile bootstrap and CSRF proof |
| POST | `/auth/oidc/login` | Start a browser-bound login transaction |
| GET | `/auth/oidc/callback` | Receive query response and clear the visible callback URL |
| POST | `/auth/oidc/complete` | Same-origin completion and server-side token exchange |
| POST | `/auth/oidc/logout` | Revoke the local RP session |

The shipped frontend handles bootstrap, CSRF and native/SSO account switching.
Profile data in localStorage is not authentication evidence. Cookie writes
require exact configured Origin and `x-o2-csrf`, except anonymous bootstrap's
browser-binding cookie, which requires `x-o2-bootstrap: 1` and rejects a foreign
Origin. Browser API writes require CSRF; WebSocket upgrades require the configured
Origin. Preserve these headers in the proxy; do not expose a permissive
cross-origin credentialed bypass.

With explicit Authorization, the normal machine credential validator takes
precedence. Otherwise, presence of the SSO session cookie selects SSO even if
malformed or expired: it cannot fall back to a stale local Basic cookie. Old
`auth_tokens`/`auth_ext` cookies are cleared on SSO establishment and logout.
SSO cookies never authorize ingestion or OTLP. Existing machine Basic/token
authentication remains independent and returns authentication errors, not
browser login redirects.

A provider outage blocks new logins; existing local sessions continue until
local expiry, revocation, or a denied binding/membership decision. Disabling
OIDC denies new logins and existing application sessions. Keep the trusted
configuration mounted while residual browser cookies exist: bootstrap and
CSRF-protected logout can still revoke records and clear cookies after
disablement, binding removal, or membership removal. Then use native login.

## Proxy and telemetry safeguards

The query-mode callback briefly contains a short-lived authorization code. It
immediately replaces browser history with a clean URL before the same-origin
completion request. Responses use no-store/no-referrer and a per-response CSP;
no external assets are loaded from that callback page. Tokens, client secrets,
and verifiers must never appear in URLs.

The application omits OIDC query values from access/slow logs, disables HTTP
tracing on OIDC routes, and redacts Authorization, Cookie, CSRF, Location, and
Set-Cookie headers. **Also configure every ingress, proxy and external telemetry
system** to log paths only and avoid request/response header or body capture on
`/auth/oidc/` and the provider's protocol paths. Application redaction cannot
control those systems. Never enable raw HTTP wire/debug capture on these routes.

Do not forward client-supplied identity headers as authenticated identities, and
do not replace API/OTLP authentication with interactive login redirects.

## Source and local container build

The local [binary build helper](build-binaries.sh) is the packaging path; there
is no fork binary-packaging GitHub Action. It builds the UI and `release-prod`
OSS binary inside Ubuntu 24.04 Linux containers, using the dated Rust toolchain
without enabling Enterprise. Start from a clean, committed checkout and use a
local Docker Linux engine; remote Docker contexts are rejected. Allocate at
least 16 GiB of Docker memory and 40 GiB of free build storage per architecture.
Docker Desktop's VM disk also needs that space. Cross-architecture builds use
Docker emulation and can be substantially slower than native builds.

```bash
bash contrib/aiyou/build-binaries.sh amd64
bash contrib/aiyou/build-binaries.sh arm64
# Or build both sequentially:
bash contrib/aiyou/build-binaries.sh all
```

The default architecture is `amd64`. Packages are written to repository
`bin/oidc/`, or `OIDC_BUILD_OUTPUT_DIR` when specified. `OIDC_BUILD_JOBS` defaults
to `1`; `OIDC_MIN_FREE_GB` defaults to the `40` GiB free-space safety threshold.
Do not reduce that threshold or raise parallelism without checking Docker's
disk and memory capacity. Never commit credentials to satisfy the clean-tree
requirement. The helper does not push, publish, or deploy.

Packages include SHA-256 checksums, `LICENSE`, both READMEs, this guide,
`BUILD-INFO.txt`, and a `git archive` of the exact revision. The helper actually
runs the Linux binary's `--version` and verifies it against
`contrib/aiyou/VERSION`; review the recorded revision, architecture, toolchain,
and dynamic-library requirements before distribution. These packages target
Linux GNU, not macOS or static musl. See the
[README](../../README.md#build-binaries-locally).

For a local runtime image, start from a clean committed checkout:

```bash
bash contrib/aiyou/build-oidc-image.sh
# Explicit architecture (the default is the engine's native architecture):
bash contrib/aiyou/build-oidc-image.sh arm64
```

This helper rejects uncommitted/untracked source, archives `HEAD`, and uses the
`runtime` target of `contrib/aiyou/Dockerfile.binaries`, sharing its UI and
`release-prod` OSS builder. It only uses Docker Desktop's local `desktop-linux`
Unix socket. A read-only cache probe checks for the same architecture/toolchain's
release binary and dependency directory. Confirmed native cached builds use a
16 GiB starting floor; native builds without confirmed target output use 20 GiB,
and emulated builds keep 40 GiB. These
are headroom allowances, not measured minimum requirements. Host and Docker
filesystem free space are checked before building and every 15 seconds during
the build; falling below a 5 GiB reserve cancels only this build without deleting
data. `OIDC_MIN_FREE_GB` can raise, not lower, the detected floor. The helper never
overwrites an existing output tag. `OIDC_BUILD_JOBS` defaults to `1`.
The local tag is `openobserve-oidc:<full-revision>-<architecture>`. Image platform,
revision/version labels, and actual binary `--version` are checked. The runtime
image also checks `--version` against its installed shared libraries during the
build. No server is started, and the post-build check has no network access.
It does not publish or deploy, and never packages a host macOS executable.
The image retains `BUILD-INFO.txt` under `/usr/share/openobserve`, license under
`/usr/share/licenses/openobserve`, operator documents under
`/usr/share/doc/openobserve`, and the exact source archive under
`/usr/share/openobserve`.
If the pinned official Rust distribution is unavailable, the build is blocked.
An explicit `OIDC_RUST_TOOLCHAIN=nightly-YYYY-MM-DD` override must be treated as a
separate compatibility decision and validated before distribution.

Packaged UI builds expose the exact corresponding source through Source actions
on the login page and authenticated Help menu. Source archive names include the
version and commit revision; local image tags include the revision and architecture.
Retain the immutable archive and license with each distribution.
A development branch link alone is not a replacement for the corresponding
source of a distributed binary. Preserve upstream notices and do not include
secrets, user records, private modules, or telemetry in source archives.

## Disposable local verification and troubleshooting

Only for isolated development, set `allow_insecure_loopback=true`, use a loopback
HTTP issuer such as `http://127.0.0.1:5556/oidc`, and set
`public_url=http://127.0.0.1:5080`. Register the separate exact callback
`http://127.0.0.1:5080/auth/oidc/callback`. Use disposable metadata, an existing
disposable non-root Admin user/organization, and a disposable provider client.
Never enable insecure loopback settings for a public deployment. Check root
`.env` before starting the server because it overrides process environment.

Before a deployment, verify a complete login/logout using your actual provider,
then verify first-login provisioning, concurrent/restarted login, email changes
and collisions, default organization rollback, and denial of an unmapped identity
when automatic provisioning is disabled, wrong issuer/audience/signature/nonce,
replayed callback, expired session, missing CSRF, denied organization, Root
identity, removed membership, and removed binding. Confirm server-side
organization-list filtering, recovery logout after disablement, parallel-tab
behavior, and native login/machine OTLP compatibility. Validate shared-database
behavior for the actual replica topology; a SQLite-only local check is not
evidence for PostgreSQL or multi-server operation.

`verify-oidc-browser.py` is a mutation-capable disposable integration harness with
provider-specific UI fixtures, not a universal OIDC conformance test. Adapt its
login/consent fixtures to your own test provider before using it. Never run it
against production, shared persistent data, or real user credentials.

If SSO is unavailable, check the enabled flag, mount permissions, public URL/base
path, exact callback, discovery capabilities and endpoint-origin/path policy.
If login completes at the provider but the RP denies access, check the stable
subject, current Admin membership, provisioning policy and `allow_oss_admin`
approval. For explicit bindings, also check immutable local user ID and allowlist.
Inspect only redacted errors; do not dump token
bodies, callback queries, cookies, configuration secrets, or wire traces.

These instructions describe implemented behavior, not a claim that a particular
release binary has passed CI, provider interoperability, production TLS/proxy
redaction, deployment, or performance acceptance. Record those results
separately for the exact built revision.
