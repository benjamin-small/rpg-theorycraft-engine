import {
  BrowserTerminal,
  type CommandCtx,
  type FlagSpec,
} from '@benjamin-small/browser-terminal';
import { lessons, type Lesson } from './lessons';
import {
  RtceClient,
  type ConfigSet,
  type LexiconEntry,
  type RtceResult,
  type SimulationResult,
} from './rtce';
import './style.css';

type ConfigKey = keyof ConfigSet;

const configLabels: Record<ConfigKey, string> = {
  gamedef: 'GameDef',
  build: 'Build · gear & stats',
  scenario: 'Scenario',
  simdef: 'SimDef',
  rotation: 'Rotation',
};

const configHints: Record<ConfigKey, string> = {
  gamedef: 'Game rules: stat names, modifier buckets, and the formulas that produce each answer.',
  build: 'Your loadout: `_source` names the item or affix for humans; bucket, value, event, and condition are the parts used by the damage math.',
  scenario: 'The encounter: target defenses, fight phases, and conditional uptime.',
  simdef: 'Combat rules: skills, resources, buffs, cooldowns, and the fight clock.',
  rotation: 'Your button priority: which skill the character tries to use next.',
};

const calcFlags: FlagSpec[] = [
  { long: 'game', shape: 'str', desc: 'GameDef JSON (use $game)' },
  { long: 'build', shape: 'str', desc: 'Build JSON (use $build)' },
  { long: 'scenario', shape: 'str', desc: 'Scenario JSON (use $scenario)' },
];

const simFlags: FlagSpec[] = [
  ...calcFlags,
  { long: 'sim', shape: 'str', desc: 'SimDef JSON (use $sim)' },
  { long: 'rotation', shape: 'str', desc: 'Rotation JSON (use $rotation)' },
];

const cliFlagToConfigKey = {
  game: 'gamedef',
  build: 'build',
  scenario: 'scenario',
  sim: 'simdef',
  rotation: 'rotation',
} as const satisfies Record<string, ConfigKey>;

const app = document.querySelector<HTMLDivElement>('#app');
if (!app) throw new Error('missing #app');

app.innerHTML = `
  <header class="masthead">
    <a class="brand" href="#top" aria-label="RPG Theory Crafting Engine home">
      <span class="brand-mark">R</span>
      <span><strong>RPG Theory Crafting Engine</strong><small>Stat Sheets &amp; Simulations</small></span>
    </a>
    <div class="mast-meta"><span>Interactive config lab</span><span class="status"><i></i> Rust · Wasm</span></div>
  </header>
  <main id="top" class="shell">
    <aside class="rail" aria-label="Tutorial lessons">
      <p class="rail-label">Seven field notes</p>
      <nav id="lesson-nav"></nav>
      <p class="rail-foot">Every editor is live. Break a name, run again, and read the fail-closed error.</p>
    </aside>
    <section class="workbench">
      <div class="lesson-head">
        <div><p id="eyebrow" class="eyebrow"></p><h1 id="lesson-title"></h1><p id="lesson-summary" class="lede"></p></div>
        <div class="step-stamp"><span id="step-number"></span><small>/ 07</small></div>
      </div>
      <div id="gamer-guide" class="gamer-guide">
        <span id="lesson-mode" class="mode-chip"></span>
        <div><strong>What this means in-game</strong><p id="gamer-summary"></p></div>
      </div>
      <div class="insight"><span>Engine note</span><p id="lesson-insight"></p></div>
      <div class="keyword-example">
        <span id="keyword-label"></span>
        <div><code id="keyword-code"></code><p id="keyword-summary"></p></div>
        <button id="keyword-lookup" type="button">Look it up</button>
      </div>
      <details id="config-lexicon" class="config-lexicon">
        <summary><span><strong>Config lexicon</strong><small>Schema, declared names, built-ins, and engine context—labeled honestly.</small></span><code>rtce lexicon</code></summary>
        <div class="lexicon-body">
          <label for="lexicon-search">Search the dictionary</label>
          <input id="lexicon-search" type="search" placeholder="Try buff, clamp, event, resource…" autocomplete="off">
          <div id="lexicon-list" class="lexicon-list"></div>
        </div>
      </details>
      <div class="editor-card">
        <div class="editor-bar">
          <div id="editor-tabs" class="tabs" role="tablist"></div>
          <button id="reset-config" class="text-button" type="button">Reset lesson</button>
        </div>
        <textarea id="config-editor" spellcheck="false" aria-label="JSON configuration editor"></textarea>
        <p id="config-hint" class="config-hint"></p>
      </div>
      <div class="run-row">
        <div><span>Try in the terminal</span><code id="lesson-command"></code></div>
        <button id="run-button" class="run-button" type="button"><span>Run current lesson</span><b>⌘↵</b></button>
      </div>
      <section class="result-card" aria-live="polite">
        <div class="result-head"><span id="result-label">Calculated sheet result</span><strong id="result-badge">Ready</strong></div>
        <div id="result-summary" class="result-summary"><p>Edit a config or run the first lesson.</p></div>
        <details><summary>Raw JSON</summary><pre id="raw-result">No run yet.</pre></details>
      </section>
    </section>
    <section class="terminal-column" aria-label="Interactive browser terminal">
      <div class="terminal-head"><span><i></i> rtce shell · in-memory config</span><small>Type <code>help</code> or <code>config list</code></small></div>
      <div id="terminal"></div>
      <div class="terminal-foot"><span>Live editor JSON is injected into the shell</span><code>--game $game · --build $build</code></div>
    </section>
  </main>
`;

