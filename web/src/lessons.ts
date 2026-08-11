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
    keywordExample: { label: 'Declared name', code: 'attack_power', summary: 'This is your stat declaration—not a built-in keyword.', lookup: 'stat name' },
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
    keywordExample: { label: 'Config vocabulary', code: '"fold": "sum"', summary: 'A fold rule is explicit config that controls how a bucket combines gear bonuses.', lookup: 'fold' },
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
    keywordExample: { label: 'Config gate', code: '"event": "crit"', summary: 'This declared tag turns the amulet contribution on only in the crit branch.', lookup: 'contribution.event' },
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
    keywordExample: { label: 'Built-in function', code: 'max(0, 1 - enemy_armor / 100)', summary: 'max prevents armor from turning damage negative; its inputs still use declared config names.', lookup: 'max(a, b)' },
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
    keywordExample: { label: 'Comparison operator', code: 'stamina >= 40', summary: 'stamina is your declared resource; >= is an expression-language operator.', lookup: '> < >= <= == !=' },
    command: `rtce simulate ${SIM_ARGS}`,
    config: { gamedef: game05, build: build05, scenario: scenario05, simdef: sim05, rotation: rotation05 },
  },
  {
    number: 6,
    mode: 'simulation',
    eyebrow: 'Computed uptime',
    title: 'Apply a timed focus window',
    summary: 'Use a damage cooldown and measure how much of the fight it really covers.',
    gamerSummary: 'Now the rotation presses a Focus-style cooldown that creates a temporary damage window. The simulator tracks when the buff turns on and off, which attacks land inside it, and its real uptime over the dummy fight. This is why simulated DPS can differ from simply checking “buff active” on a stat sheet.',
    insight: 'Buff uptime by seconds can differ from the share of attacks that actually land during the buff window.',
    keywordExample: { label: 'Sim context', code: 'not(buff.focus_window)', summary: 'buff.<name> is engine-supplied sim context; not turns its 0/1 value into the opposite gate.', lookup: 'buff.<buff>' },
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
    keywordExample: { label: 'Sim context', code: 'time < duration / 2', summary: 'time and duration are engine-supplied only while a simulation is running.', lookup: 'duration' },
    command: `rtce simulate ${SIM_ARGS} --mode monte-carlo --iterations 1000 --seed 7`,
    config: { gamedef: game07, build: build07, scenario: scenario07, simdef: sim07, rotation: rotation07 },
  },
];
