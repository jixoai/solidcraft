# Maintaining the FDE2E SolidCraft fork

Public brand and provenance rules are in [../FORK.md](../FORK.md). Keep branding in a separate
fork commit; after each rebase, inspect UI/catalog names, repository/help/update URLs, PWA and
installer display metadata, sponsorship/social links and Pages canonical URLs. Do not overwrite
upstream copyrights, dependency origins or compatibility IDs. No fork release or live deployment
is implied by the upstream mirror or by a feature-branch push.

## FDE2E branding validation (2026-11-06)

- The browser editor, About window, page/PWA metadata and five live WebMCP tool titles use
  FDE2E SolidCraft. Real screenshots and logs are under ignored `target/fde2e-branding/`.
- Targeted native checks pass: 45 update tests, 8 Help tests, 6 header/layout tests and the MCP
  handshake test. The updater accepts fork releases and rejects both upstream owners.
- The 15-crate Wasm check, browser build, real Chrome WebMCP modelling and registration fallbacks, browser API and
  offline/update tests pass. Message IDs and placeholders remain unchanged in 35 locale catalogs.
- Applicable packaging Python tests pass (24). Four Windows-specific checks cannot run on this
  machine because `powershell` is absent; the platform display metadata and compatibility paths
  were checked separately. No installer build or code-signing claim is made.
- `cargo xtask ci` passes static gates and workspace native Clippy, then its unbounded native
  run times out in two untouched `help_output` fuzz-CLI tests. Both pass when run in isolation
  (0.11 seconds). Remaining full-CI processes were stopped; **full CI is not green**.
- For the next full validation, bound native binary concurrency with
  `SOLIDCRAFT_TEST_JOBS=4 cargo xtask ci`. This does not waive the existing baseline failures
  recorded below. Never switch branches while CI runs.

This fork keeps upstream history separate from the browser WebMCP implementation. The product requirements are in [PRD.md](PRD.md). Contribution and validation rules remain in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Remotes and branches

| Name | Purpose |
| --- | --- |
| `origin` | `https://github.com/jixoai/solidcraft.git`: the only GitHub push destination |
| `upstream` | `https://github.com/tangency-labs/solidcraft.git`: fetch only |
| `main` | An unmodified copy of upstream `main`; no fork implementation commits |
| `feat/webmcp-worker` | Browser WebMCP, evaluated-model transfer, Worker transactions and their tests |
| `local` | Delta's existing local managed-repository remote; leave it intact |

Use the default branch for upstream synchronization, not feature development. Keep implementation and documentation commits separate so they can be reviewed or transplanted independently. Do not merge the implementation into `main` unless the fork intentionally adopts a different default-branch policy.

The local checkout sets these protections:

```sh
git remote set-url --push upstream DISABLED
git config --local remote.pushDefault origin
git config --local push.default simple
git config --local pull.ff only
git config --local fetch.prune true
```

These settings are local Git configuration, not tracked files. A fresh clone must configure them again. Preserve an existing Delta `local` remote rather than replacing it.

## Set up a fresh clone

```sh
git clone https://github.com/jixoai/solidcraft.git
cd solidcraft
git remote add upstream https://github.com/tangency-labs/solidcraft.git
git remote set-url --push upstream DISABLED
git config --local remote.pushDefault origin
git config --local push.default simple
git config --local pull.ff only
git config --local fetch.prune true
git fetch origin
git fetch upstream main
git switch --track origin/feat/webmcp-worker
```

The last command requires the implementation branch to have been published. A local commit or bundle is not proof that the branch exists on GitHub.

## Synchronize the fork's main branch

Fetch first and confirm that the fork contains no main-only commits:

```sh
git fetch origin main
git fetch upstream main
git rev-list --left-right --count origin/main...upstream/main
git merge-base --is-ancestor origin/main upstream/main
```

The first number must be zero, and the ancestry check must succeed. If not, stop and inspect the divergent commits. Never reset or force-push `main` to hide fork changes.

When the ancestry check succeeds, update the remote fork with a normal fast-forward push:

```sh
git push origin upstream/main:refs/heads/main
git fetch origin main
```