const byId = <T extends HTMLElement>(id: string): T => {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing #${id}`);
  return element as T;
};

const nav = byId<HTMLElement>('lesson-nav');
const tabs = byId<HTMLElement>('editor-tabs');
const editor = byId<HTMLTextAreaElement>('config-editor');
const resultBadge = byId<HTMLElement>('result-badge');
const resultSummary = byId<HTMLElement>('result-summary');
const rawResult = byId<HTMLElement>('raw-result');

function prettyConfig(config: ConfigSet): ConfigSet {
  const pretty = { ...config };
  for (const key of Object.keys(config) as ConfigKey[]) {
    const document = config[key];
    if (document !== undefined) {
      pretty[key] = JSON.stringify(JSON.parse(document), null, 2);
    }
  }
  return pretty;
}

let currentLesson = lessons[0];
let currentKey: ConfigKey = 'gamedef';
let workingConfig: ConfigSet = prettyConfig(currentLesson.config);
let terminal: BrowserTerminal | undefined;

function availableKeys(config: ConfigSet): ConfigKey[] {
  return (Object.keys(config) as ConfigKey[]).filter((key) => config[key] !== undefined);
}

function saveEditor(): void {
  workingConfig[currentKey] = editor.value;
}

function syncShellVariables(): void {
  if (!terminal) return;

  const variables: Record<string, string> = {};
  for (const [name, key] of Object.entries(cliFlagToConfigKey) as Array<[string, ConfigKey]>) {
    const document = workingConfig[key];
    if (document === undefined) {
      terminal.unsetVariable(name);
    } else {
      variables[name] = document;
    }
  }
  terminal.setVariables(variables);
}

function selectTab(key: ConfigKey): void {
  saveEditor();
  currentKey = key;
  editor.value = workingConfig[key] ?? '';
  byId('config-hint').textContent = configHints[key];
  [...tabs.querySelectorAll('button')].forEach((button) => {
    const selected = button.dataset.key === key;
    button.classList.toggle('active', selected);
    button.setAttribute('aria-selected', String(selected));
  });
}

function drawTabs(): void {
  tabs.replaceChildren(
    ...availableKeys(workingConfig).map((key) => {
      const button = document.createElement('button');
      button.type = 'button';
      button.dataset.key = key;
      button.textContent = configLabels[key];
      button.setAttribute('role', 'tab');
      button.addEventListener('click', () => selectTab(key));
      return button;
    }),
  );
  if (!availableKeys(workingConfig).includes(currentKey)) currentKey = 'gamedef';
  editor.value = workingConfig[currentKey] ?? '';
  selectTab(currentKey);
}

