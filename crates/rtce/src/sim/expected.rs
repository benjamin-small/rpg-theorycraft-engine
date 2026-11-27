//! Pure math for EV-mode expected buff uptime (`ev_uptime`, #33 item 4).
//!
//! Two pieces, both allocation-free once warmed:
//!
//! - [`q_of`]: the probability that a cast event applies a buff AT LEAST
//!   ONCE, conditioning on the crit model exactly as MC realizes it
//!   (`roll_procs_mc`), from a [`NoFire`] product accumulated per proc.
//! - [`RollLog`]: the latest-wins roll log whose weights sum to the
//!   buff's uptime probability at an instant.
//!
//! No default or evaluation instant lives here: callers (the executor)
//! decide when a roll happens and what `chance` evaluates to.

use std::collections::VecDeque;

use crate::simdef::{ProcRolls, Trigger};

/// Running products of `(1 - chance)` over the procs that can apply one
/// buff in one cast event, split by how they are gated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoFire {
    /// `on_cast` / `on_hit` procs: not crit-gated, fully folded (a
    /// `per_hit` one already raised to the hit count).
    indep: f64,
    /// `on_crit` `per_hit` procs, per roll.
    crit_per_hit: f64,
    /// `on_crit` `per_cast` procs.
    crit_per_cast: f64,
    /// `on_non_crit` `per_hit` procs, per roll.
    nc_per_hit: f64,
    /// `on_non_crit` `per_cast` procs.
    nc_per_cast: f64,
}

impl NoFire {
    /// No procs: every product is 1.
    pub(crate) const ONE: NoFire = NoFire {
        indep: 1.0,
        crit_per_hit: 1.0,
        crit_per_cast: 1.0,
        nc_per_hit: 1.0,
        nc_per_cast: 1.0,
    };

    /// Fold one proc in. `chance` is clamped to `[0, 1]` (MC fires once
    /// at `chance > 1`); `n` is the cast's measured hit count.
    pub(crate) fn add(&mut self, trigger: Trigger, rolls: ProcRolls, chance: f64, n: u32) {
        let miss = 1.0 - chance.clamp(0.0, 1.0);
        let per_hit = rolls == ProcRolls::PerHit;
        match trigger {
            Trigger::OnCast | Trigger::OnHit => {
                self.indep *= if per_hit { miss.powi(n as i32) } else { miss };
            }
            Trigger::OnCrit => {
                if per_hit {
                    self.crit_per_hit *= miss;
                } else {
                    self.crit_per_cast *= miss;
                }
            }
            Trigger::OnNonCrit => {
                if per_hit {
                    self.nc_per_hit *= miss;
                } else {
                    self.nc_per_cast *= miss;
                }
            }
        }
    }
}

/// How a cast's hits crit, as MC realizes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CritModel {
    /// A non-damaging cast: crit-gated procs never roll.
    None,
    /// One crit outcome per cast (probability `p`) gates every
    /// crit-gated proc; `n` hits.
    Shared {
        /// Crit probability.
        p: f64,
        /// Hit count.
        n: u32,
    },
    /// Each of `n` hits crits independently with probability `p`.
    Independent {
        /// Per-hit crit probability.
        p: f64,
        /// Hit count.
        n: u32,
    },
}

