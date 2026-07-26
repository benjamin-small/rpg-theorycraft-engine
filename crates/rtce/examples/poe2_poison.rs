//! PoE2 slice 2 of 3 — **poison**: an unbounded stack of INDEPENDENT
//! ailment instances, each ticking the rate it SNAPSHOTTED at its own
//! application, applied by the attacking skill's own `apply_buff` so that
//! every instance inherits the magnitude of the hit that applied it.
//!
//! Run: `cargo run -p rtce --example poe2_poison`
//! Siblings: `poe2_charges` (stacking buffs), `poe2_triggers` (procs).
//!
//! **Scope, honestly** — as in `poe2_charges`: the `GameDef` is
//! `tests/fixtures/poe2/gamedef.json`, a PoE2-*shaped* demonstration
//! slice, not Path of Exile 2's damage model and not derived from game
//! data. Every coefficient here is `representative`, picked so the
//! arithmetic hand-derives. The real thing is `../poe2-calcs`' GENERATED
//! `gamedef/poe2.gamedef.json` (209 pipeline stages; standing reference
//! 124.53 dps for a default Monk build).
//!
//! **The one thing to take away.** An action-applied snapshot captures
//! under the CASTING ACTION'S `damage.stats` overlay; a proc-applied one
//! captures the AMBIENT build. Writing this poison as a proc — which is
//! the only way rtce 0.2.0 could have written it — silently halves it
//! here, and the run at the bottom of this file proves that rather than
//! asserting it.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile as plan_compile;
use rtce::scenario::Scenario;
use rtce::sim::{compile as sim_compile, run, Mode, SimReport};
use rtce::simdef::{Rotation, SimDef};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn main() {
    // ── Tier 1: the algorithm, from the committed fixture ─────────────
    let gamedef_json = include_str!("../tests/fixtures/poe2/gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── Tier 2: a chaos-scaling character. `coeff_pct` is 100 HERE and
    //    200 on the attacking action's overlay — deliberately different,
    //    because that gap is what makes "which build did the snapshot
    //    read?" an observable question rather than a claim. ────────────
    let build: BuildState = serde_json::from_str(
        r#"{
          "stats": {
            "weapon_avg": 100.0, "coeff_pct": 100.0,
            "crit_chance": 25.0, "crit_bonus_pct": 200.0,
            "poison_coeff_pct": 25.0, "pen_chaos": 10.0
          },
          "contributions": [
            { "bucket": "inc_phys",   "value":  50.0 },
            { "bucket": "inc_chaos",  "value": 100.0 },
            { "bucket": "more_chaos", "value":  25.0 }
          ]
        }"#,
    )
    .expect("valid build");

    // ── Tier 3: SEQUENCING. One skill, one ailment, no resources and no
    //    procs.
    //
    //    `poison` is the mechanic:
    //      `max_stacks: 0` — UNBOUNDED. Nothing but expiry ever trims the
    //         stack, so no instance is ever evicted here.
    //      `on_reapply: add_independent` — each application starts its own
    //         window with its own expiry. The opposite of `poe2_charges`'
    //         shared clock: these fall off one at a time, oldest first.
    //      `tick_objective: { objective: "poison_dps", snapshot: true }` —
    //         each instance CAPTURES `poison_dps` at its own application
    //         and ticks that value unchanged to expiry. The buff's total
    //         rate is the SUM over live instances, so the stack count is
    //         already inherent in it and is never multiplied in again.
    //      no `contributions` — deliberately. A poison that fed a bucket
    //         its own tick objective reads would self-amplify one
    //         application behind (documented on `TickObjective`), and this
    //         slice is about the OVERLAY, so every capture here is the
    //         same number and the arithmetic stays one multiplication.
    //
    //    `apply_buff` on the ACTION is the load-bearing choice — see the
    //    contrast run at the bottom.
    //
    //    The 4.5s duration is `representative` and half-integer on
    //    purpose: it keeps every expiry instant strictly between cast
    //    completions, so no pin has to lean on a same-instant tie-break.
    let simdef_json = r#"{
      "actions": {
        "viper_strike": {
          "cast_time": "1", "cooldown": 0.0,
          "damage": { "stats": { "coeff_pct": 200.0 } },
          "apply_buff": ["poison"]
        }
      },
      "buffs": {
        "poison": {
          "duration": 4.5,
          "max_stacks": 0,
          "on_reapply": "add_independent",
          "tick_objective": { "objective": "poison_dps", "snapshot": true }
        }
      },
      "damage_objective": "hit"
    }"#;
    let simdef: SimDef = serde_json::from_str(simdef_json).expect("valid simdef");

    let rotation: Rotation =
        serde_json::from_str(r#"{ "rules": [ { "action": "viper_strike" } ] }"#)
            .expect("valid rotation");
    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── Scenario: a 20s dummy, 20% physical and 30% chaos resistance.
    let dummy: Scenario = serde_json::from_str(
        r#"{ "phases": [ { "name": "dummy", "weight": 20,
              "stats": { "enemy_res_phys": 20.0, "enemy_res_chaos": 30.0 } } ] }"#,
    )
    .expect("valid scenario");

    let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");
    let hit_damage = report.actions["viper_strike"].damage;
    let dot_damage = report.total.total_damage - hit_damage;

    println!("PoE2 poison (P7e slice 2) — 20s dummy, EV mode");
    println!(
        "  viper_strike: {} casts, {:.4} hit damage",
        report.actions["viper_strike"].casts, hit_damage
    );
    println!(
        "  poison: uptime {:.4}, avg_stacks {:.4}, {:.4} DoT damage",
        report.buffs["poison"].uptime, report.buffs["poison"].avg_stacks, dot_damage
    );
    println!(
        "  total: {:.4} damage over {:.0}s = {:.4} dps",
        report.total.total_damage, report.total.duration, report.total.dps
    );

    // ══════════════════ Hand-worked pins ══════════════════════════════
    //
    // ── The two magnitudes ────────────────────────────────────────────
    // Constant factors first (`representative`, picked to cancel):
    //   crit_mult  = 1 + 25/100 × (200/100 − 1)      = 1.25
    //   mit_phys   = max(0, 1 − 20/100)              = 0.80   (1.25×0.80 = 1)
    //   mit_chaos  = max(0, 1 − (30 − 10)/100)       = 0.80
    //   shock_mult = 1 (no shock in this slice)
    //   chaos chain = (1 + 100/100) × more_chaos × mit_chaos
    //               = 2.00 × 1.25 × 0.80             = 2.00, exactly
    //
    //   phys_scaled = weapon_avg × coeff_pct/100 × (1 + 50/100) × more_global
    //               = 100 × coeff_pct/100 × 1.5 × 1 = coeff_pct × 1.5
    //   hit         = phys_scaled × 1.25 × 1 × 0.80 = phys_scaled
    //   poison_dps  = phys_scaled × 25/100 × 2.00   = phys_scaled × 0.5
    //
    // Two different `coeff_pct` are in play, and that is the whole point:
    //   OVERLAY  (the action's `damage.stats`, coeff 200):
    //     phys_scaled = 300 → hit = 300 per cast, poison rate R = 150/s
    //   AMBIENT  (the plain build, coeff 100):
    //     phys_scaled = 150 →                     rate R' =  75/s
    // `apply_buff` on the action captures under the overlay, so R = 150.
    // A proc would have captured R' = 75 — pinned in the contrast below.
    //
    // ── The cadence ───────────────────────────────────────────────────
    // One 1s skill, no cost, no cooldown: decisions at t = 0…19,
    // completions at t = 1…20, one poison application per completion
    // (applied AFTER the hit is credited — `sim`'s cast-complete order).
    // 20 hits × 300 = 6000 hit damage.
    //
    // Each instance is live on [a, a + 4.5), clipped at the 20s end:
    //   a = 1…15   15 instances × 4.5s  = 67.5
    //   a = 16     [16, 20.5) → clipped =  4
    //   a = 17, 18, 19             3 + 2 + 1 =  6
    //   a = 20     lands exactly at the end, integrates to 0
    //   ∫ stacks dt = 67.5 + 4 + 6 = 77.5
    //   avg_stacks  = 77.5 / 20 = 3.875
    //
    // The STEADY-STATE count is Little's law, λ × W = 1/s × 4.5s = 4.5,
    // and the instantaneous count is an integer oscillating around it: on
    // [t, t+0.5) five instances are live (applied at t−4 … t), and the
    // t−4 one expires at t+0.5 leaving four until the next completion.
    // Half a second at 5 and half at 4 averages to exactly 4.5. The 3.875
    // above is that steady state diluted by the [0,1) ramp and the
    // truncated tail — which is why BOTH are stated: 4.5 is the mechanic,
    // 3.875 is this 20-second fight.
    //
    // ── The numbers ───────────────────────────────────────────────────
    // A snapshot buff's total tick rate is Σ(instance rates), and the
    // stack count is already inherent in that sum, so the DoT is
    //   ∫ stacks dt × R = 77.5 × 150 = 11625
    // NOT 77.5 × 150 × (some stack count) again. If the summed rate were
    // ALSO multiplied by the live count, this would integrate R × ∫stacks²
    // instead — a much larger number, and the pin says which.
    //   hit   = 20 × 300              =  6000
    //   DoT   = 77.5 × 150            = 11625
    //   total                         = 17625
    //   dps   = 17625 / 20            =   881.25
    // `uptime` is "at least one instance live", which is a much blunter
    // reading of the same timeline: the first instance lands at t=1 and
    // the one applied at t=19 runs to 23.5, so the stack is continuously
    // non-empty on [1,20] — uptime 19/20 = 0.95, and it would read the
    // same for a one-instance buff. `avg_stacks` is the counted number.
    assert_eq!(report.actions["viper_strike"].casts, 20);
    assert!(close(hit_damage, 6000.0), "hit damage: got {hit_damage}");
    assert!(
        close(report.buffs["poison"].avg_stacks, 3.875),
        "avg_stacks: got {} — want ∫77.5 / 20s",
        report.buffs["poison"].avg_stacks
    );
    assert!(
        close(dot_damage, 11625.0),
        "DoT: got {dot_damage} — want 77.5 instance-seconds × R=150"
    );
    assert!(
        close(report.total.total_damage, 17625.0),
        "got {}",
        report.total.total_damage
    );
    assert!(close(report.total.dps, 881.25), "got {}", report.total.dps);
    assert!(
        close(report.buffs["poison"].uptime, 0.95),
        "uptime: got {} — want 19/20",
        report.buffs["poison"].uptime
    );
    println!("\n  EV pins hold: 6000 hit + 11625 DoT = 17625 / 881.25 dps / 3.875 stacks ✓");

    // ── Monte Carlo, and why EXACT equality is the right gate ─────────
    //
    // The P7 design spec makes EV/MC agreement a hard gate for snapshot
    // DoT totals and steady-state stack counts. Here they agree to the
    // bit, and that is a stronger statement than a tolerance band, for
    // three separate reasons worth keeping straight:
    //   1. this gamedef has no `events` block, so `evaluate_phase_sampled`
    //      has nothing to branch on and per-cast damage cannot vary;
    //   2. `apply_buff` is deterministic — no roll, no ICD, no accumulator
    //      — so the instance TRAJECTORY is identical in both modes;
    //   3. a snapshot capture is EV-blended in BOTH modes by design (see
    //      `sim::exec`'s `eval_objective`), so what each instance captures
    //      never depends on the RNG.
    // The mutation this catches is any RNG draw leaking onto the buff
    // application path, which would break same-seed determinism here
    // before it broke anything else.
    let mc = run(
        &plan,
        &sim_plan,
        &build,
        &dummy,
        Mode::MonteCarlo {
            iterations: 128,
            seed: 5,
        },
    )
    .expect("mc sim runs");
    let dist = mc.distribution.expect("MC mode reports a distribution");
    println!(
        "\nMonte Carlo (N=128, seed=5): mean {:.4}  std {:.4}",
        dist.mean, dist.std
    );
    assert!(close(dist.mean, 881.25), "MC mean {}", dist.mean);
    assert!(close(dist.std, 0.0), "MC std {}", dist.std);
    assert!(
        close(mc.total.total_damage, report.total.total_damage),
        "MC total {} vs EV {}",
        mc.total.total_damage,
        report.total.total_damage
    );
    assert!(
        close(mc.buffs["poison"].avg_stacks, 3.875),
        "MC avg_stacks {}",
        mc.buffs["poison"].avg_stacks
    );
    println!("  MC reproduces EV exactly (std 0) ✓");

    // ══════════════ Contrast: the overlay rule, run ═══════════════════
    //
    // Same gamedef, same build, same scenario, same cadence, same poison
    // buff — one difference: the poison is applied by a PROC on the
    // skill's cast instead of by the skill's own `apply_buff`.
    //
    // The proc fires at exactly the same instants (a proc rolls at cast
    // complete, `chance: "1"` with no ICD means every cast), so the
    // instance trajectory is IDENTICAL — ∫ stacks dt is still 77.5 and
    // `avg_stacks` is still 3.875. Only the captured RATE moves: a
    // proc-applied snapshot reads the AMBIENT effective build, which has
    // the build's own `coeff_pct` of 100 rather than the action's overlay
    // of 200, so R' = 75/s instead of 150/s.
    //   hit   = 20 × 300     = 6000    (unchanged — same action, same overlay)
    //   DoT   = 77.5 × 75    = 5812.5  (exactly half)
    //   total               = 11812.5
    //   dps                 =   590.625
    //
    // This is the config-author trap the P7d design note names: the proc
    // spelling looks equivalent and is silently worth half. It is also why
    // rtce 0.2.0 could not express a PoE2 ailment at all — `apply_buff` on
    // an action is what makes the ailment inherit its applying hit.
    let proc_simdef: SimDef = serde_json::from_str(
        r#"{
          "actions": {
            "viper_strike": {
              "cast_time": "1", "cooldown": 0.0,
              "damage": { "stats": { "coeff_pct": 200.0 } }
            }
          },
          "buffs": {
            "poison": {
              "duration": 4.5,
              "max_stacks": 0,
              "on_reapply": "add_independent",
              "tick_objective": { "objective": "poison_dps", "snapshot": true }
            }
          },
          "procs": {
            "envenom": { "trigger": "on_cast", "chance": "1", "icd": 0.0,
                         "apply_buff": "poison" }
          },
          "damage_objective": "hit"
        }"#,
    )
    .expect("valid simdef");
    let proc_plan = sim_compile(&plan, &proc_simdef, &rotation).expect("simdef compiles");
    let via_proc = run(&plan, &proc_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");
    let proc_dot = via_proc.total.total_damage - via_proc.actions["viper_strike"].damage;

    println!(
        "\n  applied by a PROC instead: {:.4} DoT ({:.4} total, {:.4} dps), \
         avg_stacks {:.4}",
        proc_dot,
        via_proc.total.total_damage,
        via_proc.total.dps,
        via_proc.buffs["poison"].avg_stacks
    );
    assert!(
        close(via_proc.buffs["poison"].avg_stacks, 3.875),
        "the trajectory must be identical: got {}",
        via_proc.buffs["poison"].avg_stacks
    );
    assert!(
        close(via_proc.actions["viper_strike"].damage, 6000.0),
        "the hit must be identical: got {}",
        via_proc.actions["viper_strike"].damage
    );
    assert!(
        close(proc_dot, 5812.5),
        "proc DoT: got {proc_dot} — want 77.5 × R'=75, exactly half the \
         action-applied 11625"
    );
    assert!(
        close(proc_dot * 2.0, dot_damage),
        "the proc path must be exactly half: {proc_dot} vs {dot_damage}"
    );
    assert!(
        close(via_proc.total.dps, 590.625),
        "got {}",
        via_proc.total.dps
    );
    println!("  contrast pins hold: 5812.5 DoT — exactly half the action-applied 11625 ✓");

    // A last honest note, asserted rather than asserted-in-prose: the two
    // runs differ ONLY in the DoT. Same casts, same hit damage, same
    // stacks — so the 2× really is attributable to the capture and to
    // nothing else about the two configs.
    fn shape(r: &SimReport) -> (u64, f64, f64) {
        (
            r.actions["viper_strike"].casts,
            r.actions["viper_strike"].damage,
            r.buffs["poison"].avg_stacks,
        )
    }
    assert_eq!(shape(&report).0, shape(&via_proc).0);
    assert!(close(shape(&report).1, shape(&via_proc).1));
    assert!(close(shape(&report).2, shape(&via_proc).2));
}
