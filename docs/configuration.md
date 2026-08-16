# Configuration

rtce models game math and combat timelines with JSON documents. It does not
read environment variables and requires no `.env` file. Native CLI inputs are
ordinary files; the browser tutorial injects the same documents from its live
editors.

## Calculation documents

A closed-form stat-sheet calculation uses three documents:

1. **GameDef** declares stats, conditions, modifier buckets, probabilistic
   events, pipeline stages, and named objectives.
2. **BuildState** supplies character stats and modifier contributions. A
   contribution can be gated by a condition or an event.
3. **Scenario** supplies one or more fight phases, their duration or weight,
   stat overrides, and condition uptimes.

Run them with:

```sh
rtce evaluate --game gamedef.json --build build.json --scenario scenario.json
```

Use `rtce explain` with the same arguments for bucket, stage, phase, and event
branch traces.

## Simulation documents

A timeline simulation adds two documents:

4. **SimDef** declares resources, actions, buffs, procs, and the objective used
   as action damage.
5. **Rotation** is an ordered priority list. The first eligible rule runs;
   resource costs and cooldown readiness are automatic hard gates.

Run an expected-value timeline with `rtce simulate` and add
`--mode monte-carlo --iterations N --seed S` for reproducible sampled fights.
The complete Docker commands and browser equivalents are in
[`CLI.md`](CLI.md).

## Expressions and names

Most expression identifiers come from the configuration: stats, conditions,
buckets, earlier pipeline stages, resources, actions, and buffs. The engine
also supplies context names in the scopes where they make sense, including
`time`, `duration`, `cooldown.<action>`, `buff.<buff>`,
`buff_remaining.<buff>`, `casts.<action>`, and `stacks.<buff>`.

Run `rtce lexicon` for the authoritative, machine-readable dictionary of
schema terms, declared names, functions, operators, engine context, and
conventions. The interactive tutorial exposes the same dictionary.

Numeric functions are `min(a, b)`, `max(a, b)`, `clamp(x, lo, hi)`,
`floor(x)`, `sqrt(x)`, and `pow(base, exponent)`. `pow` accepts fractional
exponents, so formulas such as the probability that at least one active stack
came from a critical hit remain inside the GameDef:

```text
1 - pow(1 - crit_chance, max(stack_potential, 1))
```

`sqrt` and `pow` use Rust `f64`/IEEE semantics. Invalid domains produce NaN
and overflow can produce infinity; they do not become expression compile
errors. A closed-form `Plan` returns that derived value to its caller, while a
simulation field that requires a finite quantity rejects it at runtime with the
field and evaluation instant named. Input stats, contributions, phase values,
and uptimes retain their existing fail-closed finite-value validation.

## Bounded solve stages

When the unknown appears in several denominators or piecewise routing paths,
a closed-form expression stops being practical. A pipeline entry may therefore
declare `solve` instead of `expr`. This complete GameDef solves the greatest
incoming hit whose two armour-mitigated components consume at most `pool`:

```json
{
  "stats": ["armour", "pool", "max_hit_search_upper"],
  "pipeline": [
    {
      "name": "max_hit",
      "solve": {
        "variable": "incoming_hit",
        "residual": "0.2 * incoming_hit * (1 - armour / (armour + 2 * incoming_hit)) + 0.8 * incoming_hit * (1 - armour / (armour + 8 * incoming_hit)) - pool",
        "lower": "0",
        "upper": "max_hit_search_upper",
        "absolute_tolerance": 1e-7,
        "relative_tolerance": 1e-9,
        "max_iterations": 128
      }
    },
    { "name": "max_whole_hit", "expr": "floor(max_hit)" }
  ],
  "objectives": ["max_hit", "max_whole_hit"]
}
```

The residual must be monotone non-decreasing over the configured interval.
Values `<= 0` mean feasible; values `> 0` mean the modeled pool or constraint
has been exceeded. The local `variable` is visible only in `residual` and may
not collide with a declared or engine name. `lower` and `upper` are compiled
once against stats, conditions, buckets, and earlier stages; forward
references and use of the local variable in a bound fail at plan compilation.

Evaluation uses deterministic bisection and maintains
`residual(lower) <= 0 <= residual(upper)`. It stops when the bracket width is
at most:

```text
absolute_tolerance + relative_tolerance * max(abs(lower), abs(upper))
```

The stage returns the greatest known feasible lower bound, so a later
`floor(max_hit)` remains conservative. If floating-point precision leaves no
representable midpoint, that same lower bound is returned. Otherwise, using
the complete `max_iterations` budget without meeting tolerance is an error;
the engine never silently returns an under-converged result. One stage may run
at most 4096 iterations, and it allocates no hidden state.

Compilation rejects invalid identifiers, name collisions, non-finite or
negative tolerances, two zero tolerances, and iteration budgets outside
`1..=4096`. Evaluation returns a contextual `PlanError` for non-finite bounds
or residual samples, inverted bounds, an unbracketed root, a non-finite
effective tolerance, or exhausted iterations. Solve stages are scalar rather
than event-branched; ordinary expression-stage JSON remains unchanged.

Unknown JSON keys and unresolved expression names fail closed with contextual
errors. Keys beginning with `_` are the one exception: they are ignored
annotations for human guidance, such as `_source` and `_guide`.

## Starting points

- [`guide/README.md`](guide/README.md) builds a complete model in seven small
  chapters.
- `crates/rtce/tests/fixtures/guide/` contains the exact runnable JSON for
  those chapters.
- [`CLI.md`](CLI.md) shows how to mount custom files into the Docker image.
- The published [browser tutorial](https://benjamin-small.github.io/rpg-theorycraft-engine/)
  lets you edit and run the same documents without installing a toolchain.
