import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import { evaluate, initSync } from "../src/wasm/rtce_wasm.js";

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
  }],
  objectives: ["root"],
});
const scenario = JSON.stringify({ phases: [{ name: "dummy", weight: 1 }] });

const first = evaluate(gamedef, "{}", scenario);
const second = evaluate(gamedef, "{}", scenario);
assert.equal(first, second, "Wasm solve output must be byte-repeatable");

const root = JSON.parse(first).objectives.root;
assert.ok(root * root <= 2, `Wasm result must be conservative, got ${root}`);
assert.ok(Math.abs(root - Math.sqrt(2)) <= 3e-12, `Wasm result missed tolerance: ${root}`);

console.log(`Wasm bounded solve passed: ${root}`);
