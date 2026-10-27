# OpenObserve OSS with OIDC

[English](README.md) | [简体中文](README.zh-CN.md)

An independent public fork of [OpenObserve](https://github.com/openobserve/openobserve),
the observability platform for logs, metrics, traces, and dashboards. Thank you
to the OpenObserve authors and contributors for developing and open-sourcing
the original platform. This fork builds on upstream **v1.1.2**
(`d16d693ba99bc20621a58b948ecbcad7ec7fa882`); its fork release is **v1.1.2.1**.
It is not an official OpenObserve release or an Enterprise edition.

For the underlying platform, see the [upstream documentation](https://openobserve.ai/docs/)
and [v1.1.2 source](https://github.com/openobserve/openobserve/tree/d16d693ba99bc20621a58b948ecbcad7ec7fa882).

## What this fork adds

- Browser single sign-on with a generic OpenID Connect provider, disabled by default.
- Authorization Code flow with PKCE S256, state and nonce; server-side
  `client_secret_basic` exchange, RS256/JWKS validation, and UserInfo subject checks.
- Explicit `(issuer, sub)` bindings to existing non-root local users and an
  organization allowlist; independent, revocable HttpOnly sessions and CSRF protection.
- Source actions exposing the exact corresponding source in packaged builds.
- Local Docker packaging for Linux amd64 and arm64 binaries with the UI,
  license, source archive, build information, and SHA-256 checksums.

Native login and machine Basic/API/OTLP authentication remain separate. SSO does
**not** add Enterprise RBAC, Viewer/read-only enforcement, or automatic user
provisioning. Admitted users have this OSS edition's **Admin-level capabilities
inside allowed organizations**; approve that access before adding a binding.
Root and service-account identities are not admitted through SSO.

## Frontend ownership and packaging

This fork includes the OpenObserve UI under `web/`: its SSO action, callback/session
bootstrap, sign-out, and source download belong to this repository. The local
binary and image helpers build `web/dist/` and embed it in the OpenObserve binary;
no separate OpenObserve frontend directory needs to be deployed beside that binary.

Dispatcher's central sign-in and consent pages are a separate identity-provider
application. Their source belongs to `dispatcher_admin`, in `apps/sso/` and its
shared `packages/features/auth-shell/` dependencies. A directory named `sso-dist/`
containing `index.html`, `config.json`, and `assets/` is that application's build
output, not missing OpenObserve source. Build and publish it through the Dispatcher
frontend release process and serve it on the Provider's configured origin/path.
It is not copied into this generic OpenObserve RP or its corresponding-source archive.
Other OIDC providers supply their own sign-in UI.

The fork release lives on [`codex/oidc-v1.1.2.1`](https://github.com/aiyou178/openobserve/tree/codex/oidc-v1.1.2.1)
and tag [`v1.1.2.1`](https://github.com/aiyou178/openobserve/tree/v1.1.2.1).
The upstream default branch is named `main`; the release stays on its explicit
v1.1.2 baseline instead of upgrading to the current default branch.

## Enable OIDC

1. Run the fork binary and prepare an existing **non-root local user** with Admin
   membership in each intended existing organization. Record its immutable local
   `users.id` through your authorized metadata administration interface.
2. Register a confidential client at your identity provider:

   | Setting | Value |
   | --- | --- |
   | Flow | Authorization Code |
   | PKCE | S256 |
   | Token endpoint authentication | `client_secret_basic` |
   | Response mode | `query` |
   | Scopes | `openid profile` |
   | Exact callback | `https://observe.example.com/auth/oidc/callback` |

   The provider must publish discovery, RS256 signing keys in JWKS, and UserInfo.
   Endpoints must share the configured issuer's origin and remain below its path
   prefix. Other endpoint hosts and HTTP redirects are rejected.
3. Copy [oidc.example.json](contrib/aiyou/oidc.example.json) to a private read-only
   server mount, for example `/run/secrets/oidc.json`. Set the issuer, client ID,
   public URL, and client-secret file path. Store the secret separately in that
   mounted file, never in the repository or frontend environment.
4. Add an approved entry to `bindings`:

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

   `email` selects an existing local record; it is **not** matched against a
   claimed email to establish identity. The immutable local ID and external
   `(issuer, sub)` must both match. Empty `bindings` denies everyone.
5. Start OpenObserve with:

   ```bash
   export ZO_OIDC_ENABLED=true
   export ZO_OIDC_CONFIG_FILE=/run/secrets/oidc.json
   ./openobserve
   ```

   Use HTTPS for issuer and public URL. Neither may have a trailing slash, query,
   or fragment. For a public path prefix such as
   `https://observe.example.com/observe`, also set `ZO_BASE_URI=/observe` and
   register `https://observe.example.com/observe/auth/oidc/callback` exactly.
6. Open `/web/` under the public URL and use **SSO** on the login page. Use native
   login for root administration and separate machine credentials for ingestion.
   Do not redirect OTLP/API ingestion endpoints through interactive SSO.

Configuration and secret mounts must be readable only by the service and
authorized operators. Never commit real identities, credentials, or logs. See
the [OIDC operator guide](contrib/aiyou/oidc-integration.md) for session limits,
proxy redaction, revocation, local-only development, and troubleshooting.

## Build binaries locally

Use [build-binaries.sh](contrib/aiyou/build-binaries.sh) from a clean, committed
checkout. It builds the UI and `release-prod` OSS Rust binary in local Ubuntu
24.04 Linux containers, without enabling Enterprise or using GitHub Actions.
Docker must use a **local** Linux engine; remote Docker contexts are rejected.
Allocate at least 16 GiB of memory to Docker and keep at least 40 GiB of free
build storage per architecture. Building amd64 on an ARM host uses Docker's
Linux emulation and can be substantially slower than a native build.
The amd64 Rust steps use a process-local QEMU guest-address offset to avoid
allocator address conflicts under ARM emulation; global Docker settings and
the compiler target are unchanged.

```bash
# Default: Linux amd64
bash contrib/aiyou/build-binaries.sh

# Linux arm64, or both architectures sequentially
bash contrib/aiyou/build-binaries.sh arm64
bash contrib/aiyou/build-binaries.sh all
```

The helper archives the exact committed revision and writes packages to
`bin/oidc/` by default. It does not push an image or commit, create a release, or
deploy. Uncommitted or untracked source changes must be reviewed and committed
before building; never commit secrets to satisfy this check.

| Optional environment variable | Meaning |
| --- | --- |
| `OIDC_BUILD_OUTPUT_DIR` | Package output directory; default `bin/oidc/` under the repository |
| `OIDC_BUILD_JOBS` | Concurrent Cargo build jobs; default `1` |
| `OIDC_RUST_TOOLCHAIN` | Explicit dated nightly override, e.g. `nightly-YYYY-MM-DD` |
| `OIDC_MIN_FREE_GB` | Minimum free-space safety threshold in GiB; default `40` |

Do not lower the storage threshold or increase build parallelism without
checking Docker's available disk and memory. Docker Desktop's VM disk must also
have sufficient room; host free space alone is not enough.

Expected binary packages:

```text
openobserve-linux-amd64-v1.1.2.1.tar.gz
openobserve-linux-arm64-v1.1.2.1.tar.gz
```

Each has a `.sha256` file. Packages contain `openobserve`, `LICENSE`, both READMEs,
the OIDC guide and example under `contrib/aiyou/`, `BUILD-INFO.txt`, and the exact source archive. The binary
embeds the UI and its Source download. Check the checksum before extraction;
review `BUILD-INFO.txt` for revision, toolchain, and dynamic-library requirements.
These are **Linux GNU binaries**, not static musl, macOS, or Windows packages.
Packaging success is not deployment or OIDC acceptance evidence.

The helper uses the repository's dated Rust nightly pin. If its official
distribution is unavailable, it fails rather than silently changing toolchains.
An explicitly selected dated nightly override is recorded in the build information
and still requires compatibility validation.

For a local Linux runtime image from a clean committed snapshot:

```bash
bash contrib/aiyou/build-oidc-image.sh
# Or explicitly select one architecture:
bash contrib/aiyou/build-oidc-image.sh amd64
```

The helper defaults to the local engine's native architecture and uses the same
`release-prod` builder, pinned toolchain, single-job default, UI, license, and
exact source archive as binary packaging. It requires Docker Desktop's local
`desktop-linux` Unix socket. A read-only cache probe selects a 16 GiB starting
floor only for a native build with an existing release binary and dependency
cache for the same architecture and toolchain. Native builds without confirmed
target output use 20 GiB; emulated builds retain 40 GiB. The cached-build allowance budgets 6 GiB for rebuilt compiler output,
4 GiB for frontend/toolchain layers, 1 GiB for export/context, and a 5 GiB reserve.
Native builds without target output add 4 GiB for dependency compilation.
These are headroom policies, not measured minimum requirements. Host and
Docker free space are checked; the build is cancelled if either falls below a
5 GiB reserve. No caches are deleted, and existing output tags are rejected.
`OIDC_MIN_FREE_GB` may raise the detected floor, not lower it. The local image is tagged
`openobserve-oidc:<full-revision>-<architecture>`. It verifies the image's platform,
revision, version, and actual `--version` output without starting the server or
using the network. It does not publish or deploy. See the
[operator guide](contrib/aiyou/oidc-integration.md#source-and-local-container-build).
The upstream Cargo package version is retained; the fork release version comes
from `contrib/aiyou/VERSION`.

## License and attribution

This public OSS fork uses the [GNU Affero General Public License, version 3 or
later](LICENSE), retaining upstream copyright and license notices. The OIDC
integration is independently implemented in public OSS code; no private
Enterprise modules are enabled or included.

When distributing a modified build or serving it over a network, preserve the
notices and give users a prominent way to obtain its exact corresponding source
and build instructions. Supplied packaging embeds the source and exposes it on
the login page and authenticated Help menu. A moving branch link is not a
replacement for a distributed binary's corresponding source.

OpenObserve names and marks belong to their respective owners; this fork does
not imply their endorsement. Contributions to the original platform should
follow the [upstream contribution guide](CONTRIBUTING.md).
