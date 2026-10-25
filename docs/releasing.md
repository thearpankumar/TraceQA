# Releasing

A release publishes the `traceqa` binary for macOS, Windows and Linux on the
repository's Releases page, under a version number. It is started by pushing a
version tag, and everything after that is automatic.

## Version numbers

TraceQA uses semantic versioning: `MAJOR.MINOR.PATCH`.

| Part | Raise it when | Example |
|---|---|---|
| PATCH | Only bugs are fixed | `0.1.0` to `0.1.1` |
| MINOR | Features are added and nothing breaks for users | `0.1.1` to `0.2.0` |
| MAJOR | Something changes in a way that breaks users | `0.9.0` to `1.0.0` |

Versions below `1.0.0` are early and may still change. Their releases are
marked as pre-releases on GitHub.

Every package carries the same version: the Rust workspace (`Cargo.toml`), the
four Python packages and the TypeScript package. `scripts/check_version.py`
checks that they agree, in CI on every pull request and again when a release is
tagged.

## Making a release

1. **Set the version**, in a normal pull request, if it changes. Edit it in all
   six files and run `python3 scripts/check_version.py` to confirm they agree:

   | File | Field |
   |---|---|
   | `Cargo.toml` | `version` under `[workspace.package]` |
   | `python/traceqa-protocol/pyproject.toml` | `version` |
   | `drivers/playwright-py/pyproject.toml` | `version` |
   | `drivers/webdriver-py/pyproject.toml` | `version` |
   | `sdks/python/pyproject.toml` | `version` |
   | `sdks/typescript/package.json` | `"version"` |

   The three Python packages that depend on `traceqa-protocol` pin it exactly
   (`traceqa-protocol==X.Y.Z` in `dependencies` of `drivers/playwright-py`,
   `drivers/webdriver-py` and `sdks/python`), so change those pins too. The drivers
   report the version of their installed package, so no version is written in
   their code. `check_version.py` fails on a pin that disagrees, on a literal
   `VERSION` in a driver, and on a Cargo path dependency that carries a version.

   Then run `cargo build` and `uv lock`, so `Cargo.lock` and `uv.lock` carry the
   new version too, and merge the pull request.

2. **Check Safari by hand**, on a Mac with "Allow remote automation" switched on
   in Safari's Developer settings. CI cannot run Safari.

   ```sh
   TRACEQA_TEST_SAFARI=1 uv run pytest drivers/webdriver-py collector
   ```

3. **Tag the merged commit** and push the tag:

   ```sh
   git checkout master
   git pull
   git tag -a v0.2.0 -m "TraceQA 0.2.0"
   git push origin v0.2.0
   ```

   Pushing a branch never sends its tags, so the tag is pushed by name.

4. **Wait for the Release workflow** under the repository's Actions tab. When
   it finishes, the release is on the Releases page.

The same can be done without a terminal: on GitHub, open Releases, then Draft a
new release, type the tag (for example `v0.2.0`), choose `master` as the
target and publish. GitHub creates the tag, and the workflow runs the same way.

## What the workflow does

| Job | What it does |
|---|---|
| Check the version | Fails unless the tag points at a commit on `master` and is `v` followed by the version in every package. A tag that does not match never ships. |
| Audit the dependencies | Runs `cargo audit` (known vulnerabilities) and `cargo deny check` (licences, banned crates, sources, see `deny.toml`). A failure stops the release before any binary is built. |
| Build | Builds the release binary on Linux, macOS and Windows with the compiler named by `RUST_TOOLCHAIN` in the workflow, sets `TRACEQA_RELEASE_COMMIT` to the tagged commit (`github.sha`) so the installer's pinned commit never depends on tree state, checks that it starts and reports the tagged version, and packages it with the README and the licence. It can read the repository and nothing else. |
| Publish | Runs in the `release` environment. Writes `SHA256SUMS.txt`, signs a build provenance attestation for each archive and for `SHA256SUMS.txt`, and creates the GitHub release with the three archives, release notes generated from the merged pull requests, and the pre-release mark below `1.0.0`. This is the only job that can write. |

The published files:

| File | For |
|---|---|
| `traceqa-v0.2.0-linux-x86_64.tar.gz` | Linux on Intel or AMD processors |
| `traceqa-v0.2.0-macos-arm64.tar.gz` | Macs with Apple processors |
| `traceqa-v0.2.0-windows-x86_64.zip` | Windows |
| `SHA256SUMS.txt` | Checksums, to confirm a download is complete and unchanged |

