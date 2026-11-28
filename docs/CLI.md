# CLI and browser tutorial

The `rtce` library now has two thin interfaces over the same JSON runner:

- a native `rtce` command for scripts, CI, and local experiments;
- a TypeScript client backed by WebAssembly for the interactive tutorial.

Both use the same engine and produce versioned JSON reports.

## Start with Docker

Docker is the primary demo path. No local Rust or Node installation is needed.

Build the CLI image and run the bundled stat-sheet lesson:

```sh
docker build --target cli -t rtce .
docker run --rm rtce
```

The image defaults to `demo calc`. The other bundled runs are:

```sh
docker run --rm rtce demo sim
docker run --rm rtce demo monte-carlo
```

Each command writes JSON to stdout and diagnostics to stderr, so normal shell
tools can capture or pipe the result:

```sh
docker run --rm rtce --compact demo sim > report.json
```

## Run your own configuration

There are three closed-form inputs:

1. `GameDef` — the game's algorithm;
2. `BuildState` — one character sheet;
3. `Scenario` — the fight being asked about.

Mount a directory read-only at `/work`, then evaluate it:

```sh
docker run --rm \
  -v "$PWD/my-config:/work:ro" \
  rtce evaluate \
  --game /work/gamedef.json \
  --build /work/build.json \
  --scenario /work/scenario.json
```

Use `explain` with the same flags to include phase, bucket, stage, and event
branch traces:

```sh
docker run --rm \
  -v "$PWD/my-config:/work:ro" \
  rtce explain \
  --game /work/gamedef.json \
  --build /work/build.json \
  --scenario /work/scenario.json
```

A timeline adds `SimDef` and `Rotation`:

```sh
docker run --rm \
  -v "$PWD/my-config:/work:ro" \
  rtce simulate \
  --game /work/gamedef.json \
  --build /work/build.json \
  --scenario /work/scenario.json \
  --sim /work/simdef.json \
  --rotation /work/rotation.json
```

For a seeded distribution rather than one expected-value timeline:

```sh
docker run --rm \
  -v "$PWD/my-config:/work:ro" \
  rtce simulate \
  --game /work/gamedef.json \
  --build /work/build.json \
  --scenario /work/scenario.json \
  --sim /work/simdef.json \
  --rotation /work/rotation.json \
  --mode monte-carlo \
  --iterations 1000 \
  --seed 7
```

Run `docker run --rm rtce --help` or append `--help` to a subcommand for the
complete flag reference.

### Dump a simulation timeline

Add `--events` to `simulate` to stream the resolved events of iteration zero
to stdout as JSON Lines. For Monte Carlo, `--events=7` selects the eighth
iteration and requires at least eight iterations. Omit `--events` to get
the usual simulation report.

This native example writes the bundled archer fight to `timeline.jsonl`:

```sh
fixture=crates/rtce/tests/fixtures/guide
cargo run -p rtce-cli -- simulate \
  --game "$fixture/07-gamedef.json" \
  --build "$fixture/07-build.json" \
  --scenario "$fixture/07-scenario.json" \
  --sim "$fixture/07-simdef.json" \
  --rotation "$fixture/07-rotation.json" \
  --events > timeline.jsonl
```

Every line is one JSON event. The first line is `simulation_started` and
also carries `schema_version`, `mode`, `iteration`, and `seed`. The seed is
a decimal string for Monte Carlo and null in expected mode. A successful
timeline ends with `simulation_ended`. A failed run can leave a partial
stream and writes its diagnostic to stderr.

The Rust API, `rtce::sim::run_with_events`, calls a sink for each record as
it happens. The sink can serialize JSON Lines directly or accumulate typed
records while the simulation inputs are in scope. Wasm consumers can call
`simulate_expected_with_events` or `simulate_monte_carlo_with_events` with the
same arguments as the existing bindings, plus a zero-based iteration after
the seed for Monte Carlo. These bindings return the event array and simulation
report in a single JSON envelope.

### Stream events into a JavaScript UI

Use `streamSimulation` to run the engine in a Web Worker. It exposes a
`ReadableStream` of event objects and a Promise for the final report. The
Worker posts one message per event as it occurs, in simulation order.