function loadLesson(number: number): Lesson {
  const lesson = lessons.find((candidate) => candidate.number === number);
  if (!lesson) throw new Error(`lesson must be between 1 and ${lessons.length}`);
  currentLesson = lesson;
  workingConfig = prettyConfig(lesson.config);
  currentKey = 'gamedef';
  byId('eyebrow').textContent = lesson.eyebrow;
  byId('lesson-title').textContent = lesson.title;
  byId('lesson-summary').textContent = lesson.summary;
  byId('gamer-summary').textContent = lesson.gamerSummary;
  byId('lesson-insight').textContent = lesson.insight;
  byId('keyword-label').textContent = lesson.keywordExample.label;
  byId('keyword-code').textContent = lesson.keywordExample.code;
  byId('keyword-summary').textContent = lesson.keywordExample.summary;
  const mode = byId('lesson-mode');
  mode.textContent = lesson.mode === 'sheet' ? 'Sheet calculation · no timeline' : 'Combat simulation · running timeline';
  mode.dataset.mode = lesson.mode;
  byId('result-label').textContent = lesson.mode === 'sheet' ? 'Calculated sheet result' : 'Simulated fight result';
  byId('step-number').textContent = String(lesson.number).padStart(2, '0');
  byId('lesson-command').textContent = lesson.command;
  [...nav.querySelectorAll('button')].forEach((button) =>
    button.classList.toggle('active', Number(button.dataset.lesson) === number),
  );
  drawTabs();
  syncShellVariables();
  resultBadge.textContent = 'Ready';
  resultSummary.innerHTML = '<p>Configuration loaded. Run it as-is, then change one value and compare.</p>';
  rawResult.textContent = 'No run yet.';
  return lesson;
}

nav.replaceChildren(
  ...lessons.map((lesson) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.dataset.lesson = String(lesson.number);
    button.innerHTML = `<span>${String(lesson.number).padStart(2, '0')}</span><p><strong>${lesson.eyebrow}</strong><small>${lesson.title}</small></p>`;
    button.addEventListener('click', () => loadLesson(lesson.number));
    return button;
  }),
);

function editedConfig(): ConfigSet {
  saveEditor();
  return { ...workingConfig };
}

function configFromFlags(
  flags: Record<string, unknown>,
  required: Array<keyof typeof cliFlagToConfigKey>,
): ConfigSet {
  const selected: Partial<ConfigSet> = {};

  for (const flag of required) {
    const value = flags[flag];
    if (typeof value !== 'string' || value.length === 0) {
      throw new Error(`missing required argument --${flag} <VAR>`);
    }
    selected[cliFlagToConfigKey[flag]] = value;
  }

  return selected as ConfigSet;
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>]/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[character] ?? character);
}

function eventBranchLabel(fired: string[]): string {
  return fired.length === 0 ? 'normal hit' : `${fired.join(' + ')} hit`;
}

function formatPercent(fraction: number): string {
  return `${(fraction * 100).toFixed(2).replace(/\.?0+$/, '')}%`;
}

