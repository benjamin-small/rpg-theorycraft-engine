import game01 from '../../crates/rtce/tests/fixtures/guide/01-gamedef.json?raw';
import build01 from '../../crates/rtce/tests/fixtures/guide/01-build.json?raw';
import scenario01 from '../../crates/rtce/tests/fixtures/guide/01-scenario.json?raw';
import game02 from '../../crates/rtce/tests/fixtures/guide/02-gamedef.json?raw';
import build02 from '../../crates/rtce/tests/fixtures/guide/02-build.json?raw';
import scenario02 from '../../crates/rtce/tests/fixtures/guide/02-scenario.json?raw';
import game03 from '../../crates/rtce/tests/fixtures/guide/03-gamedef.json?raw';
import build03 from '../../crates/rtce/tests/fixtures/guide/03-build.json?raw';
import scenario03 from '../../crates/rtce/tests/fixtures/guide/03-scenario.json?raw';
import game04 from '../../crates/rtce/tests/fixtures/guide/04-gamedef.json?raw';
import build04 from '../../crates/rtce/tests/fixtures/guide/04-build.json?raw';
import scenario04 from '../../crates/rtce/tests/fixtures/guide/04-scenario-mixed.json?raw';
import game05 from '../../crates/rtce/tests/fixtures/guide/05-gamedef.json?raw';
import build05 from '../../crates/rtce/tests/fixtures/guide/05-build.json?raw';
import scenario05 from '../../crates/rtce/tests/fixtures/guide/05-scenario.json?raw';
import sim05 from '../../crates/rtce/tests/fixtures/guide/05-simdef.json?raw';
import rotation05 from '../../crates/rtce/tests/fixtures/guide/05-rotation.json?raw';
import game06 from '../../crates/rtce/tests/fixtures/guide/06-gamedef.json?raw';
import build06 from '../../crates/rtce/tests/fixtures/guide/06-build.json?raw';
import scenario06 from '../../crates/rtce/tests/fixtures/guide/06-scenario.json?raw';
import sim06 from '../../crates/rtce/tests/fixtures/guide/06-simdef.json?raw';
import rotation06 from '../../crates/rtce/tests/fixtures/guide/06-rotation.json?raw';
import game07 from '../../crates/rtce/tests/fixtures/guide/07-gamedef.json?raw';
import build07 from '../../crates/rtce/tests/fixtures/guide/07-build.json?raw';
import scenario07 from '../../crates/rtce/tests/fixtures/guide/07-scenario.json?raw';
import sim07 from '../../crates/rtce/tests/fixtures/guide/07-simdef.json?raw';
import rotation07 from '../../crates/rtce/tests/fixtures/guide/07-rotation.json?raw';
import type { ConfigSet } from './rtce';

export interface Lesson {
  number: number;
  mode: 'sheet' | 'simulation';
  eyebrow: string;
  title: string;
  summary: string;
  gamerSummary: string;
  insight: string;
  keywordExample: {
    label: string;
    code: string;
    summary: string;
    lookup: string;
    source: keyof ConfigSet;
    sourceNeedle: string;
  };
  command: string;
  config: ConfigSet;
}

const CALC_ARGS = '--game $game --build $build --scenario $scenario';
const SIM_ARGS = `${CALC_ARGS} --sim $sim --rotation $rotation`;

