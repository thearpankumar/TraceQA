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
| Check the version | Fails unless the tag is `v` followed by the version in every package. A tag that does not match never ships. |
| Build | Builds the release binary on Linux, macOS and Windows, checks that it starts and reports the tagged version, and packages it with the README and the licence. |
| Publish | Writes `SHA256SUMS.txt` and creates the GitHub release with the three archives, release notes generated from the merged pull requests, and the pre-release mark below `1.0.0`. |

The published files:

| File | For |
|---|---|
| `traceqa-v0.2.0-linux-x86_64.tar.gz` | Linux on Intel or AMD processors |
| `traceqa-v0.2.0-macos-arm64.tar.gz` | Macs with Apple processors |
| `traceqa-v0.2.0-windows-x86_64.zip` | Windows |
| `SHA256SUMS.txt` | Checksums, to confirm a download is complete and unchanged |

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

## Self-hosted runner checklist

CI can run on a self-hosted machine, named by the repository variable
`RUNNER_LABELS` (a JSON array such as `["self-hosted","linux"]`). The workflow
sends a job there only for pushes to `master` and for pull requests from this
repository that Dependabot did not open. Pull requests from forks, Dependabot
pull requests, and every run when the variable is unset use GitHub-hosted
`ubuntu-latest`. The `Changes` job decides this once, and it always runs on a
GitHub-hosted machine. A persistent machine keeps state between jobs, so
GitHub advises against self-hosted runners for public repositories.

The workflow cannot enforce the settings outside the repository. These are
owner actions:

- Use ephemeral (just-in-time) runners, one job per machine, so nothing
  survives a job. This is the real fix for cross-run poisoning.
- On AWS, require IMDSv2 with a hop limit of 1 (or block the metadata
  service), and attach no instance role to the machine.
- Limit the runner group to this repository, and do not give it to forks.
- Keep the fork pull request approval policy at "Require approval for all
  external contributors".
