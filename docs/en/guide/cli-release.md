# CLI releases and CI

## One-time repository setup

Create **Settings → Environments → cli-release**:

- Require reviewer `mivinci`.
- Disable **Prevent self-review** for the single-maintainer approval flow.
- Disable **Allow administrators to bypass configured protection rules**.
- If restricting deployment branches/tags, allow the main branch and `v*` tags.

The release workflow checks this configuration and stops if approval is missing,
self-review is blocked, or administrator bypass is enabled. It never silently
creates an unprotected environment. A repository administrator must configure
these settings if the current integration lacks Environment management access.

Under **Settings → Actions → General → Workflow permissions**, allow Actions to
create pull requests. Workflows explicitly request contents, pull-requests and
actions write permissions; no personal access token is required. Repository or
organization policy can still restrict these permissions.

## Prepare a version

Run **Prepare CLI release** manually on main with a stable `X.Y.Z` version, without
`v`, e.g. `0.3.0`. It branches from latest main as `release/cli-vX.Y.Z`, updates only
the CLI manifest and lockfile package version, and opens a version PR. Versions
must increase; existing tags/branches are never overwritten.

PRs created by GITHUB_TOKEN do not trigger pull_request workflows, so preparation
explicitly dispatches **Rust** on the version branch and both app gates to preserve
required check names without rebuilding unaffected applications. Review the PR and CI, then
merge manually. Preparation never merges or publishes automatically. If PR creation
is disabled, the branch may already exist: enable the permission, open its PR
manually and dispatch Rust; do not force-push over it.

## Approve and publish

1. After merging the version PR, wait for Rust CI on that exact main commit.
2. Manually run **CLI release** on main.
3. Validation checks manifest/lock versions, main ancestry, latest successful Rust
   CI for the exact commit, and Environment approval configuration.
4. Review the version and commit SHA in the run, then **Review deployments** to
   approve `cli-release` yourself.
5. After approval, create the immutable `vX.Y.Z` tag and directly call the shared
   four-platform build workflow. This does not depend on GITHUB_TOKEN tag pushes
   triggering another workflow.
6. Verify each built binary's `--version`, generate SHA256SUMS after all builds
   succeed, upload all assets to a draft, then publish the release.

Manual matching `v*` tag pushes remain supported and require the same main/CI and
Environment checks. For this entry point, the operator creates the tag before
approval; approval still precedes building/publishing. Later main changes cannot
alter the selected release commit.

Retry failed jobs after build/upload errors, or rerun from the same tag/commit.
Draft assets can be repaired, but published assets and existing tags are never
overwritten. If the current main commit has no Rust CI (e.g. docs-only changes),
run Rust manually on that commit before releasing.

## App impact gate

Platform and Seed Processing retain stable check names and run a lightweight gate
on PRs/main. Application sources, used shared packages, primitives/wasm and root
Cargo.toml changes trigger the corresponding build. Lock-only Rust changes compare
the complete locked q2q-wasm dependency closure, including versions, sources,
checksums and transitive edges. CLI-only version changes do not rebuild/deploy apps.

Missing baselines or ambiguous dependencies conservatively run checks. Manual runs
without a comparison base always execute; preparation supplies its base SHA for
impact gating. Unaffected runs skip installation, checks, builds and deployment,
while the stable check finishes successfully. Root npm config/lock changes remain
conservative; npm dependency closures are not yet compared.

PRs never receive deployment credentials. Only affected main push/manual runs deploy.
App workflow entries still appear on GitHub for unaffected changes, with only the
lightweight gate executed.
