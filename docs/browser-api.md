# Browser command API

After the browser app starts, `window.solidcraft` controls the design open in that tab.
Calls use the same command ids, parameters, control handlers and undo history as the desktop
control channel. No separate engine or server is started.

| Call | Result |
|---|---|
| `solidcraft.request(method, params?)` | Any [control method](control-protocol.md), such as `ui.inspect`, `document.inspect` or `engine.batch` |
| `solidcraft.execute(command, params?)` | Run an engine command without opening its interactive dialog |
| `solidcraft.commands()` | Discover commands, their parameters and current enablement |
| `solidcraft.inspect(params?)` | Inspect the active document, optionally with `{measure: true}` |

Every call returns a promise. It resolves to the control reply's `result`, or rejects with an
`Error`. Omitted or `null` parameters become `{}`. Parameters must be JSON serializable and
at most 16 MiB of JSON. At most 256 requests and 32 MiB of parameters may be pending; excess requests reject without
running. Requests keep their submission order. Modelling and measurement use a reusable engine
Worker when **Keep the window responsive while agents run commands** is enabled (the default
for new browser preferences). Other control requests run between frames, at most one per frame.
A hidden or suspended tab can delay completion; keep it active while driving the app. A reload
ends the session and its pending work.

In the browser's developer console, on an empty design:

```js
await solidcraft.commands();
await solidcraft.request("ui.home", {open: false});
await solidcraft.execute("solid.box", {
  length: 30, width: 20, height: 10, body_name: "Browser API box"
});
await solidcraft.inspect({measure: true});
await solidcraft.execute("edit.undo");
await solidcraft.execute("edit.redo");
await solidcraft.request("ui.inspect");
```

An agent with JavaScript evaluation access to this tab can make the same calls. Scripts
executing in the page can use the API with the page's browser permissions. File paths refer to
the browser's virtual file system, not arbitrary files on the user's computer. Native-only
operations remain subject to the browser host's limitations; this API does not grant desktop
filesystem or window access.

## An agent in the browser (WebMCP)