/// `1 - P(no application)` for one cast event. O(n), no allocation.
pub(crate) fn q_of(nf: &NoFire, crit: CritModel) -> f64 {
    let no_fire = match crit {
        CritModel::None => nf.indep,
        CritModel::Shared { p, n } => {
            let n = n as i32;
            nf.indep
                * (p * nf.crit_per_hit.powi(n) * nf.crit_per_cast
                    + (1.0 - p) * nf.nc_per_hit.powi(n) * nf.nc_per_cast)
        }
        CritModel::Independent { p, n } => {
            // Closed form of sum_k Binomial(n,p)(k) * a^k * b^(n-k) *
            // C^[k>0] * D^[k<n], with a/b = crit/non-crit per_hit,
            // C/D = crit/non-crit per_cast. The bulk term applies C and
            // D to every k; the k=0 term lacks C and the k=n term lacks
            // D, corrected by the other two. O(1), exact, no underflow
            // to a wrong answer (an underflowing term is negligible).
            // n = 0 (no hits): neither crit-gated group rolls.
            if n == 0 {
                nf.indep
            } else {
                let (a, b, c, d) = (
                    nf.crit_per_hit,
                    nf.nc_per_hit,
                    nf.crit_per_cast,
                    nf.nc_per_cast,
                );
                let n = n as i32;
                nf.indep
                    * (c * d * (p * a + (1.0 - p) * b).powi(n)
                        + (1.0 - c) * d * ((1.0 - p) * b).powi(n)
                        + c * (1.0 - d) * (p * a).powi(n))
            }
        }
    };
    1.0 - no_fire
}

/// One application roll: the latest-wins weight of an application that
/// covers until `t_end`.
#[derive(Debug, Clone, Copy)]
struct Entry {
    t_end: f64,
    weight: f64,
    seq: u64,
}

/// The live roll log for one buff. `weight_j = q_j * prod_{k>j}(1 - q_k)`
/// is the probability roll j is the LATEST successful application, so
/// the buff is up at `s` with probability `sum weight_j` over entries
/// with `t_end_j > s`. Dropping an expired entry drops its weight with
/// it and leaves the others exact (their weights already carry the
/// later rolls' failure factors).
#[derive(Debug, Clone, Default)]
pub(crate) struct RollLog {
    entries: VecDeque<Entry>,
}

impl RollLog {
    /// Append a roll (probability `q`, covering until `t_end`, ordered
    /// by `seq`); every older weight is multiplied by `1 - q`.
    pub(crate) fn push(&mut self, t_end: f64, q: f64, seq: u64) {
        let keep = 1.0 - q;
        for e in &mut self.entries {
            e.weight *= keep;
        }
        self.entries.push_back(Entry {
            t_end,
            weight: q,
            seq,
        });
    }

    /// P(buff up at `s`): total weight of entries with `t_end > s`.
    /// The latest-wins reading at an arbitrary instant; the executor
    /// reads [`RollLog::live_weight`] instead (its log is swept by event).
    #[cfg(test)]
    pub(crate) fn p_at(&self, s: f64) -> f64 {
        self.entries
            .iter()
            .filter(|e| e.t_end > s)
            .map(|e| e.weight)
            .sum()
    }

    /// Total weight of every entry still in the log — P(up) NOW for an
    /// executor that removes entries only through its age-out event: an
    /// entry whose `t_end` equals the current instant is still live
    /// until that event pops, exactly as a discrete instance is live
    /// until its `BuffExpire` pops (which a coincident completion can
    /// precede under `completions_first`).
    pub(crate) fn live_weight(&self) -> f64 {
        self.entries.iter().map(|e| e.weight).sum()
    }

    /// Remove entries with `t_end <= s`, weight and all.
    pub(crate) fn drop_expired(&mut self, s: f64) {
        self.entries.retain(|e| e.t_end > s);
    }

    /// The earliest `t_end` and its entry's seq (smallest seq on ties).
    pub(crate) fn next_age_out(&self) -> Option<(f64, u64)> {
        self.entries
            .iter()
            .map(|e| (e.t_end, e.seq))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
    }

