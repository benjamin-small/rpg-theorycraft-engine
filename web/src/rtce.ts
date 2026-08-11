import initWasm, {
  evaluate as wasmEvaluate,
  explain as wasmExplain,
  simulate_expected as wasmSimulateExpected,
  simulate_monte_carlo as wasmSimulateMonteCarlo,
} from './wasm/rtce_wasm.js';

export interface ConfigSet {
  gamedef: string;
  build: string;
  scenario: string;
  simdef?: string;
  rotation?: string;
}

export interface EvaluationResult {
  schema_version: 1;
  kind: 'evaluation';
  objectives: Record<string, number>;
}

export interface ExplanationResult {
  schema_version: 1;
  kind: 'explanation';
  objective_names: string[];
  trace: {
    objectives: number[];
    phases: Array<{
      name: string;
      weight: number;
      conditions: Array<[string, number]>;
      buckets: Array<[string, number]>;
      stages: Array<[string, number]>;
      branches: Array<{
        stage: string;
        fired: string[];
        weight: number;
        event_factors: number;
        value: number;
      }>;
    }>;
  };
}

export interface SimulationResult {
  schema_version: 1;
  kind: 'simulation';
  mode: 'expected' | 'monte_carlo';
  report: {
    total: { duration: number; total_damage: number; dps: number };
    actions: Record<string, { casts: number; damage: number; share: number }>;
    buffs: Record<string, { uptime: number; avg_stacks: number }>;
    condition_uptime: Record<string, number>;
    resources: Record<string, { time_capped: number; time_starved: number }>;
    proc_counts: Record<string, number>;
    distribution: null | {
      mean: number;
      std: number;
      p10: number;
      p50: number;
      p90: number;
    };
  };
}

export type RtceResult = EvaluationResult | ExplanationResult | SimulationResult;

let ready: Promise<void> | undefined;

function initialize(): Promise<void> {
  const inlinedWasm = (globalThis as { __RTCE_WASM__?: BufferSource }).__RTCE_WASM__;
  ready ??= initWasm(inlinedWasm ? { module_or_path: inlinedWasm } : undefined).then(
    () => undefined,
  );
  return ready;
}

function parse<T extends RtceResult>(json: string): T {
  return JSON.parse(json) as T;
}

function simulationInputs(config: ConfigSet): [string, string] {
  if (!config.simdef || !config.rotation) {
    throw new Error('This lesson has no SimDef or Rotation yet. Load lesson 5 or later.');
  }
  return [config.simdef, config.rotation];
}

/** Typed browser interface over the same Rust runner used by the CLI. */
export class RtceClient {
  static async create(): Promise<RtceClient> {
    await initialize();
    return new RtceClient();
  }

  evaluate(config: ConfigSet): EvaluationResult {
    return parse(wasmEvaluate(config.gamedef, config.build, config.scenario));
  }

  explain(config: ConfigSet): ExplanationResult {
    return parse(wasmExplain(config.gamedef, config.build, config.scenario));
  }

  simulateExpected(config: ConfigSet): SimulationResult {
    const [simdef, rotation] = simulationInputs(config);
    return parse(
      wasmSimulateExpected(config.gamedef, config.build, config.scenario, simdef, rotation),
    );
  }

  simulateMonteCarlo(
    config: ConfigSet,
    iterations = 1000,
    seed = 42,
  ): SimulationResult {
    const [simdef, rotation] = simulationInputs(config);
    return parse(
      wasmSimulateMonteCarlo(
        config.gamedef,
        config.build,
        config.scenario,
        simdef,
        rotation,
        iterations,
        BigInt(seed),
      ),
    );
  }
}