This operation does not switch the current worktree or integrate upstream changes into the implementation branch. A concurrent incompatible update to remote `main` makes the normal push fail rather than overwriting it.

To update local `main`, first finish any running CI and preserve uncommitted work. Then:

```sh
git switch main
git merge --ff-only origin/main
git switch feat/webmcp-worker
```

If another worktree owns `main`, update it in that worktree. Do not use `git branch -f` to bypass worktree ownership or a divergent local branch.

## Integrate upstream into the implementation

Synchronizing `main` does not automatically update the feature. Integrate upstream when making a substantive implementation update or resolving a reported conflict. Do not push the feature solely to catch up: every push can restart expensive CI.

Before rebasing, ensure the worktree is clean and create a uniquely named backup branch. Do not remove the backup until the rebased implementation has been validated and published.

```sh
git switch feat/webmcp-worker
git status --short
git fetch origin
git fetch upstream main
git branch backup/webmcp-before-update-YYYYMMDD-HHMM HEAD
git rebase upstream/main
```

Replace the backup suffix with the current date and time. If there are uncommitted changes, stop and preserve them first; `git status` is an inspection, not an automatic guard. If another contributor changed the remote feature, inspect and coordinate before rewriting it.

Resolve each conflict by preserving both upstream behavior and the fork's implementation. Do not accept all of one side. For the lockfile, retain upstream dependency upgrades and the fork's required `rmp-serde`/`serde_bytes` dependencies. Do not regenerate unrelated packages solely to erase a conflict.

```sh
git add PATHS_RESOLVED
git rebase --continue
```

Use `git rebase --abort` to return to the pre-rebase branch if integration cannot safely be completed. Never switch branches while `cargo xtask ci` is running.

### High-risk integration boundaries

| Files | What must survive an upstream update |
| --- | --- |
| `apps/solidcraft-web/src/main.rs`, `index.html` | Normal app startup, compiled wasm handoff, Worker startup and upstream UI changes |
| `apps/solidcraft-web/src/api.rs`, `webmcp.rs`, `worker.rs` | Real tool registration, schemas, limits, queue ordering, cancellation and recovery |
| `crates/doc/src/model_transfer.rs`, `eval.rs` | Exact geometry and complete evaluated states; display data stays display-only |
| `crates/engine/src/worker.rs` | Transaction rollback, undo/redo, stale-result refusal and binary size limits |
| `crates/ui-egui/src/control*.rs`, `lib.rs`, `prefs.rs` | User edits take priority, ordinary host commands work, legacy synchronous preference remains |
| `xtask/src/web_offline_test.rs`, `mesh_decisions.rs` | Actual browser regression coverage; no new mesh-based model decisions |

Transfer data belongs to instances of the same web build. Do not mix a new app bundle with old worker assets or assume the transient wire format is a persisted design-format migration.

## Validate before committing or publishing

Use the current checkout and keep logs outside tracked files:

```sh
mkdir -p target/fork-maintenance
cargo fmt --all --check
git diff --check
cargo xtask ci > target/fork-maintenance/ci.log 2>&1
```

Check the exit status and log. The full gate includes static checks, native Clippy, tests, wasm and offline coverage. A started command, targeted tests or a previous build's log is not a passing full gate.

Build current browser assets before browser API and WebMCP tests:

```sh
(cd apps/solidcraft-web && NO_COLOR=true trunk build --release --public-url ./)
cargo xtask web-api-test --site dist/web
cargo xtask webmcp-test --site dist/web
```

Install the browser build prerequisites listed in [building.md](building.md). `NO_COLOR=true` avoids Trunk rejecting an inherited `NO_COLOR=1` value. Confirm current command syntax with `cargo xtask --help` if upstream changes it. Reopen the saved complex model and check bodies, sketches, timeline, key volumes and parameter changes. Retain real screenshots or a recording as evidence; do not commit `target/`, browser profiles, tokens or generated reference files.

The original local validation covered browser API, actual Chrome WebMCP, Worker failure recovery and the AERO 01 model. It was not a completed full CI run. The current update's exact validation status belongs in its commit/review handoff; do not treat this historical statement as a permanent failure or passing result.

