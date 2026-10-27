# OpenObserve OSS with OIDC

[English](README.md) | [简体中文](README.zh-CN.md)

An independent fork of [OpenObserve](https://github.com/openobserve/openobserve),
the open-source observability platform for logs, metrics, traces, and dashboards.
Thank you to the upstream authors and contributors for building and sharing OpenObserve.
This fork adds **generic OpenID Connect (OIDC) single sign-on** to the OSS edition.
It is not an official OpenObserve release and does not enable private Enterprise code.

**Fork version:** [`v1.1.2.3`](https://github.com/aiyou178/openobserve/tree/v1.1.2.3)

**Upstream baseline:** [`v1.1.2` / `d16d693`](https://github.com/openobserve/openobserve/tree/d16d693ba99bc20621a58b948ecbcad7ec7fa882)

**Release branch:** [`codex/oidc-v1.1.2.3`](https://github.com/aiyou178/openobserve/tree/codex/oidc-v1.1.2.3)

For the underlying platform, see the [OpenObserve documentation](https://openobserve.ai/docs/).

## What this fork adds

- Optional browser SSO with a standards-based OIDC identity provider; disabled by default.
- Authorization Code flow with PKCE S256, state and nonce, `client_secret_basic`,
  RS256/JWKS validation, and matching ID Token/UserInfo subjects.
- First-login user creation keyed by a durable `(issuer, sub)` identity, with configurable default organizations.
- Optional explicit bindings for existing non-root local users.
- Independent, revocable HttpOnly browser sessions, CSRF protection, and current-membership checks.
- SSO login, session bootstrap, sign-out, expiry handling, and Source download in the OpenObserve UI.
- Local Linux amd64/arm64 packaging with the embedded UI, exact corresponding source,
  license notices, build information, and SHA-256 checksums.

Native login and machine Basic/API/OTLP credentials remain separate. SSO does not
automatically create organizations, merge accounts by email, or add Enterprise RBAC.
Admitted users have this OSS edition's **Admin-level capabilities inside allowed organizations**;
approve that access before enabling automatic provisioning or adding a binding. Root and service accounts cannot use SSO.

## Enable OIDC

1. Register a confidential client with your identity provider using Code flow,
   PKCE S256, `client_secret_basic`, query response mode, and `openid profile`.
   Register the exact callback, for example `https://observe.example.com/auth/oidc/callback`.
2. Create the organizations that new OIDC users should join. Set their exact IDs in
   `auto_provision.default_organizations`; new accounts receive this OSS edition's Admin role.
3. Copy [oidc.example.json](contrib/aiyou/oidc.example.json) to a private server file,
   such as `/run/secrets/oidc.json`. Configure issuer, client ID, public URL,
   the separate client-secret file, and `auto_provision.default_organizations`.
   Set `allow_oss_admin: true` to approve the OSS privilege level. Per-user bindings
   are unnecessary. Omitting `auto_provision` retains the optional explicit-binding mode.
4. Enable the feature and start OpenObserve:

   ```bash
   export ZO_OIDC_ENABLED=true
   export ZO_OIDC_CONFIG_FILE=/run/secrets/oidc.json
   ./openobserve
   ```

5. Open `/web/` and select **SSO**. Keep native login for root administration and
   independent machine credentials for ingestion.

Use HTTPS and exact issuer/callback URLs. Keep configuration and credentials out
of the repository and frontend assets. For path prefixes, session limits, approved
provisioning, optional bindings, proxy privacy, and recovery, see the [OIDC operator guide](contrib/aiyou/oidc-integration.md).

## Frontend and build

All OpenObserve frontend changes are included in this repository under `web/`.
The packaging helpers build `web/dist/` and embed it in the binary, including the
SSO controls and Source download. There is no separate OpenObserve UI bundle to deploy.
The identity provider supplies its own authentication and consent pages.

On first login the server atomically creates an external user, its default
organization memberships, and the persistent identity mapping. Later logins use
that mapping even if the provider email changes, and never restore removed
memberships. Verified email may initialize the new profile; absent or unverified
email uses a stable `@oidc.invalid` identifier. An email collision denies creation
rather than linking an existing account. No usable password or machine token is issued.

From a clean, committed checkout with a local Docker Linux engine:

```bash
# Linux amd64
bash contrib/aiyou/build-binaries.sh amd64

# Linux arm64, or both architectures
bash contrib/aiyou/build-binaries.sh arm64
bash contrib/aiyou/build-binaries.sh all

# Local runtime image; defaults to the engine's native architecture
bash contrib/aiyou/build-oidc-image.sh
```

Binary packages are written to `bin/oidc/` by default and include `BUILD-INFO.txt`,
licenses, documentation, exact source, and a `.sha256` file. The fork version comes
from `contrib/aiyou/VERSION`; the upstream Cargo package version is retained.
These are Linux GNU binaries, not static musl, macOS, or Windows builds.

Builds use the repository's pinned Rust nightly. An unavailable pin stops the build;
any explicit dated-toolchain override is recorded and needs compatibility validation.
Check host and Docker disk/memory before building: binary packaging defaults to a
40 GiB free-space floor and one Cargo job; image packaging applies its documented
cache-aware floor. The helpers do not publish or deploy.
See the [build instructions](contrib/aiyou/oidc-integration.md#source-and-local-container-build)
for prerequisites, overrides, source provenance, and runtime dependencies.

## Upstream attribution and license

The original platform is [OpenObserve](https://github.com/openobserve/openobserve).
We thank its authors and contributors and preserve the upstream copyright notices
and [license](https://github.com/openobserve/openobserve/blob/d16d693ba99bc20621a58b948ecbcad7ec7fa882/LICENSE).
This fork is distributed under the [GNU Affero General Public License, version 3 or later](LICENSE).
OpenObserve names and marks belong to their respective owners; this fork does not imply endorsement.

Packaged builds provide the exact corresponding source and build instructions through
the Source action on the login page and authenticated Help menu. Preserve that archive
and the license notices when distributing a modified build or providing network access.
A moving branch link is not a substitute for the exact source of a distributed binary.