Where the browser implements [WebMCP](https://webmachinelearning.github.io/webmcp/), the app
registers tools on `document.modelContext`, so an agent running in the browser finds the
command surface without driving the page. Chrome has WebMCP from 149 on, behind its origin
trial or `chrome://flags/#enable-webmcp-testing` (`--enable-features=WebMCP` on the command
line); a browser without it leaves `document.modelContext` undefined and nothing is
registered.

| Tool | Call | Annotations |
|---|---|---|
| `solidcraft_list_commands` | `engine.commands` (the whole catalog, about 350 KB) | read-only |
| `solidcraft_describe_command` `{command}` | `engine.execute` of `engine.describe` | read-only |
| `solidcraft_execute` `{command, params?}` | `engine.execute` | consequential |
| `solidcraft_batch` `{commands, rollback?}` | `engine.batch` | consequential |
| `solidcraft_inspect` `{measure?}` | `document.inspect` | read-only |

Every successful answer is `{ok: true, result}`. A failure is `{ok: false, error}`; a failed
batch also retains `result`, including `failed.index`, `failed.error`, `completed` and
`rolled_back`. The rollback flag describes the design, and cannot undo file writes. The browser
may replace an exception with a generic message, so callback failures travel as values that
keep their reasons. Errors raised by WebMCP itself, such as an aborted call, can still reject.
`execute` and `batch` carry the consequential hint because file commands can write files.
Tools that return design data carry `untrustedContentHint`, since names can come from imported
designs.

WebMCP shares the API's 16 MiB JSON, 256 pending-request and 32 MiB queue limits. A batch contains 1–10,000
steps. Unknown arguments, malformed steps and arguments of the wrong type are refused before
any step runs, including with `rollback: false`. Omit optional properties when unused.

The tools follow the page's `tools` permission policy and are registered for the same origin.
A same-origin frame (`?embed=editor`) offers them too. `allow="tools"` alone does not expose
them to a cross-origin parent: WebMCP also requires provider `exposedTo` and consumer
`fromOrigins` opt-ins, which this integration does not configure. Nothing opens a network
listener.

A session through the tools only:

```js
const context = document.modelContext;
const call = async (name, input) => {
  const tool = (await context.getTools()).find(t => t.name === name);
  return JSON.parse(await context.executeTool(tool, input));
};
await call("solidcraft_batch", {commands: [
  {command: "sketch.create", params: {plane: "XY"}},
  {command: "sketch.rectangle.two_point", params: {p0: [0, 0], p1: [80, 50]}},
  {command: "sketch.finish", params: {}},
  {command: "solid.extrude", params: {distance: 10}},
]});
await call("solidcraft_inspect", {measure: true});
```

## Keeping the browser responsive

One dedicated Worker reuses the page's compiled wasm module and its evaluated session across
calls. Pure modelling batches run there as one transaction. Exact native B-rep, model records,
undo/redo and prepared display buffers return through transferable binary buffers. The UI
accepts those results without replaying the modelling commands or tessellating the result.
Unchanged calls reuse the worker's source; only new document versions are included in replies.
Each binary message is limited to 128 MiB. Exceeding the limit reports that limit and leaves
the live design unchanged. A failed transaction discards its worker; the next request starts
a fresh one, so an oversized result does not disable later modelling commands.

The existing Stop action and a WebMCP call's AbortSignal terminate worker work without applying
it. Abort listeners are removed when their requests finish. An edit made during computation
causes the transaction to start again against the edited design (at most twice). Switching to
another design refuses the old transaction. View navigation keeps using the normal UI path.

File operations, linked designs, clipboard, appearances and host-specific commands use the existing control handlers.
Requests with control-level scope, validation or timing options also keep those handlers.
A batch containing such a command uses the control handler for the whole batch. These paths
can still contain synchronous work; the worker is not a guarantee that every browser task is
below a particular frame budget. Large result decoding and GPU uploads also take UI time.
Keep appearance assignments separate from expensive modelling batches so the geometry stays
on the worker.

Worker startup needs `worker-src blob:` and access to this build's module bindings under the
site's Content Security Policy. A worker that cannot start or crashes returns an error and
keeps the displayed design. Disabling **Keep the window responsive while agents run commands**
restores synchronous execution.

`solidcraft.runtime()` reports worker readiness, workers started, routing counts,
queue depth, the last computation and adoption durations, and transfer sizes. These are local
diagnostics; they do not contain the design or send telemetry.

## A host page that keeps the designs

`?embed=editor` runs the app as an editing embed: the ribbon, browser, timeline and properties,
without the application bar or start page, and with nothing kept in the browser. A page that
frames it from the same origin reaches this API as `frame.contentWindow.solidcraft`, keeps the
designs itself and moves them with three [control methods](control-protocol.md):

```js
const app = frame.contentWindow.solidcraft;
const kept = await app.request("document.export"); // {name, revision, dirty, format: "json", text}
// ...the page stores kept.text, and says so:
await app.request("document.saved", {revision: kept.revision});
// ...and later puts it back, timeline and parameters as saved:
await app.request("document.import", {text: kept.text, name: "Bracket"});
```

The 16 MiB limit on parameters applies to `document.import`'s `text`. A design that links other
design files keeps those links as paths; the files themselves are not in `text`.

## Verify a browser build

Build the app with `trunk build` in `apps/solidcraft-web`, then run from the repository root:

```sh
cargo xtask web-api-test --site dist/web
```

The test starts a local server and headless Chrome, discovers commands, creates a box in the
live document, checks undo and redo, and verifies error handling and the pending-request limit.
It also checks that Ctrl+Tab and Ctrl+Shift+Tab are left to the browser, which switches tabs with them.
Set `CHROME` to a Chrome or Chromium executable if it is not installed in a usual location.

For the WebMCP tools:

```sh
cargo xtask webmcp-test --site dist/web
```

This launch enables Chrome's WebMCP feature. The test lists the five tools and their schemas,
reads the catalog, then builds an EN 1092-1 DN 50 PN 16 blind flange through
`solidcraft_batch` — flange Ø 165, bolt circle Ø 125, 4 × Ø 18 holes, raised face Ø 102 × 2,
thickness 18, from the public DN 50 dimension chart — and measures its volume against the
analytic value (342 176 mm³, within 0.1 %). It also checks that a refused call comes back with
its reason, that undo and redo work through the tools, and that a batch whose step fails puts
the design back.
