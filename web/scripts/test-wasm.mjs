import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import {
  evaluate,
  initSync,
  simulate_expected_with_events,
  simulate_monte_carlo_with_events,
} from "../src/wasm/rtce_wasm.js";

const module = readFileSync(new URL("../src/wasm/rtce_wasm_bg.wasm", import.meta.url));
initSync({ module });

const gamedef = JSON.stringify({
  stats: [],
  pipeline: [{
    name: "root",
    solve: {
      variable: "x",
      residual: "x * x - 2",
      lower: "0",
      upper: "2",
      absolute_tolerance: 1e-12,
      relative_tolerance: 1e-12,
      max_iterations: 128,
    },
  }, {
    name: "fractional_hits",
    recurrence: {
      state: [
        { name: "pool", initial: "10", next: "max(pool - 3, 0)" },
        { name: "hits", initial: "0", next: "hits + 1" },
        { name: "overkill", initial: "0", next: "max(3 - pool, 0)" },
      ],
      until: "pool <= 0",
      result: "hits - overkill / 3",
      max_iterations: 10,
    },
  }, {
    name: "cascade",
    recurrence: {
      state: [
        { name: "a", initial: "12", next: "a - take_a" },
        { name: "b", initial: "7", next: "b - take_b" },
        { name: "steps", initial: "0", next: "steps + 1" },
        { name: "overkill", initial: "0", next: "rest - take_b" },
      ],
      locals: [
        { name: "take_a", expr: "min(5, a)" },
        { name: "rest", expr: "5 - take_a" },
        { name: "take_b", expr: "min(rest, b)" },
      ],
      until: "b <= 0",
      result: "steps - overkill / 5",
      max_iterations: 10,
    },
  }, {
    name: "batched_drain",
    recurrence: {
      state: [
        { name: "remaining", initial: "1000000", next: "max(remaining - spent, 0)" },
        { name: "steps", initial: "0", next: "steps + n" },
        { name: "overkill", initial: "0", next: "max(spent - remaining, 0)" },
      ],
      locals: [{ name: "spent", expr: "3 * n" }],
      batch: { symbol: "n" },
      until: "remaining <= 0",
      result: "steps - overkill / 3",
      max_iterations: 37,
    },
  }],
  objectives: ["root", "fractional_hits", "cascade", "batched_drain"],
});
const scenario = JSON.stringify({ phases: [{ name: "dummy", weight: 1 }] });

const first = evaluate(gamedef, "{}", scenario);
const second = evaluate(gamedef, "{}", scenario);
assert.equal(first, second, "Wasm solve output must be byte-repeatable");

const root = JSON.parse(first).objectives.root;
assert.ok(root * root <= 2, `Wasm result must be conservative, got ${root}`);
assert.ok(Math.abs(root - Math.sqrt(2)) <= 3e-12, `Wasm result missed tolerance: ${root}`);
const fractionalHits = JSON.parse(first).objectives.fractional_hits;
assert.ok(
  Math.abs(fractionalHits - 10 / 3) <= 1e-12,
  `Wasm recurrence missed fractional final hit: ${fractionalHits}`,
);

const { cascade, batched_drain: batchedDrain } = JSON.parse(first).objectives;
assert.ok(Math.abs(cascade - 3.8) <= 1e-12, `Wasm recurrence locals mismatch: ${cascade}`);
assert.ok(
  Math.abs(batchedDrain - 1_000_000 / 3) <= 1e-6,
  `Wasm batched recurrence mismatch: ${batchedDrain}`,
);

const fixture = (name) => readFileSync(
  new URL(`../../crates/rtce/tests/fixtures/poe2/${name}`, import.meta.url),
  "utf8",
);
const pobGame = fixture("issue22-recurrence-gamedef.json");
const pobScenario = fixture("issue22-scenario.json");
const pobCases = [
  ["issue22-base-build.json", 17582.417582418, 0],
  ["issue22-block-build.json", 19008.019008019, 10],
];
for (const [build, expectedEhp, expectedBlock] of pobCases) {
  const output = JSON.parse(evaluate(pobGame, fixture(build), pobScenario)).objectives;
  assert.ok(Math.abs(output.total_ehp - expectedEhp) < 0.01, `Wasm PoB EHP mismatch: ${output.total_ehp}`);
  assert.ok(
    Math.abs(output.effective_block_chance - expectedBlock) < 0.01,
    `Wasm PoB block mismatch: ${output.effective_block_chance}`,
  );
}

console.log(`Wasm bounded solve passed: ${root}; recurrence passed: ${fractionalHits}; locals: ${cascade}; batched: ${batchedDrain}`);

const simdef = JSON.stringify({
  actions: { shot: { cast_time: "1", damage: {} } },
  damage_objective: "root",
});
const rotation = JSON.stringify({ rules: [{ action: "shot" }] });
const eventInputs = [gamedef, "{}", scenario, simdef, rotation];
const expected = JSON.parse(simulate_expected_with_events(...eventInputs));
const sampled = JSON.parse(simulate_monte_carlo_with_events(...eventInputs, 2, 7n, 1));
for (const output of [expected, sampled]) {
  const { events } = output.event_stream;
  assert.deepEqual(events.map(({ type, time }) => [type, time]), [
    ["simulation_started", 0],
    ["phase_entered", 0],
    ["cast_started", 0],
    ["cast_landed", 1],
    ["simulation_ended", 1],
  ]);
  assert.equal(events[2].cast_id, events[3].cast_id);
  assert.equal(events[3].damage, root);
  assert.equal(events[4].total_damage, root);
}
assert.equal(expected.event_stream.iteration, 0);
assert.equal(sampled.event_stream.iteration, 1);
assert.equal(sampled.event_stream.seed, "7");
assert.equal(sampled.report.distribution.iterations, 2);
console.log("Wasm simulation event stream passed in expected and sampled modes.");