## Verifying a download

Every archive and `SHA256SUMS.txt` carries a build provenance attestation: a
statement signed through GitHub that this repository's Release workflow built
that exact file. With the GitHub CLI:

```sh
gh attestation verify traceqa-v0.2.0-linux-x86_64.tar.gz --repo thearpankumar/TraceQA
sha256sum -c SHA256SUMS.txt --ignore-missing
```

The first command proves where the file came from; the second proves the
download is complete. To report a vulnerability, see `SECURITY.md`.

The first release made with this workflow is the first real test of the tag
path, which cannot be run without tagging. After it, run both commands above on
all three archives and fix the workflow before the next release if anything fails.

### Release setup the workflow cannot do

- Add required reviewers to the `release` environment (Settings, Environments).
  The workflow creates the environment on first use without any protection.
  Also limit it to the `v*` tags.
- Raise `RUST_TOOLCHAIN` (top of `release.yml`) in a pull request when a new
  stable compiler is wanted. Run `rustc --version` locally with that version,
  and `cargo test --locked`, first.
- `rust-version` in the root `Cargo.toml` is the oldest compiler the project builds
  with, and the `msrv` job in `ci.yml` builds with exactly that version. Raise it
  deliberately, in its own pull request, when a dependency or the code needs a
  newer compiler, and never lower it without building with the lower version first
  (`cargo +<version> check --workspace --locked`). The `msrv` job fails on its own
  when something starts needing a newer one.
- The workspace is on Rust edition 2021 on purpose. Edition 2024 needs only Rust 1.85, so
  the MSRV is no obstacle, but switching also changes the default rustfmt style (a reformat
  of most files), makes `clippy::collapsible_if` ask for let chains throughout, and shifts the
  drop order of temporaries in `run.rs`. That is a large diff with no behaviour gain. Revisit it
  in a pull request of its own: `cargo fix --edition`, set `edition = "2024"` in the root
  `Cargo.toml`, then `cargo fmt --all` and fix what clippy reports.
- Lints live in `[workspace.lints]` in the root `Cargo.toml` (every crate opts in with
  `[lints] workspace = true`) and `clippy.toml`. CI runs clippy with `-D warnings`, so a
  `warn` there gates. Every crate has `publish = false` until a publish plan exists; with it
  `cargo deny` can enforce `wildcards = "deny"`.
- Tags must be on `master`. A tag on any other commit fails the first job.

## Code signing and drivers

The binaries are not code-signed yet. macOS quarantines a downloaded binary
until `xattr -d com.apple.quarantine traceqa` is run, and Windows SmartScreen
may ask for confirmation. Signing needs an Apple Developer account and a
Windows code-signing certificate.

A released binary installs its drivers from its own tag: `traceqa install`
in version `0.2.0` fetches the drivers of `v0.2.0`, so the two always match.

## Fixing a mistake

| Problem | What to do |
|---|---|
| The workflow failed at "Check the version" | Nothing was published. Delete the tag (below), fix the version in a pull request, then tag again. |
| A release went out with a bug | Do not replace it. Fix the bug and release the next PATCH version. |
| A tag was pushed by mistake, and its release was not used | Delete the release on GitHub, then the tag: `git tag -d v0.2.0` and `git push origin --delete v0.2.0`. |

## Merging and releasing are separate

Merging a pull request puts code on `master` and publishes nothing. A release
happens only when a tag is pushed, so several pull requests can be merged and
then released together under one version.

## Pinned actions

Every action in the workflows is pinned to a full 40-character commit SHA, with
the release it belongs to in a trailing comment (`# v4.4.0`). A tag can be moved
to different code; a commit cannot. `dtolnay/rust-toolchain` has no release
tags, so its pin is a commit of `master` (comment `# master`) and each use names
its toolchain with `with: toolchain:`. Dependabot proposes new SHAs in pull
requests, each one a reviewable diff.

To check a SHA by hand, resolve the tag in the action's own repository and compare:

```sh
gh api repos/actions/checkout/commits/v4.4.0 --jq .sha
```