export const lessons: Lesson[] = [
  {
    number: 1,
    mode: 'sheet',
    eyebrow: 'The first number',
    title: 'Turn one stat into one answer',
    summary: 'Start with Attack Power and turn it into the damage shown for one hit.',
    gamerSummary: 'Think of this like opening your character sheet. Your build has 120 Attack Power, so the game calculates a 120-damage hit. The engine calls that requested answer an objective. It is instant tooltip math—not an attack being cast, and no combat clock is running.',
    insight: 'The GameDef holds the game’s formulas, the Build holds this character’s stats, and the Scenario describes the target.',
    keywordExample: {
      label: 'Rule → result',
      code: '"expr": "attack_power"',
      summary: 'The engine executes this entire hit formula: read Attack Power from Build—currently 120—and store it as hit. Because hit is an objective, 120 becomes the result without casting an attack or starting a clock.',
      lookup: 'stat name',
      source: 'gamedef',
      sourceNeedle: '"expr": "attack_power"',
    },
    command: `rtce evaluate ${CALC_ARGS}`,
    config: { gamedef: game01, build: build01, scenario: scenario01 },
  },
  {
    number: 2,
    mode: 'sheet',
    eyebrow: 'Modifier grammar',
    title: 'Fold bonuses into buckets',
    summary: 'Combine bonuses from gear, passives, and buffs into one sheet number.',
    gamerSummary: 'Open the Build tab and read it like a loadout: the Stormstring Bow gives +30% Damage and the Trailseeker Gloves give +25% Damage. Both affixes land in the same additive bucket, so your sheet combines them into +55% before calculating the hit. The `_source` labels are player-facing notes, not hidden math. There is still no rotation, resource spending, or cooldown timing.',
    insight: 'Each gear affix contributes a value to a named modifier bucket. The bucket decides whether those bonuses add together or multiply separately.',
    keywordExample: {
      label: 'Bucket math in action',
      code: '"additive": { "fold": "sum" }',
      summary: 'This stacking rule makes the engine add the bow’s +30 and gloves’ +25 before the hit formula reads the bucket: 120 × (1 + 55 / 100) = 186. Gear supplies bonuses; GameDef decides how they stack.',
      lookup: 'fold',
      source: 'gamedef',
      sourceNeedle: '"additive": {\n      "fold": "sum"',
    },
    command: `rtce evaluate ${CALC_ARGS}`,
    config: { gamedef: game02, build: build02, scenario: scenario02 },
  },
  {
    number: 3,
    mode: 'sheet',
    eyebrow: 'Probability as data',
    title: 'Branch for critical hits',
    summary: 'Fold crit chance and crit damage into an average tooltip hit.',
    gamerSummary: 'A character sheet usually shows expected damage, not whether your next arrow will crit. Here the Eagle Eye Amulet contributes +50% to the declared crit_damage bucket only when the declared crit event fires. That bucket is ×1.0 for the normal branch and ×1.5 for the crit branch. The engine weights those 186- and 279-damage outcomes by 70% and 30% to produce one 213.9 average sheet hit.',
    insight: 'This formula uses only config-declared names: crit_damage is a bucket, and the amulet contribution is tagged with the crit event. The engine refolds that bucket once per branch; the lexicon labels event_multiplier separately as an optional engine shortcut.',
    keywordExample: {
      label: 'Branch engine',
      code: '"branched": true',
      summary: 'This makes the engine evaluate hit once normally and once with crit-tagged gear, then probability-weight the 186 and 279 damage branches into the 213.9 sheet average.',
      lookup: 'branched',
      source: 'gamedef',
      sourceNeedle: '"branched": true',
    },
    command: `rtce explain ${CALC_ARGS}`,
    config: { gamedef: game03, build: build03, scenario: scenario03 },
  },
  {
    number: 4,
    mode: 'sheet',
    eyebrow: 'One build, many fights',
    title: 'Blend conditions and phases',
    summary: 'Estimate one build across changing boss phases and defenses.',
    gamerSummary: 'Imagine a boss that changes armor or gives you only partial uptime on a damage bonus. The sheet calculator prices the build in each phase, weights those answers by how long the phase lasts, and returns one average objective for the fight. It still does not choose skills or play the encounter second by second.',
    insight: 'The Scenario changes the fight being measured without rewriting the game formulas or the character build.',
    keywordExample: {
      label: 'Phase weighting',
      code: '"uptimes": { "focused": 0.2 }',
      summary: 'This is assumed sheet uptime, not a scheduled buff: the engine prices Focused Fury at 20% strength in the three-part average phase and 100% in the one-part armor-break phase, then blends both armor and uptime 75/25.',
      lookup: 'condition name',
      source: 'scenario',
      sourceNeedle: '"focused": 0.2',
    },
    command: `rtce evaluate ${CALC_ARGS}`,
    config: { gamedef: game04, build: build04, scenario: scenario04 },
  },
  {
    number: 5,
    mode: 'simulation',
    eyebrow: 'Enter the timeline',
    title: 'Give the character a rotation',
    summary: 'Put the build on a training dummy and play its rotation for sixty seconds.',
    gamerSummary: 'This is the first actual combat simulation. The clock starts, the character follows a priority list, spends and regenerates stamina, and uses whichever skill is available. The damage meter adds every hit, then divides total damage by 60 seconds to report simulated DPS. Timing and resource starvation can now change the answer.',
    insight: 'The stat-sheet formulas still calculate each hit, but the simulator decides when those hits can happen.',
    keywordExample: {
      label: 'Priority fallback',
      code: '"action": "power_shot", "when": "stamina >= 40"',
      summary: 'At every decision the engine reads top-down: spend 40 stamina on Power Shot when this passes; otherwise fall through to Quick Shot and regain 40. That exact loop produces 31 Power Shots and 29 Quick Shots in 60 seconds.',
      lookup: 'resource name',
      source: 'rotation',
      sourceNeedle: '"action": "power_shot",\n      "when": "stamina >= 40"',
    },
    command: `rtce simulate ${SIM_ARGS}`,
    config: { gamedef: game05, build: build05, scenario: scenario05, simdef: sim05, rotation: rotation05 },
  },
  {
    number: 6,
    mode: 'simulation',
    eyebrow: 'Computed uptime',
    title: 'Apply a timed focus window',
    summary: 'Use a damage cooldown and measure how much of the fight it really covers.',
    gamerSummary: 'Open SimDef and follow the names: focus_fire applies the focus_window buff, which lasts 2.5 seconds and sets focused to 1 while active. Open Build and Focused Fury uses that focused condition to turn on its bonus. The simulator tracks the resulting uptime and which attacks actually land inside the damage window.',
    insight: 'A buff’s conditions map is the bridge from timeline state to sheet damage: focus_window turns on focused, and focused unlocks the gated gear bonus.',
    keywordExample: {
      label: 'Timeline → damage',
      code: '"conditions": { "focused": 1 }',
      summary: 'When focus_window is live, the engine sets focused to 1 and unlocks Focused Fury’s gated +50% crit damage. Six 2.5-second windows equal 25% time uptime, but only 12 of 60 attacks land inside—20% of hits experience the buff.',
      lookup: 'condition name',
      source: 'simdef',
      sourceNeedle: '"conditions": {\n        "focused": 1',
    },
    command: `rtce simulate ${SIM_ARGS}`,
    config: { gamedef: game06, build: build06, scenario: scenario06, simdef: sim06, rotation: rotation06 },
  },
  {
    number: 7,
    mode: 'simulation',
    eyebrow: 'See the distribution',
    title: 'Sample a thousand fights',
    summary: 'Run the dummy fight a thousand times to see good and bad RNG.',
    gamerSummary: 'Instead of averaging crit chance into every hit, this mode rolls each fight separately—just like playing the same pull again and again. The result shows low-roll, typical, and high-roll DPS, so you can see how much crit luck moves the damage meter. The seed makes the same thousand simulated pulls reproducible.',
    insight: 'The rotation stays the same here; individual crit rolls create the DPS spread around the sheet average.',
    keywordExample: {
      label: 'Sheet chance → real RNG',
      code: '"chance": "crit_chance / 100"',
      summary: 'Build supplies 30 Crit Chance, so this evaluates to 0.30. Expected mode blends that chance into every hit; Monte Carlo makes a yes/no roll for each of the same 60 hits across 1,000 fights. Only crit luck varies, and seed 7 makes it repeatable.',
      lookup: 'events.<name>.chance',
      source: 'gamedef',
      sourceNeedle: '"chance": "crit_chance / 100"',
    },
    command: `rtce simulate ${SIM_ARGS} --mode monte-carlo --iterations 1000 --seed 7`,
    config: { gamedef: game07, build: build07, scenario: scenario07, simdef: sim07, rotation: rotation07 },
  },
];