function summarize(result: RtceResult): string {
  if (result.kind === 'evaluation') {
    return Object.entries(result.objectives)
      .map(([name, value]) => `<article><span>${escapeHtml(name.replaceAll('_', ' '))}</span><strong>${value.toFixed(3)}</strong></article>`)
      .join('');
  }
  if (result.kind === 'explanation') {
    const branchStages = new Set(result.trace.phases.flatMap((phase) => phase.branches.map((branch) => branch.stage)));
    const showBranchStage = branchStages.size > 1;
    const branchCards = result.trace.phases.flatMap((phase) => phase.branches.map((branch) => {
      const phaseLabel = result.trace.phases.length > 1 ? `${phase.name} · ` : '';
      const stageLabel = showBranchStage ? `${branch.stage} · ` : '';
      return `<article><span>${escapeHtml(phaseLabel + stageLabel + eventBranchLabel(branch.fired))} · ${formatPercent(branch.weight)} chance</span><strong>${branch.value.toFixed(1)} damage</strong><small>optional factor trace ×${branch.event_factors.toFixed(2)}</small></article>`;
    }));
    const objectiveCards = result.objective_names.map((name, index) =>
      `<article class="average-card"><span>weighted average · ${escapeHtml(name.replaceAll('_', ' '))}</span><strong>${result.trace.objectives[index].toFixed(1)} damage</strong><small>your calculated sheet result</small></article>`,
    );
    return [...branchCards, ...objectiveCards].join('');
  }
  const distribution = result.report.distribution;
  return `<article><span>DPS</span><strong>${result.report.total.dps.toFixed(3)}</strong></article><article><span>total damage</span><strong>${result.report.total.total_damage.toFixed(1)}</strong></article><article><span>duration</span><strong>${result.report.total.duration.toFixed(0)}s</strong></article>${distribution ? `<article><span>p10 · p50 · p90</span><strong>${distribution.p10.toFixed(1)} · ${distribution.p50.toFixed(1)} · ${distribution.p90.toFixed(1)}</strong></article>` : ''}`;
}

function showResult<T extends RtceResult>(result: T): T {
  resultBadge.textContent = result.kind === 'simulation' ? result.mode.replace('_', ' ') : result.kind;
  resultSummary.innerHTML = summarize(result);
  rawResult.textContent = JSON.stringify(result, null, 2);
  return result;
}

function showError(error: unknown): never {
  const message = error instanceof Error ? error.message : String(error);
  const cliArgumentError = /^(missing required argument|unknown config variable|invalid value for --)/.test(message);
  resultBadge.textContent = 'Error';
  resultSummary.innerHTML = `<p class="error-copy">${escapeHtml(message)}</p>`;
  rawResult.textContent = message;
  throw {
    message,
    help: cliArgumentError
      ? 'Run config list to inspect variables, or append --help to the rtce subcommand.'
      : 'Inspect the active JSON editor; config errors include their document and position.',
  };
}

function waitForPlayback(signal: AbortSignal, milliseconds: number): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(new Error('playback cancelled'));
      return;
    }
    const onAbort = () => {
      window.clearTimeout(timer);
      reject(new Error('playback cancelled'));
    };
    const timer = window.setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, milliseconds);
    signal.addEventListener('abort', onAbort, { once: true });
  });
}

async function streamLines(ctx: CommandCtx, lines: string[], delay = 18): Promise<void> {
  for (const line of lines) {
    ctx.log(line);
    await waitForPlayback(ctx.signal, delay);
  }
}

function configCount(config: ConfigSet, key: ConfigKey): number {
  const document = config[key];
  if (!document) return 0;
  const parsed = JSON.parse(document) as Record<string, unknown>;
  return Object.keys(parsed).filter((name) => !name.startsWith('_')).length;
}

function gearContributionLines(config: ConfigSet): string[] {
  const build = JSON.parse(config.build) as {
    contributions?: Array<{
      _source?: unknown;
      bucket?: unknown;
      value?: unknown;
      event?: unknown;
      condition?: unknown;
    }>;
  };

  return (build.contributions ?? []).flatMap((contribution) => {
    if (
      typeof contribution._source !== 'string'
      || typeof contribution.bucket !== 'string'
      || typeof contribution.value !== 'number'
    ) return [];

    const gates = [
      typeof contribution.event === 'string' ? `when ${contribution.event} procs` : '',
      typeof contribution.condition === 'string' ? `while ${contribution.condition}` : '',
    ].filter(Boolean);
    const gate = gates.length > 0 ? ` ${gates.join(' and ')}` : '';
    return [`[gear] ${contribution._source} → +${contribution.value}% ${contribution.bucket}${gate}`];
  });
}