```js
import { streamSimulation } from './timeline';

const { events, result } = streamSimulation(config, {
  kind: 'monte_carlo',
  iterations: 1000,
  seed: 42n,
  eventIteration: 0,
});

for await (const event of events) {
  appendToTimeline(event);
}

const { report } = await result;
```

The first event carries the same metadata as the first JSON Lines record.
In Monte Carlo, `simulation_ended` marks the end of the selected timeline.
The report resolves after all iterations finish. A failure rejects both the
stream and the report Promise. Breaking out of the loop cancels the stream
and terminates its Worker. The Worker pushes events as the engine produces
them, so a slow consumer can accumulate unread events.

The underlying Wasm exports are `simulate_expected_stream` and
`simulate_monte_carlo_stream`. Each takes the same arguments as the
corresponding `_with_events` export, plus a final `onEvent(eventJson)`
callback. The callback receives one JSON record synchronously per event and
the Wasm function returns the final report JSON. A Promise around a call on
the main thread still leaves the computation on the main thread. The Worker
lets the UI paint while the simulation is running.

Run `npm run dev --prefix web` and open
`/examples/streaming-timeline.html` on the Vite server to try the archer
simulation. The example uses the Worker and stream helper directly.
[Worker example](../web/src/timeline.worker.ts) · [Stream helper](../web/src/timeline.ts)

### Inspect the config vocabulary

To inspect the expression vocabulary without opening a config, run:

```sh
docker run --rm rtce lexicon
```

The structured dictionary separates config schema words, names declared by
your config, expression functions/operators, engine-supplied runtime context,
and special conventions. That distinction is intentional: `fold` is a schema
key, `attack_power` and `stamina` come from your files, `max` and `clamp` are
language functions, while names such as `time` and `cooldown.<action>` exist
only during a simulation.

## Open the browser tutorial

The tutorial grows one archer config through the seven guide chapters: one
stat, bucket folds, critical-hit branches, fight phases, a resource rotation,
computed buff uptime, and finally Monte Carlo.

```sh
docker compose up --build tutorial
```

Open <http://localhost:8080>. The page contains live JSON editors and an
embedded browser-terminal. Useful terminal commands are:

```text
lesson list
lesson load 3
config show gamedef
config list
rtce lexicon
rtce evaluate --game $game --build $build --scenario $scenario
rtce explain --game $game --build $build --scenario $scenario
rtce simulate --game $game --build $build --scenario $scenario --sim $sim --rotation $rotation
rtce simulate --game $game --build $build --scenario $scenario --sim $sim --rotation $rotation --mode monte-carlo --iterations 1000 --seed 7
rtce reset
```

Results are structured values, so browser-terminal pipelines work directly:

```text
rtce evaluate --game $game --build $build --scenario $scenario | get objectives
rtce simulate --game $game --build $build --scenario $scenario --sim $sim --rotation $rotation | get report | get actions
lesson list | map command
```

The browser commands deliberately match the native CLI's argument names. Since
the standalone tutorial cannot read a filesystem, browser-terminal injects the
live editor JSON as `$game`, `$build`, `$scenario`, `$sim`, and `$rotation`.
The variables update as the editor changes and expand before the registered
command runs. Run `config list` or the shell's built-in `vars` command to
inspect what is available in the current lesson. In Docker, replace those
variables with paths such as `/work/gamedef.json`; the rest of the command is
the same.

The page's **Run CLI demo** button uses that same terminal path. For
simulation lessons, the terminal shows the engine's events at their simulated
timestamps as the Worker emits them. In Monte Carlo it shows iteration zero
and then displays the pooled result from all iterations. The result cards
and structured shell output use the final report.

All calculation happens in the browser. The nginx container serves static
HTML, JavaScript, and two Wasm modules; it exposes no calculation API and
stores no configuration.

### Open it directly from the filesystem

The normal production output is intentionally split into cacheable JavaScript,
CSS, and Wasm assets and therefore needs an HTTP server. For a demo that opens
from Finder or a `file://` URL, export the standalone artifact with Docker:

```sh
docker build --target standalone --output web/file-demo .
```

Then open `web/file-demo/rtce-field-guide.html`. This remains Docker-first and
does not require a local Rust, Wasm, or Node toolchain.

