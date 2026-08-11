# CLI and browser tutorial

The `rtce` library now has two thin interfaces over the same JSON runner:

- a native `rtce` command for scripts, CI, and local experiments;
- a TypeScript client backed by WebAssembly for the interactive tutorial.

Both produce the same versioned JSON envelopes. Neither reimplements engine
math outside the `rtce` crate.

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
rtce evaluate --game $game --build $build --scenario $scenario
rtce explain --game $game --build $build --scenario $scenario
rtce simulate --game $game --build $build --scenario $scenario --sim $sim --rotation $rotation
rtce simulate --game $game --build $build --scenario $scenario --sim $sim --rotation $rotation --mode monte-carlo --iterations 1000 --seed 7
rtce reset
```

Results are structured values, so browser-terminal pipelines work directly:

```text
rtce evaluate --game $game --build $build --scenario $scenario | get objectives
rtce simulate --game $game --build $build --scenario $scenario --sim $sim --rotation $rotation | get report.actions
lesson list | select number lesson command
```

The browser commands deliberately match the native CLI's argument names. Since
the standalone tutorial cannot read a filesystem, browser-terminal injects the
live editor JSON as `$game`, `$build`, `$scenario`, `$sim`, and `$rotation`.
The variables update as the editor changes and expand before the registered
command runs. Run `config list` or the shell's built-in `vars` command to
inspect what is available in the current lesson. In Docker, replace those
variables with paths such as `/work/gamedef.json`; the rest of the command is
the same.

The page's **Run current lesson** button uses that same terminal path. It types
the lesson command at the prompt, executes it, streams compilation and fight
playback into the terminal, then updates the result cards from the command's
structured return value. For simulations, the playback timestamps are clearly
labeled as report-derived: action counts and damage totals are exact engine
output, while the illustrative timestamps are reconstructed from those totals
because `SimReport` does not expose the executor's internal event queue.

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

Every successful response contains:

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

Invalid files, unknown keys, unresolved symbols, and runtime simulation errors
produce a non-zero native exit or a rejected browser call with the same
contextual message.