function evaluationPlayback(result: RtceResult, config: ConfigSet): string[] {
  const game = JSON.parse(config.gamedef) as {
    stats?: unknown[];
    buckets?: Record<string, unknown>;
    events?: Record<string, unknown>;
    pipeline?: unknown[];
  };
  const lines = [
    `[compile] GameDef · ${game.stats?.length ?? 0} stats · ${Object.keys(game.buckets ?? {}).length} buckets · ${Object.keys(game.events ?? {}).length} events · ${game.pipeline?.length ?? 0} stages`,
    `[resolve] Build + Scenario · ${configCount(config, 'build')} build sections · ${configCount(config, 'scenario')} scenario sections`,
    ...gearContributionLines(config),
  ];

  if (result.kind === 'evaluation') {
    for (const [name, value] of Object.entries(result.objectives)) {
      lines.push(`[objective] ${name} = ${value.toFixed(4)}`);
    }
  } else if (result.kind === 'explanation') {
    const branchStages = new Set(result.trace.phases.flatMap((phase) => phase.branches.map((branch) => branch.stage)));
    const showBranchStage = branchStages.size > 1;
    for (const phase of result.trace.phases) {
      lines.push(`[phase] ${phase.name} · weight ${(phase.weight * 100).toFixed(1)}%`);
      for (const branch of phase.branches) {
        const stageLabel = showBranchStage ? `${branch.stage} · ` : '';
        lines.push(
          `[event] ${stageLabel}${eventBranchLabel(branch.fired)} · ${formatPercent(branch.weight)} chance · ${branch.value.toFixed(2)} damage · optional factor trace ×${branch.event_factors.toFixed(2)}`,
        );
      }
    }
    result.objective_names.forEach((name, index) => {
      lines.push(`[objective] ${name} = ${result.trace.objectives[index].toFixed(4)}`);
    });
    if (
      result.trace.phases.length === 1
      && result.trace.phases[0].branches.length > 1
      && result.objective_names.length === 1
      && branchStages.size === 1
      && branchStages.has(result.objective_names[0])
    ) {
      const terms = result.trace.phases[0].branches
        .map((branch) => `${formatPercent(branch.weight)} × ${branch.value.toFixed(1)}`)
        .join(' + ');
      lines.push(`[average] ${terms} = ${result.trace.objectives[0].toFixed(1)} damage`);
    }
  }
  lines.push('[done] result committed to the workbench');
  return lines;
}

function simulationPlayback(result: SimulationResult, config: ConfigSet): string[] {
  const { report } = result;
  const lines = [
    `[compile] Plan + SimDef + Rotation ready`,
    ...gearContributionLines(config),
    `[timeline] ${report.total.duration.toFixed(1)}s · ${result.mode.replace('_', ' ')}`,
    '[playback] report-derived stream; cast counts and damage totals are exact',
  ];
  const events: Array<{ time: number; text: string }> = [];

  for (const [name, action] of Object.entries(report.actions)) {
    const damagePerCast = action.casts === 0 ? 0 : action.damage / action.casts;
    for (let index = 0; index < action.casts; index += 1) {
      const time = ((index + 1) * report.total.duration) / Math.max(action.casts, 1);
      events.push({
        time,
        text:
          damagePerCast > 0
            ? `[hit] ${name} → ${damagePerCast.toFixed(2)} damage`
            : `[cast] ${name}`,
      });
    }
  }

  events.sort((left, right) => left.time - right.time || left.text.localeCompare(right.text));
  for (const event of events.slice(0, 120)) {
    lines.push(`t=${event.time.toFixed(2).padStart(6)}s  ${event.text}`);
  }
  if (events.length > 120) lines.push(`[playback] ${events.length - 120} additional events omitted`);

  for (const [name, buff] of Object.entries(report.buffs)) {
    lines.push(
      `[buff] ${name} · ${(buff.uptime * 100).toFixed(1)}% uptime · ${buff.avg_stacks.toFixed(2)} avg stacks`,
    );
  }
  if (report.distribution) {
    const distribution = report.distribution;
    lines.push(
      `[distribution] mean=${distribution.mean.toFixed(3)} · std=${distribution.std.toFixed(3)} · p10=${distribution.p10.toFixed(3)} · p50=${distribution.p50.toFixed(3)} · p90=${distribution.p90.toFixed(3)}`,
    );
  }
  lines.push(
    `[done] ${report.total.total_damage.toFixed(2)} damage / ${report.total.duration.toFixed(1)}s = ${report.total.dps.toFixed(4)} DPS`,
  );
  return lines;
}

