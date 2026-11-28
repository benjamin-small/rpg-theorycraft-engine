import initWasm, {
  simulate_expected_stream,
  simulate_monte_carlo_stream,
} from './wasm/rtce_wasm.js';
import type { SimulationResult } from './rtce';
import type { SimulationEvent, TimelineMessage, TimelineRequest } from './timeline';

self.addEventListener('message', async ({ data }: MessageEvent<TimelineRequest>) => {
  const { config, mode, wasm } = data;
  const { gamedef, build, scenario, simdef, rotation } = config;

  try {
    if (!simdef || !rotation) {
      throw new Error('A simulation requires SimDef and Rotation documents');
    }

    await initWasm({ module_or_path: wasm });
    const onEvent = (json: string) => {
      const event = JSON.parse(json) as SimulationEvent;
      self.postMessage({ type: 'event', event } satisfies TimelineMessage);
    };
    const result = mode.kind === 'expected'
      ? simulate_expected_stream(gamedef, build, scenario, simdef, rotation, onEvent)
      : simulate_monte_carlo_stream(
          gamedef, build, scenario, simdef, rotation,
          mode.iterations, mode.seed, mode.eventIteration, onEvent,
        );
    self.postMessage({
      type: 'result',
      result: JSON.parse(result) as SimulationResult,
    } satisfies TimelineMessage);
  } catch (error) {
    self.postMessage({
      type: 'error',
      message: error instanceof Error ? error.message : String(error),
    } satisfies TimelineMessage);
  }
});
