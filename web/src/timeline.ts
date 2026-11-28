import type { ConfigSet, SimulationResult } from './rtce';
import TimelineWorker from './timeline.worker?worker&inline';
import wasmUrl from './wasm/rtce_wasm_bg.wasm?url';

export type TimelineMode =
  | { kind: 'expected' }
  | { kind: 'monte_carlo'; iterations: number; seed: bigint; eventIteration: number };

type Crit =
  | { type: 'expected'; probability: number }
  | { type: 'shared'; critical: boolean }
  | { type: 'counted'; critical_hits: number; sampled_hits: number };

/** Shared timeline fields plus the event variants supplied by a simulator. */
export type SimulationEvent<Details extends { type: string } = RtceEventDetails> = {
  time: number;
  source_id: string | null;
  target_id: string | null;
} & Details;

type RtceEventDetails =
  | {
      type: 'simulation_started'; duration: number; schema_version: 1;
      mode: TimelineMode['kind']; iteration: number; seed: string | null;
    }
  | { type: 'phase_entered'; phase: string; phase_index: number }
  | {
      type: 'cast_started'; action: string; cast_id: number; rule: number | null;
      triggered_by: string | null; cooldown_ready_at: number | null; gcd_ready_at: number | null;
    }
  | { type: 'segment_started'; cast_id: number; segment: number; duration: number }
  | {
      type: 'cast_landed'; cast_id: number; segment: number | null; completes_cast: boolean;
      damage: number | null; hits: number | null; crit: Crit | null;
    }
  | { type: 'cast_cancelled'; cast_id: number; segments_landed: number; replacement: string }
  | { type: 'proc_fired'; proc: string; cast_id: number; icd_ready_at: number }
  | {
      type: 'buff_changed'; buff: string;
      change: 'applied' | 'application_ignored' | 'expired' | 'removed';
      stacks: number; next_expiry: number | null; cast_id: number | null;
    }
  | {
      type: 'expected_buff_changed'; buff: string; probability: number; roll_chance: number | null;
      roll_expires_at: number | null; cast_id: number | null;
    }
  | { type: 'buff_damage'; buff: string; since: number; damage: number }
  | {
      type: 'resource_changed'; resource: string; change: 'initial' | 'cost' | 'gain' | 'limits';
      amount: number; max: number; regen_per_second: number; cast_id: number | null;
    }
  | { type: 'simulation_ended'; total_damage: number };

export interface TimelineRequest {
  config: ConfigSet;
  mode: TimelineMode;
  wasm: string | BufferSource;
}

export type TimelineMessage<Event = SimulationEvent, Report = SimulationResult> =
  | { type: 'event'; event: Event }
  | { type: 'result'; result: Report }
  | { type: 'error'; message: string };

export interface TimelineStream<Event = SimulationEvent, Report = SimulationResult> {
  events: ReadableStream<Event>;
  result: Promise<Report>;
}

/** Run a simulation in a dedicated Worker and deliver its events individually. */
export function streamSimulation(
  config: ConfigSet,
  mode: TimelineMode = { kind: 'expected' },
  signal?: AbortSignal,
): TimelineStream {
  const worker = new TimelineWorker();
  let onAbort: (() => void) | undefined;
  const stopWorker = () => {
    if (onAbort) signal?.removeEventListener('abort', onAbort);
    worker.onmessage = null;
    worker.onmessageerror = null;
    worker.onerror = null;
    worker.terminate();
  };
  let resolveResult!: (result: SimulationResult) => void;
  let rejectResult!: (reason: unknown) => void;
  const result = new Promise<SimulationResult>((resolve, reject) => {
    resolveResult = resolve;
    rejectResult = reject;
  });
  // An event-only consumer can observe errors through the stream.
  void result.catch(() => {});

  const events = new ReadableStream<SimulationEvent>({
    start(controller) {
      const fail = (error: unknown) => {
        stopWorker();
        controller.error(error);
        rejectResult(error);
      };

      if (signal) {
        onAbort = () => fail(signal.reason);
        if (signal.aborted) {
          onAbort();
          return;
        }
        signal.addEventListener('abort', onAbort, { once: true });
      }

      worker.onmessage = ({ data }: MessageEvent<TimelineMessage>) => {
        switch (data.type) {
          case 'event':
            controller.enqueue(data.event);
            break;
          case 'result':
            stopWorker();
            controller.close();
            resolveResult(data.result);
            break;
          case 'error':
            fail(new Error(data.message));
            break;
        }
      };
      worker.onerror = (event) => {
        event.preventDefault();
        fail(new Error(event.message || 'Simulation Worker failed'));
      };
      worker.onmessageerror = () => fail(new Error('Could not read the simulation Worker message'));

      try {
        // The standalone tutorial supplies its embedded Wasm to the inline Worker.
        const embedded = (globalThis as { __RTCE_WASM__?: BufferSource }).__RTCE_WASM__;
        const wasm = embedded ?? new URL(wasmUrl, document.baseURI).href;
        worker.postMessage({ config, mode, wasm } satisfies TimelineRequest);
      } catch (error) {
        fail(error instanceof Error ? error : new Error(String(error)));
      }
    },
    cancel(reason) {
      stopWorker();
      rejectResult(reason ?? new DOMException('Simulation stream cancelled', 'AbortError'));
    },
  });

  return { events, result };
}