const client = await RtceClient.create();
const lexicon = client.lexicon();

function renderLexicon(entries: LexiconEntry[], query = ''): void {
  const needle = query.trim().toLowerCase();
  const matches = entries.filter((entry) =>
    [entry.term, entry.kind, entry.scope, entry.meaning, entry.example, ...(entry.aliases ?? [])]
      .some((value) => value.toLowerCase().includes(needle)),
  );
  byId('lexicon-list').innerHTML = matches.length === 0
    ? '<p class="lexicon-empty">No matching term. Try a broader word.</p>'
    : matches.map((entry) => `
      <article>
        <div><code>${escapeHtml(entry.term)}</code><span data-kind="${entry.kind}">${entry.kind}</span></div>
        <small>${escapeHtml(entry.scope)}</small>
        <p>${escapeHtml(entry.meaning)}</p>
        <pre>${escapeHtml(entry.example)}</pre>
        ${entry.aliases ? `<em>Aliases: ${entry.aliases.map(escapeHtml).join(', ')}</em>` : ''}
      </article>
    `).join('');
}

renderLexicon(lexicon.entries);
byId<HTMLInputElement>('lexicon-search').addEventListener('input', (event) => {
  renderLexicon(lexicon.entries, (event.currentTarget as HTMLInputElement).value);
});
byId('keyword-lookup').addEventListener('click', () => {
  const details = byId<HTMLDetailsElement>('config-lexicon');
  const search = byId<HTMLInputElement>('lexicon-search');
  details.open = true;
  search.value = currentLesson.keywordExample.lookup;
  renderLexicon(lexicon.entries, search.value);
  search.focus();
});

const inlinedTerminalWasm = (globalThis as { __BTERM_WASM__?: BufferSource }).__BTERM_WASM__;
terminal = await BrowserTerminal.create({
  mount: byId('terminal'),
  wasmBinary: inlinedTerminalWasm,
});