## Publish safely

For an unpublished branch or normal new commits:

```sh
git push -u origin feat/webmcp-worker
```

For an already published, coordinated rebase, record the remote feature commit before starting the update. Use that exact SHA in the lease:

```sh
git push --force-with-lease=refs/heads/feat/webmcp-worker:EXPECTED_REMOTE_SHA origin feat/webmcp-worker
```

Do not substitute a fresh remote SHA merely to bypass a failed lease. Inspect the other person's changes. Never force-push `main` or push to upstream.

If authentication fails, commits remain local. Reauthenticate an account with write access to `jixoai/solidcraft`, then retry:

```sh
gh auth login --hostname github.com --git-protocol https
gh auth setup-git
gh auth status
```

Do not paste tokens into chat, repository files or logs. After a push, confirm the remote branch SHA, not just the presence of a local tracking configuration.

Git and `gh` can use different credentials. A failed `gh auth status` does not by itself prove that Git publication is blocked. Check the actual Git transport before changing credential helpers.

## Prepared checkpoint (2026-11-06)

- Original implementation base: `67c58cbd28d70ca1a1a1b718d589e8ea7625b437`.
- Upstream snapshot and synchronized fork `main`: `942e1540db2f38e9df36b8b91730cb989d6261d9`.
- Original implementation checkpoint: `e22756e92`, kept on local `backup/webmcp-original-20261106`.
- Rebased implementation: `b84407afa`. `git range-diff` reports an identical implementation patch after the 59-commit upstream update; no textual conflict was discarded.
- The original-base full gate passed static checks, native Clippy and the 15-crate wasm check. The 47 web tests and four Worker regressions also passed.
- That gate was not green: failures were observed in `boolean_props`, `naming_corpus`, `model_cache`, `solidcraft_smlib`, `action_matrix` and doc evaluation-cost tests. Three remaining long-running kernel/engine/UI binaries were terminated after the failures. These checks were not disabled or reclassified as passing.
- The integrated checkout passed static checks, workspace native Clippy and the 15-crate wasm check. Its repeated full native test build was stopped after the original gate had exposed the failures; full integrated CI remains incomplete.
- A fresh, isolated browser build using upstream's egui/eframe 0.37 passed the real Chrome WebMCP and browser API suites. The compiled assets also passed offline/update tests with an explicitly generated offline manifest. This was a development build, not a published release build or a new performance benchmark.
- That fresh build reopened the AERO 01 design: 95 closed bodies, 96 timeline features, 12 zero-DOF sketches and no timeline notices. The `service_explode` walk `0 → 60 → 0 mm` moved 87 bodies, preserved their volumes and restored their positions. A new screenshot, exported design and report are local `target/fork-maintenance/reopened-model.*`.
- Original and integration gate logs are local `target/fork-maintenance/ci-original-base.log` and `ci-upstream-integration.log`. They are not distributed with a clone. Integration-specific browser results must refer to the newly built site, not the old demo assets.

This is an implementation branch, not a claim that the full upstream release gate passes. Fix or triage unrelated kernel failures in their owning area before requesting a production merge. Later updates must create new validation evidence rather than relying on this checkpoint.

## Backups and upstream contributions

- The current preparation preserves the original uncommitted patch and added files under local `target/fork-maintenance/`. That directory is ignored and is not a durable backup.
- After local commits exist, an external Git bundle can preserve the branch without GitHub access. Store it outside build-cleanup paths and do not describe it as a remote publication.
- Keep the pre-integration backup branch until the new implementation is checked. It preserves the original commit history but not uncommitted files.
- Open an upstream PR only when explicitly requested. Check existing issues and draft claims first, keep one topic per PR and follow the contribution skill.
- Product requirements and fork operations are not necessary runtime changes. Submit them separately from a runtime PR if contributing upstream.
- No scheduled workflow silently rebases or force-pushes the implementation. Upstream integration requires conflict review and validation; fetch-only checks and fast-forward main synchronization are separate operations.