When developing the tutorial locally, the equivalent build is:

```sh
npm run build --prefix web
```

Then open `web/dist-standalone/rtce-field-guide.html`. It embeds the stylesheet,
application bundle, browser-terminal Wasm, and rtce Wasm in one file. It makes
no network requests and does not need Docker or a development server.

The standalone file is deliberately a demo artifact rather than the normal
deployment format: base64 makes it larger and prevents streaming Wasm
compilation. The Docker tutorial continues to serve the smaller split assets.

## Publish with GitHub Pages

The repository's [Pages workflow](../.github/workflows/pages.yml) deploys the
normal split Vite build from `web/dist` whenever `main` is updated. The workflow
installs the Rust and Node toolchains, builds both Wasm modules and the
TypeScript application, uploads the static artifact, and deploys it through the
repository's `github-pages` environment.

Vite emits relative asset URLs, so the JavaScript, CSS, and Wasm files work at
the repository project path. The published tutorial is available at
<https://benjamin-small.github.io/rpg-theorycraft-engine/> after the Pages
workflow finishes.

Generated `web/dist` files are CI artifacts and remain ignored by Git; only the
source and pipeline configuration are committed.

## Run without Docker

The native command is an ordinary workspace binary:

```sh
cargo run -p rtce-cli -- demo calc
cargo run -p rtce-cli -- evaluate \
  --game gamedef.json --build build.json --scenario scenario.json
```

For the browser tutorial, install the `wasm32-unknown-unknown` target and a
matching `wasm-bindgen-cli`, then install the web dependencies:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126 --locked
npm install --prefix web
npm run dev --prefix web
```

The TypeScript facade is [`web/src/rtce.ts`](../web/src/rtce.ts). `RtceClient`
exposes `evaluate`, `explain`, `simulateExpected`, and `simulateMonteCarlo` over
typed `ConfigSet` input. The binding accepts JSON strings intentionally: serde
and every fail-closed config rule remain on the Rust side of the boundary.

## JSON response contract

Every successful JSON envelope contains:

```json
{
  "schema_version": 1,
  "kind": "evaluation"
}
```

`kind` is `evaluation`, `explanation`, or `simulation`. Evaluation objectives
are keyed by name. Explanations add the engine's trace. Simulations add `mode`
and the complete `SimReport`. New report fields may be added compatibly; a
breaking envelope change requires a new `schema_version`.

When tracing is enabled in Wasm, the envelope also contains `event_stream`
with its own `schema_version`, the zero-based `iteration`, the master `seed`
as a decimal string (null in expected mode), and an ordered `events` array.
The CLI writes the same event records directly under `--events`.
Each record has a `type` and a `time` in seconds. Array order breaks ties
when records share a timestamp. Cast IDs are local to the selected timeline
and join cast starts, segment starts, landings, cancels, and effects.

| Record | Use |
| --- | --- |
| `simulation_started`, `simulation_ended` | Fight duration and total damage for the traced iteration. |
| `phase_entered` | Scenario phase name and zero-based index. |
| `cast_started`, `segment_started` | Action or segment start, the configured rule or triggering proc, and cooldown timing when applicable. |
| `cast_landed`, `cast_cancelled` | Per-landing damage, hit and crit information, and player-driven cancels. A cast still in flight at the horizon has no final landing. |
| `proc_fired` | Proc name, the originating cast, and when its internal cooldown ends. |
| `buff_changed`, `expected_buff_changed` | Discrete applications, expiries and removals, or expected-uptime roll and age-out records. |
| `buff_damage` | Continuous damage credited over the interval from `since` to `time`. |
| `resource_changed` | Resource amount, cap, and regen after initialization, costs, gains, or limit changes. |

Landings summarize hits resolved together. Expected mode reports crit
probability, Monte Carlo reports the sampled outcome or count, and free
casts carry branch-blended damage with a null crit field. Continuous damage
and regeneration follow the engine's integration intervals. The stream
covers resolved operations. Rotation candidate checks and failed proc
rolls are outside this record set.

Invalid files, unknown keys, unresolved symbols, and runtime simulation errors
produce a non-zero native exit or a rejected browser call with the same
contextual message.