`commits/<tag>` works for annotated and lightweight tags. Do not take a SHA
from a fork. `zizmor` with a GitHub token (`GH_TOKEN`) runs the online
`impostor-commit` check that catches this.

## Package checks

The Lint job runs `npm run lint:package` in `sdks/typescript`: it builds, then
`publint --strict` checks `package.json` (exports, types, files) and
`attw --pack . --profile esm-only` checks the packed tarball's types for an ESM
only package. `prepublishOnly` runs the type check and the build, so a publish
from a clean checkout cannot ship a stale `dist/`. Every published package
carries a copy of the root `LICENSE` (`tests/test_package_licenses.py`).

## Dependency audits and updates

The Audit workflow (`.github/workflows/audit.yml`) runs every Monday, on demand,
and whenever `Cargo.lock`, `uv.lock`, `sdks/typescript/package-lock.json` or
`deny.toml` change. It always uses GitHub-hosted machines and can only read.
It runs `cargo audit`, `cargo deny check` (policy in `deny.toml`: licences,
sources, duplicate and wildcard versions), `npm audit --audit-level=high` and
`pip-audit` on the exported `uv.lock`. A new licence in the tree fails the
check; decide, then add it to `deny.toml` with a comment. Run the same checks
locally:

```sh
cargo audit && cargo deny --locked check
(cd sdks/typescript && npm audit --audit-level=high --package-lock-only)
uv export --frozen --no-hashes --no-emit-workspace --no-header > requirements.txt
uvx pip-audit==2.10.1 -r requirements.txt --no-deps --disable-pip
```

`.github/dependabot.yml` opens weekly update pull requests for Cargo, Python
(`uv`), npm and GitHub Actions, with minor and patch updates grouped and a
seven-day wait for new releases. Dependabot configuration errors show only on
GitHub (Insights, Dependency graph, Dependabot); check there after changing it.

`traceqa map` fetches robots.txt with `ureq` (rustls, no other features). Its TLS stack brings
three licences beyond MIT and Apache-2.0: ISC (`ring`, `rustls-webpki`, `untrusted`), BSD-3-Clause
(`subtle`) and CDLA-Permissive-2.0 (`webpki-roots`). They are listed in `deny.toml` with that
comment, exactly as `cargo deny` reported them; remove them together if the dependency goes.
`.github/CODEOWNERS` names the owner for the workflows, `deny.toml`, scripts, the
MCP server and the lock files.

### Repository settings the files cannot make

Owner actions, under Settings:

- Enable private vulnerability reporting (Code security), which `SECURITY.md` points to.
- Enable Dependabot alerts and Dependabot security updates.
- Protect `master`: require the `CI` check and review from code owners.

## Self-hosted runner checklist

CI can run on a self-hosted machine, named by the repository variable
`RUNNER_LABELS` (a JSON array such as `["self-hosted","linux"]`). The workflow
sends a job there only for pushes to `master` and for pull requests from this
repository that Dependabot did not open. Pull requests from forks, Dependabot
pull requests, and every run when the variable is unset use GitHub-hosted
`ubuntu-latest`. The `Changes` job decides this once, and it always runs on a
GitHub-hosted machine. A persistent machine keeps state between jobs, so
GitHub advises against self-hosted runners for public repositories.

Even a same-repository pull request can carry changed build scripts, and the
machine keeps its disk between jobs. So on the pool a pull request run works
in its own tool homes (Cargo, rustup, uv, npm, Playwright browsers, TraceQA's
home) under the runner's temporary folder, set by the first step of the lint
and test jobs, and a last step removes `target/`, `.venv` and `node_modules`.
Pushes to `master` keep the shared folders. Without this, a pull request could
leave a changed toolchain or cache for a later `master` run. The Rust cache is
saved only by pushes to `master`; GitHub already scopes a pull request's cache
writes to its own merge ref, so that is a second layer. Ephemeral runners
(below) make all of this unnecessary.

The workflow cannot enforce the settings outside the repository. These are
owner actions:

- Use ephemeral (just-in-time) runners, one job per machine, so nothing
  survives a job. This is the real fix for cross-run poisoning.
- On AWS, require IMDSv2 with a hop limit of 1 (or block the metadata
  service), and attach no instance role to the machine.
- Limit the runner group to this repository, and do not give it to forks.
- Keep the fork pull request approval policy at "Require approval for all
  external contributors".
