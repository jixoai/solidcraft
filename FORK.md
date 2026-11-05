# FDE2E SolidCraft fork

## Source and licence

This repository is a fork of [tangency-labs/solidcraft](https://github.com/tangency-labs/solidcraft),
anchored at `942e1540db2f38e9df36b8b91730cb989d6261d9` for the current integration.
The fork repository is [jixoai/solidcraft](https://github.com/jixoai/solidcraft).

FDE2E maintains this fork under the public product name **FDE2E SolidCraft**. This is not an
upstream release, endorsement or transfer of upstream trademarks. The upstream MIT OR
Apache-2.0 licences, copyrights, notices and asset attributions remain intact. New contributions
use the same project licence. The simpler brand presentation follows the FDE2E Film project
convention, without copying its artwork, text or implementation.

## Changes carried by the fork

- Browser WebMCP tools route into the existing command registry and design session.
- A persistent Rust/Wasm Worker performs agent modelling off the UI thread, with exact evaluated
  model transfer, bounded requests, cancellation, stale-result checks and rollback/recovery.
- The original synchronous mode remains available through `agent_off_thread=false`.
- Public UI, PWA metadata and package display names use FDE2E SolidCraft. Original social,
  sponsorship and promotional links are removed from the active project entry points.
- Documentation, issue reporting and desktop update downloads point to this fork. The update
  allowlist explicitly rejects upstream release assets, including the former upstream owner.
- Product requirements and maintenance procedures are recorded locally.

No branded release or public deployment is claimed. In-app documentation links target this
implementation branch until it becomes the fork's published default branch. GitHub Pages SEO
uses the deployment's `PAGES_BASE_URL`, not the upstream custom domain. The old upstream
cross-origin design-transfer page is not shipped by this fork.

## Compatibility and provenance boundary

Keep `solidcraft-*` crate/binary names, `window.solidcraft`, MCP/tool identifiers, `.sc` and
legacy design extensions, format tags, environment variables, settings/recovery/keychain paths,
package filenames, application IDs and install paths stable. These are compatibility contracts,
not claims of upstream authorship. Existing original, attributed CAD artwork is retained as
licensed artwork; upstream wordmarks, animated loading promotion and social preview cards are
not displayed in the fork's web entry points.

Do not mechanically replace upstream names or repository URLs in licence texts, attributions,
dependency origins, historical issue references or compatibility records. Upstream operational
instructions in historical release documentation do not authorise publishing through its stores,
credentials or domains.

## Branch strategy

- `main`: unmodified upstream mirror. No fork branding or implementation commits land here.
- `feat/webmcp-worker`: integration branch for the fork implementation and branding.
- `origin`: fork fetch/push; `upstream`: source fetch only, with push disabled.
- Future upstream updates: back up the integration branch, fast-forward the mirror, rebase fork
  commits, inspect the range-diff, validate and publish using `--force-with-lease` only when a
  previously published feature branch was rebased. Never force-push `main`.

Keep branding separate from engine changes so future upstream merges can distinguish product
customisation from modelling behaviour. See [docs/fork-maintenance.md](docs/fork-maintenance.md)
for commands and validation limitations.