    /// Empty the log, keeping its capacity.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simdef::{ProcRolls, Trigger};

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} vs {b}");
    }

    #[test]
    fn q_shared_on_crit_per_hit_n_hits() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerHit, 0.5, 3);
        close(q_of(&nf, CritModel::Shared { p: 0.4, n: 3 }), 0.35);
    }

    #[test]
    fn q_on_crit_plus_on_non_crit_are_exclusive() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerCast, 0.5, 1);
        nf.add(Trigger::OnNonCrit, ProcRolls::PerCast, 0.5, 1);
        close(q_of(&nf, CritModel::Shared { p: 0.25, n: 1 }), 0.5);
    }

    #[test]
    fn q_independent_mixes_per_cast_and_per_hit() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerCast, 1.0, 2);
        close(q_of(&nf, CritModel::Independent { p: 0.5, n: 2 }), 0.75);
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerHit, 0.5, 2);
        // k ~ Bin(2, .5): no-fire = .25 + .5*.5 + .25*.25
        close(
            q_of(&nf, CritModel::Independent { p: 0.5, n: 2 }),
            1.0 - 0.5625,
        );
    }

    #[test]
    fn q_indep_only_for_utility_cast() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCast, ProcRolls::PerCast, 0.3, 1);
        close(q_of(&nf, CritModel::None), 0.3);
    }

    #[test]
    fn q_chance_clamps_above_one() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnHit, ProcRolls::PerCast, 1.7, 1);
        assert_eq!(q_of(&nf, CritModel::None), 1.0);
    }

    #[test]
    fn log_constant_d_matches_product() {
        let mut log = RollLog::default();
        // t = 1, 2, 3 with D 2.5: t_end 3.5, 4.5, 5.5
        for (i, t_end) in [3.5, 4.5, 5.5].into_iter().enumerate() {
            log.push(t_end, 0.5, i as u64);
        }
        assert_eq!(log.p_at(3.2), 0.875);
        log.drop_expired(3.5);
        assert_eq!(log.p_at(3.6), 0.75);
    }

    #[test]
    fn log_variable_d_latest_wins_drop_is_exact() {
        let mut log = RollLog::default();
        log.push(10.0, 0.5, 0); // t 0, D 10
        log.push(2.0, 0.5, 1); // t 1, D 1
        assert_eq!(log.p_at(1.5), 0.75);
        log.drop_expired(2.0);
        assert_eq!(log.p_at(5.0), 0.25);
    }

    #[test]
    fn log_q_one_is_binary() {
        let mut log = RollLog::default();
        log.push(5.0, 0.5, 0);
        log.push(6.0, 1.0, 1);
        assert_eq!(log.p_at(0.0), 1.0);
        assert_eq!(log.p_at(5.5), 1.0);
        log.drop_expired(5.5);
        assert_eq!(log.p_at(5.6), 1.0);
        assert_eq!(log.next_age_out(), Some((6.0, 1)));
    }

    #[test]
    fn log_next_age_out_ties_smallest_seq() {
        let mut log = RollLog::default();
        log.push(4.0, 0.5, 7);
        log.push(4.0, 0.5, 3);
        log.push(9.0, 0.5, 1);
        assert_eq!(log.next_age_out(), Some((4.0, 3)));
        log.clear();
        assert_eq!(log.next_age_out(), None);
    }

    #[test]
    fn q_independent_large_n_does_not_underflow() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerHit, 0.5, 2000);
        let q = q_of(&nf, CritModel::Independent { p: 0.5, n: 2000 });
        close(q, 1.0 - 0.75f64.powi(2000));
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerCast, 0.5, 2000);
        // fires iff k > 0 (certain at n 2000, p .5): q = 0.5 * (1 - 0.5^2000)
        close(q_of(&nf, CritModel::Independent { p: 0.5, n: 2000 }), 0.5);
    }

    #[test]
    fn q_independent_n_one_matches_shared() {
        let mut nf = NoFire::ONE;
        nf.add(Trigger::OnCrit, ProcRolls::PerCast, 0.5, 1);
        nf.add(Trigger::OnNonCrit, ProcRolls::PerCast, 0.5, 1);
        // 0.25*0.5 + 0.75*0.5
        close(q_of(&nf, CritModel::Independent { p: 0.25, n: 1 }), 0.5);
        close(q_of(&nf, CritModel::Shared { p: 0.25, n: 1 }), 0.5);
    }
}