terminal.registerCommand(
  { name: 'rtce lexicon', summary: 'List config schema, declared names, expression tools, and engine context' },
  () => lexicon,
);
terminal.registerCommand(
  { name: 'lesson list', summary: 'List the seven config-building lessons' },
  () => lessons.map(({ number, eyebrow, title, command }) => ({ number, lesson: eyebrow, goal: title, command })),
);
terminal.registerCommand(
  {
    name: 'lesson load',
    summary: 'Load one lesson into the live JSON editors',
    required: [{ name: 'number', shape: 'int', desc: 'lesson number, 1 through 7' }],
  },
  ({ positionals }) => {
    const lesson = loadLesson(Number(positionals[0]));
    return { loaded: lesson.number, title: lesson.title, next: lesson.command };
  },
);
terminal.registerCommand(
  { name: 'config list', summary: 'List editor-backed variables available to CLI flags' },
  () => {
    saveEditor();
    syncShellVariables();
    return (Object.entries(cliFlagToConfigKey) as Array<[string, ConfigKey]>)
      .filter(([, key]) => workingConfig[key] !== undefined)
      .map(([flag, key]) => ({ variable: `$${flag}`, flag: `--${flag}`, source: `${configLabels[key]} editor` }));
  },
);
terminal.registerCommand(
  {
    name: 'config show',
    summary: 'Return one live editor document as parsed structured data',
    required: [{ name: 'document', shape: 'str', desc: 'gamedef, build, scenario, simdef, or rotation' }],
  },
  ({ positionals }) => {
    const key = String(positionals[0]) as ConfigKey;
    if (!availableKeys(editedConfig()).includes(key)) {
      throw { message: `no ${key} document in lesson ${currentLesson.number}`, help: 'Load lesson 5 or later for simdef and rotation.' };
    }
    return JSON.parse(workingConfig[key] ?? 'null');
  },
);
terminal.registerCommand(
  {
    name: 'rtce evaluate',
    summary: 'Compile and evaluate named GameDef, Build, and Scenario inputs',
    flags: calcFlags,
  },
  async ({ flags }, _input, ctx) => {
    try {
      const config = configFromFlags(flags, ['game', 'build', 'scenario']);
      const result = showResult(client.evaluate(config));
      await streamLines(ctx, evaluationPlayback(result, config));
      return result;
    } catch (error) { return showError(error); }
  },
);
terminal.registerCommand(
  {
    name: 'rtce explain',
    summary: 'Evaluate named inputs with phase, stage, and event-branch traces',
    flags: calcFlags,
  },
  async ({ flags }, _input, ctx) => {
    try {
      const config = configFromFlags(flags, ['game', 'build', 'scenario']);
      const result = showResult(client.explain(config));
      await streamLines(ctx, evaluationPlayback(result, config));
      return result;
    } catch (error) { return showError(error); }
  },
);
terminal.registerCommand(
  {
    name: 'rtce simulate',
    summary: 'Run the live timeline config in expected-value or Monte Carlo mode',
    flags: [
      ...simFlags,
      { long: 'mode', shape: 'str', desc: 'expected or monte-carlo (default expected)' },
      { long: 'iterations', shape: 'int', desc: 'Monte Carlo timelines (default 1000)' },
      { long: 'seed', shape: 'int', desc: 'Monte Carlo master seed (default 42)' },
    ],
  },
  async ({ flags }, _input, ctx) => {
    try {
      const config = configFromFlags(flags, ['game', 'build', 'scenario', 'sim', 'rotation']);
      const mode = String(flags.mode ?? 'expected');
      if (mode !== 'expected' && mode !== 'monte-carlo') {
        throw new Error("invalid value for --mode; use 'expected' or 'monte-carlo'");
      }
      const iterations = Number(flags.iterations ?? 1000);
      const seed = Number(flags.seed ?? 42);
      const result = showResult(
        mode === 'monte-carlo'
          ? client.simulateMonteCarlo(config, iterations, seed)
          : client.simulateExpected(config),
      );
      await streamLines(ctx, simulationPlayback(result, config), 10);
      return result;
    } catch (error) { return showError(error); }
  },
);
terminal.registerCommand(
  { name: 'rtce reset', summary: 'Restore the current lesson’s committed configuration' },
  () => {
    const lesson = loadLesson(currentLesson.number);
    return { reset: lesson.number, title: lesson.title };
  },
);

function submitTerminalCommand(command: string): void {
  saveEditor();
  syncShellVariables();
  const input = byId('terminal').querySelector<HTMLTextAreaElement>('.xterm-helper-textarea');
  if (!input) throw new Error('terminal input is not ready');
  input.focus();
  input.value = command;
  input.dispatchEvent(
    new InputEvent('input', {
      bubbles: true,
      data: command,
      inputType: 'insertText',
    }),
  );
  const enter = new KeyboardEvent('keydown', {
    bubbles: true,
    cancelable: true,
    key: 'Enter',
    code: 'Enter',
  });
  // xterm's keyboard mapper still reads the legacy numeric fields for
  // control keys. Synthetic KeyboardEvents leave them at zero unless we
  // supply them explicitly; a physical Enter and this button-driven Enter
  // must take the exact same onData → core.feed path.
  Object.defineProperties(enter, {
    keyCode: { get: () => 13 },
    which: { get: () => 13 },
  });
  input.dispatchEvent(enter);
}

byId('run-button').addEventListener('click', () => submitTerminalCommand(currentLesson.command));
byId('reset-config').addEventListener('click', () => loadLesson(currentLesson.number));
editor.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
    event.preventDefault();
    submitTerminalCommand(currentLesson.command);
  }
  if (event.key === 'Tab') {
    event.preventDefault();
    const start = editor.selectionStart;
    editor.setRangeText('  ', start, editor.selectionEnd, 'end');
  }
});
editor.addEventListener('input', () => {
  saveEditor();
  syncShellVariables();
});

loadLesson(1);
