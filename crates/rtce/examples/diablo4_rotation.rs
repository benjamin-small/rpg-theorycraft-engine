//! Sequencing (P6): the SAME committed Diablo 4 gamedef used by
//! `diablo4_basics`, this time driven by a [`sim::run`] timeline instead
//! of a single `Plan::evaluate` average — Level-1's asserted uptimes
//! become Level-2's COMPUTED ones.
//!
//! **Scope, honestly:** the `SimDef`/`Rotation` below is a DEMONSTRATION
//! SLICE, not Diablo 4's real cadence data — regen is zeroed and
//! Firebolt's mana gain is set equal to Fireball's cost purely so the
//! resulting cast sequence is a clean, hand-verifiable alternation (see
//! the pin comments below). A production rotation would tune these from
//! real skill data the way `diablo4_basics`'s `GameDef` slice was
//! transcribed from `diablo4-calc`.
//!
//! Run: `cargo run -p rtce --example diablo4_rotation`
//!
//! Tiers on display (see `diablo4_basics.rs` for the Level-1 walkthrough):
//!   GameDef  (tests/fixtures/d4/gamedef.json) — unchanged, the ALGORITHM
//!   BuildState (inline below)                 — one candidate character
//!   SimDef + Rotation (inline below)           — the SEQUENCING config
//!   Scenario (inline below)                    — a 60s training dummy

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile as plan_compile;
use rtce::scenario::Scenario;
use rtce::sim::{compile as sim_compile, run, Mode};
use rtce::simdef::{Rotation, SimDef};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn main() {
    // ── Tier 1: the game's algorithm, straight from the committed fixture ──
    let gamedef_json = include_str!("../tests/fixtures/d4/gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── Tier 2: a basic Sorcerer — same shape as diablo4_basics's build,
    //    minus its own coeff_pct/hits_per_use (every damaging action below
    //    overrides both per cast via `damage.stats`) ────────────────────
    let build: BuildState = serde_json::from_str(
        r#"{
          "stats": {
            "weapon_avg": 1000.0, "mainstat": 800.0, "mainstat_divisor": 800.0,
            "crit_chance": 20.0, "op_chance": 0.0, "op_baseline": 1.5,
            "base_aps": 1.0, "enemy_dr": 0.0, "dot_coeff_pct": 0.0
          },
          "contributions": [
            { "bucket": "additive",   "value": 30.0 },
            { "bucket": "additive",   "value": 25.0, "event": "crit" },
            { "bucket": "crit_group", "value": 20.0 },
            { "bucket": "vuln_group", "value": 20.0 },
            { "bucket": "indep",      "value": 15.0 }
          ]
        }"#,
    )
    .expect("valid build");

    // ── Tier 3: SEQUENCING — mana (max 100, NO passive regen — see the
    //    module doc's "Scope, honestly" note), a Fireball spender (40
    //    mana, 1s cast, coeff 200%), a Firebolt generator (free, +40 mana,
    //    1s cast, coeff 40%), and Frost Nova (instant, 10s cooldown). A
    //    buff isn't applied by an action directly — only a PROC can (see
    //    `simdef::ProcDef`) — so `nova_pulse` (on_cast, chance 1, icd 10)
    //    is the mechanism: since Frost Nova has top rotation priority and
    //    is always cast the instant it's off cooldown, and its icd (10)
    //    matches Frost Nova's own cooldown, `nova_pulse` always rolls on
    //    Frost Nova's own on-cast event (any Fireball/Firebolt on-cast
    //    events immediately after find it still in ICD) — the same
    //    "icd equals the gating action's cooldown" trick `sim::exec`'s own
    //    `computed_buff_uptime_is_hand_worked` test pins. The buff drives
    //    `vulnerable` to 1.0 while up; the scenario below sets NO static
    //    vulnerable uptime at all, so 100% of `vulnerable`'s value here is
    //    COMPUTED) ─────────────────────────────────────────────────────
    let simdef_json = r#"{
      "resources": {
        "mana": { "max": "100", "regen_per_sec": "0" }
      },
      "actions": {
        "frost_nova": {
          "cast_time": "0", "cooldown": 10.0,
          "cost": {}, "gain": {}
        },
        "fireball": {
          "cast_time": "1", "cooldown": 0.0,
          "cost": { "mana": 40.0 }, "gain": {},
          "damage": { "stats": { "coeff_pct": 200.0 } }
        },
        "firebolt": {
          "cast_time": "1", "cooldown": 0.0,
          "cost": {}, "gain": { "mana": 40.0 },
          "damage": { "stats": { "coeff_pct": 40.0 } }
        }
      },
      "buffs": {
        "vuln_window": { "duration": 4.0, "conditions": { "vulnerable": 1.0 } }
      },
      "procs": {
        "nova_pulse": {
          "trigger": "on_cast", "chance": "1", "icd": 10.0,
          "apply_buff": "vuln_window"
        }
      },
      "damage_objective": "hit_after_dr"
    }"#;
    let simdef: SimDef = serde_json::from_str(simdef_json).expect("valid simdef");

    // Priority rotation: Frost Nova whenever it's off cooldown (the hard
    // gate alone means "when ready" — no `when` needed), else Fireball if
    // affordable, else Firebolt (always willing) as the filler/generator.
    let rotation_json = r#"{ "rules": [
      { "action": "frost_nova" },
      { "action": "fireball", "when": "mana >= 40" },
      { "action": "firebolt" }
    ]}"#;
    let rotation: Rotation = serde_json::from_str(rotation_json).expect("valid rotation");

    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── Scenario: 60s training dummy, 25% DR, NO static vulnerable
    //    uptime — Frost Nova's buff is the only source of `vulnerable`.
    let dummy: Scenario = serde_json::from_str(
        r#"{ "phases": [ { "name": "dummy", "weight": 60,
              "stats": { "enemy_dr": 25.0 } } ] }"#,
    )
    .expect("valid scenario");

    // ══════════════════════════ EV mode ═══════════════════════════════
    let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!("Diablo 4 rotation (P6 sequencing) — 60s training dummy, EV mode");
    println!(
        "  {:<12} {:>6} {:>14} {:>8}",
        "action", "casts", "damage", "share"
    );
    for (name, a) in &report.actions {
        println!(
            "  {name:<12} {:>6} {:>14.4} {:>7.2}%",
            a.casts,
            a.damage,
            a.share * 100.0
        );
    }
    println!(
        "  total: {:.4} damage over {:.0}s = {:.5} dps",
        report.total.total_damage, report.total.duration, report.total.dps
    );
    println!(
        "  vuln_window buff uptime: {:.4}   vulnerable condition uptime: {:.4}",
        report.buff_uptime["vuln_window"], report.condition_uptime["vulnerable"]
    );
    println!(
        "  mana: {:.4}s starved, {:.4}s capped",
        report.resources["mana"].time_starved, report.resources["mana"].time_capped
    );

    // ── Hand-worked pins (house rule: every example carries its number) ─
    //
    // Per-hit `hit_after_dr` values (mainstat_mult = 1+800/800 = 2; DR
    // factor = 1-25/100 = 0.75; crit_chance 20% blends nocrit/crit 0.8/0.2;
    // event_factors: nocrit=1, crit=1.5×(1+20/100)=1.8; additive: nocrit
    // 1+30/100=1.30, crit 1+(30+25)/100=1.55; indep=1+15/100=1.15; vuln_factor
    // inactive=1, active=1+1×(1.2×(1+20/100)−1)=1.44 — same algebra as
    // diablo4_basics's dummy pin):
    //   fireball base = 1000×200/100×2 = 4000
    //     inactive: nocrit 4000×1.30×1.15=5980, crit 4000×1.55×1.8×1.15=12834
    //       EV=0.8×5980+0.2×12834=7350.8 ; ×0.75dr = 5513.1
    //     active:   nocrit 5980×1.44=8611.2, crit 12834×1.44=18480.96
    //       EV=0.8×8611.2+0.2×18480.96=10585.152 ; ×0.75dr = 7938.864
    //   firebolt base = 1000×40/100×2 = 800 = 4000×0.2 — every other
    //     factor above is identical (same crit/additive/indep/vuln/dr), so
    //     firebolt's values are exactly 0.2× fireball's:
    //       inactive = 1102.62 ; active = 1587.7728
    //
    // Cadence (mana has NO regen; fireball costs 40, firebolt gains 40 —
    // chosen equal so the cadence resolves to a clean alternation instead
    // of the "gnarly" non-periodic mess continuous regen would produce):
    // decisions land on every integer second (fireball/firebolt both cast
    // in exactly 1s, frost_nova is instant and never consumes a slot), so
    //   t=0: mana=100 -> fireball (100-40=60)
    //   t=1: mana=60  -> fireball (60-40=20)
    //   t=2: mana=20<40 -> firebolt (20+40=60)
    //   t=3: mana=60  -> fireball (60-40=20)
    //   t=4: mana=20<40 -> firebolt (20+40=60)  ... and so on: from t=1
    // onward mana alternates 60/20, giving fireball at every ODD t plus
    // t=0 itself (two fireballs back-to-back only at the very start,
    // since mana starts full), firebolt at every EVEN t ≥ 2 — 60 slots
    // (t=0..59) split 31 fireball / 29 firebolt.
    // Frost Nova recasts the instant it's off cooldown: t=0,10,20,30,40,50
    // (6 casts), each opening a `vuln_window` [s, s+4) — buff EXPIRES
    // exactly at s+4 and that instant's completion lands INACTIVE (the
    // expiry event is ordered before a same-time cast completion, as in
    // `sim::exec`'s own pinned tests), so each window's ACTIVE completions
    // are exactly c ∈ {s+1, s+2, s+3} — 3 per window × 6 windows = 18
    // active completions out of 60 total; uptime = 6×4/60 = 0.4.
    // Reading off which action landed on each active completion (t=c-1,
    // via the alternation above): active fireballs at t=0,1,11,21,31,41,51
    // (7); active fireboats at t=2,10,12,20,22,30,32,40,42,50,52 (11) —
    // 18 total, so fireball: 7 active / 24 inactive (31 total), firebolt:
    // 11 active / 18 inactive (29 total).
    //   total_damage = 7×7938.864 + 24×5513.1 + 11×1587.7728 + 18×1102.62
    //                = 55572.048 + 132314.4 + 17465.5008 + 19847.16
    //                = 225199.1088
    //   dps = 225199.1088 / 60 = 3753.31848
    // Mana never idles below firebolt's unconditional fallback and never
    // regens above its 100 cap via the 20/60 oscillation above, so both
    // `time_starved` and `time_capped` are exactly 0.
    assert_eq!(report.actions["frost_nova"].casts, 6);
    assert_eq!(report.actions["fireball"].casts, 31);
    assert_eq!(report.actions["firebolt"].casts, 29);
    assert!(
        close(report.total.total_damage, 225199.1088),
        "got {}",
        report.total.total_damage
    );
    assert!(close(report.total.duration, 60.0));
    assert!(
        close(report.total.dps, 3753.31848),
        "got {}",
        report.total.dps
    );
    assert!(
        close(report.buff_uptime["vuln_window"], 0.4),
        "got {}",
        report.buff_uptime["vuln_window"]
    );
    assert!(
        close(report.condition_uptime["vulnerable"], 0.4),
        "got {}",
        report.condition_uptime["vulnerable"]
    );
    assert!(close(report.resources["mana"].time_starved, 0.0));
    assert!(close(report.resources["mana"].time_capped, 0.0));
    println!("\n  EV pins hold: 225199.1088 total / 3753.31848 dps / 0.4 vuln uptime ✓");

    // ══════════════════════════ MC mode ════════════════════════════════
    let mc_report = run(
        &plan,
        &sim_plan,
        &build,
        &dummy,
        Mode::MonteCarlo {
            iterations: 1000,
            seed: 42,
        },
    )
    .expect("mc sim runs");
    let dist = mc_report
        .distribution
        .expect("MC mode reports a distribution");

    println!("\nDiablo 4 rotation — 60s training dummy, Monte Carlo mode (N=1000, seed=42)");
    println!(
        "  mean {:.4}   std {:.4}   p10 {:.4}   p50 {:.4}   p90 {:.4}",
        dist.mean, dist.std, dist.p10, dist.p50, dist.p90
    );

    // Distribution sanity (loose, statistically justified — not a
    // hand-worked pin like the EV numbers above, since MC's exact seeded
    // output is only reproducible by running the RNG itself):
    //
    // The rotation's cadence never depends on the RNG (procs are empty,
    // and mana/cooldowns are deterministic — see `sim::exec`'s module
    // docs: only per-cast crit OUTCOMES differ from EV), so every one of
    // the 1000 iterations casts exactly the same 31/29/6 sequence and
    // differs only in each cast's sampled crit/no-crit branch (crit
    // 20%). Per-cast damage spread runs from ~1000 (firebolt, inactive,
    // no-crit) up to ~13900 (fireball, active, crit); with 60
    // quasi-independent Bernoulli(0.2) draws per iteration and 1000
    // iterations, the standard error of the reported MEAN is a couple
    // orders of magnitude below the EV pin itself (single-digit-to-low-
    // tens of dps) — a 2% band around the EV dps pin (3753.31848) is
    // therefore an extremely loose, effectively-never-flaky bound.
    let ev_dps = report.total.dps;
    let tolerance = 0.02 * ev_dps;
    assert!(
        (dist.mean - ev_dps).abs() < tolerance,
        "MC mean {} strayed >{tolerance} from EV dps {ev_dps}",
        dist.mean
    );
    assert!(dist.p10 <= dist.p50, "p10 {} > p50 {}", dist.p10, dist.p50);
    assert!(dist.p50 <= dist.p90, "p50 {} > p90 {}", dist.p50, dist.p90);
    println!("  MC sanity holds: mean within 2% of the EV pin, p10 ≤ p50 ≤ p90 ✓");
}
