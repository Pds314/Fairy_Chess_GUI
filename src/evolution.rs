// src/evolution.rs
//
// Genetic-algorithm parameter evolution for a single base engine.
//
// A tournament-like, continuously-running population of individuals, each
// an instance of the SAME base `EngineType` but with its own mutated
// `EngineParameters` — covering every tunable parameter the engine
// exposes, including ones it only has because it wraps/implements shared
// machinery (e.g. the generic search parameters merged in via
// `combined_params`, since `ChessEngine::parameter_definitions()` already
// returns that full, merged list).
//
// Mechanics:
//   * Games are drawn between two distinct individuals with probability
//     biased toward higher-rated ones ("play ELO bias") — so strong
//     individuals accumulate more games (and therefore more confident
//     ratings) than weak ones, rather than everyone playing equally.
//   * Every `games_per_replication` completed games, one replication
//     event fires: a "victim" individual is chosen with probability
//     biased toward LOW rating, and is overwritten in place by a mutated
//     (optionally crossed-over) copy of one or two "parent" individuals
//     chosen with probability biased toward HIGH rating
//     ("replication ELO bias"). The child inherits the parent's rating
//     but starts with zero games played, so its own K-factor is high and
//     it gets re-measured quickly.
//   * Mutation perturbs every *unlocked* parameter by a uniformly-random
//     amount scaled to `mutation_scale * (max - min)` for THAT parameter
//     — so a single 0..1 "mutation scale" setting means something
//     consistent across parameters with wildly different natural ranges.
//   * Locked parameters (`settings.locked_params`) are exempt from both
//     the initial random spread and all subsequent mutation/crossover:
//     every individual simply carries that parameter at its engine
//     default, forever. This is how a user can pin down a subset of an
//     engine's parameters (or its inherited search-machinery toggles)
//     and let evolution explore only the rest.
//   * Crossover (optional): for each (unlocked) parameter, the child's
//     pre-mutation base value is taken from one of the two parents with
//     50/50 probability (uniform crossover), rather than always just one
//     parent.
//   * Optional autosave: every replication cycle, the current best
//     individual's parameters are checkpointed to a `.personality` file
//     on disk (overwritten each time), so a long-running evolution isn't
//     lost if the process is closed before anyone remembers to export.
//
// Ratings use the exact same statistically-motivated K-factor decay as
// the main tournament system (`tournament::dynamic_k`), so a freshly
// spawned individual (zero games) gets fast, high-K recalibration while
// a long-lived, well-measured individual settles down.
//
// ── PRNG choice ──────────────────────────────────────────────────────────
// This uses `rand::rngs::StdRng` seeded via `SeedableRng::seed_from_u64`,
// the same approach `zobrist.rs` already uses elsewhere in this crate.
// There's nothing about selection/mutation here that needs a hand-rolled
// generator or a specific bitstream — the only property that actually
// matters is "reproducible from a seed," and `StdRng` already gives us
// that for free while being a standard, well-tested implementation.
//
// ── Saving ────────────────────────────────────────────────────────────────
// This module intentionally does NOT know about `AssetManager` or how the
// project's asset root is discovered — that's GUI/app-layer concern. By
// the time a path reaches `maybe_autosave` or `export_personality`'s
// caller, it's expected to already be an absolute, resolved path (see
// `handlers/evolution.rs` and `commands/evolution.rs`, which resolve
// through `AssetManager::resolve_save_path` before ever touching
// `EvolutionSettings::autosave_path`).
use crate::clog;
use crate::engine::{ChessEngine, EngineParameters, EngineType, ParameterDef};
use crate::tournament::dynamic_k;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
#[derive(Debug, Clone)]
pub struct EvolutionSettings {
    /// Number of individuals in the population. Fixed for the life of a
    /// run — replication replaces the weakest individual in place rather
    /// than growing the population.
    pub population: usize,
    /// How strongly play-selection favours high-rated individuals.
    /// 0 = uniform (everyone plays equally often regardless of rating).
    /// Higher values exponentially favour strong individuals getting more
    /// games (and therefore faster, more confident rating convergence).
    pub play_elo_bias: f64,
    /// How strongly replication favours high-rated parents / low-rated
    /// victims. 0 = both parent and victim chosen uniformly at random
    /// (pure random drift). Higher values make replication look much
    /// more like "the best individuals breed, the worst get replaced."
    pub replication_elo_bias: f64,
    /// Mutation magnitude as a FRACTION of each parameter's own
    /// `[min, max]` clamping range. 0 = no mutation (pure selection on
    /// the initial random population); larger values explore faster but
    /// destabilise convergence. Has no effect on locked parameters.
    pub mutation_scale: f64,
    /// Whether replication mixes two parents (uniform crossover per
    /// parameter) or clones a single parent before mutating.
    pub use_crossover: bool,
    /// Parameter IDs that are exempt from randomisation/mutation: every
    /// individual carries these at the engine's own default value,
    /// always. Lets a user pin down parameters (including inherited
    /// search-machinery toggles) they don't want evolution touching.
    pub locked_params: HashSet<String>,
    /// Trigger one replication event (and, if enabled, one autosave)
    /// every time this many games have been played in total. Defaults to
    /// roughly `population`, so the whole population gets a chance to
    /// turn over about once per "generation" worth of games.
    pub games_per_replication: usize,
    /// Ply cap per game — beyond this, the game is adjudicated a draw.
    /// Same mechanism tournaments use (`Termination::PlyLimit`).
    pub max_plies: usize,
    /// How many games to run concurrently. Read fresh on every dispatch
    /// tick, so this can be changed live while evolution is running.
    pub parallelism: usize,
    /// Rating K-factor decay parameters — see `tournament::dynamic_k`.
    pub k_initial: f64,
    pub k_floor: f64,
    pub k_scale: f64,
    /// If true, every `games_per_replication` games the current best
    /// individual's parameters are written to `autosave_path` as a
    /// `.personality` file. This is a checkpoint, not a log: each write
    /// overwrites the previous one, so `autosave_path` always holds the
    /// best-so-far. Live-toggleable while running. Expected to already
    /// be an absolute, resolved path by the time it lands here.
    pub autosave_enabled: bool,
    pub autosave_path: String,
}
impl Default for EvolutionSettings {
    fn default() -> Self {
        Self {
            population: 12,
            play_elo_bias: 1.0,
            replication_elo_bias: 1.0,
            mutation_scale: 0.08,
            use_crossover: true,
            locked_params: HashSet::new(),
            games_per_replication: 12,
            max_plies: 400,
            parallelism: 1,
            k_initial: 48.0,
            k_floor: 8.0,
            k_scale: 12.0,
            autosave_enabled: false,
            autosave_path: String::new(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Individual {
    pub id: u64,
    pub params: EngineParameters,
    pub rating: f64,
    pub games_played: u32,
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
    pub generation: u32,
    pub parent: Option<u64>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum EvolutionPhase {
    Inactive,
    Running,
}
pub struct EvolutionState {
    pub phase: EvolutionPhase,
    pub base_engine: EngineType,
    pub settings: EvolutionSettings,
    pub population: Vec<Individual>,
    pub games_played: u64,
    defs: &'static [ParameterDef],
    next_id: u64,
    rng: StdRng,
}
impl EvolutionState {
    pub fn new() -> Self {
        Self {
            phase: EvolutionPhase::Inactive,
            base_engine: EngineType::Simple,
            settings: EvolutionSettings::default(),
            population: Vec::new(),
            games_played: 0,
            defs: &[],
            next_id: 0,
            rng: StdRng::seed_from_u64(0x9E37_79B9_7F4A_7C15),
        }
    }
    pub fn is_active(&self) -> bool {
        self.phase == EvolutionPhase::Running
    }
    /// The full parameter-definition list currently in play (empty if
    /// evolution has never been started). Exposed so the GUI's "locked
    /// parameters" checklist can reflect the actually-running set rather
    /// than guessing.
    pub fn param_defs(&self) -> &'static [ParameterDef] {
        self.defs
    }
    // ─── RNG ─────────────────────────────────────────────────────────────
    fn rand_f64(&mut self) -> f64 {
        self.rng.random::<f64>()
    }
    /// Uniform in [-1, 1].
    fn rand_signed(&mut self) -> f64 {
        self.rand_f64() * 2.0 - 1.0
    }
    // ─── Lifecycle ───────────────────────────────────────────────────────
    /// Start (or restart) evolution for `base_engine` with the given
    /// settings. A throwaway instance of the engine is built solely to
    /// read its `parameter_definitions()` — which already includes any
    /// parameters inherited from shared machinery the engine wraps (e.g.
    /// the generic search tunables merged in via `combined_params`), so
    /// evolution automatically covers "all of the parameters... including
    /// those inherited from an engine it implements."
    pub fn start(
        &mut self,
        base_engine: EngineType,
        settings: EvolutionSettings,
        seed: u64,
    ) -> Result<(), String> {
        if base_engine.is_human() {
            return Err("cannot evolve a human player".to_string());
        }
        let template = base_engine
        .create()
        .ok_or_else(|| "engine failed to construct".to_string())?;
        let defs = template
        .parameter_definitions()
        .ok_or_else(|| format!("{} has no tunable parameters", base_engine.name()))?;
        if settings.population < 2 {
            return Err("population must be at least 2".to_string());
        }
        self.rng = StdRng::seed_from_u64(seed);
        self.base_engine = base_engine;
        self.settings = settings;
        self.defs = defs;
        self.games_played = 0;
        self.next_id = 0;
        let mut population = Vec::with_capacity(self.settings.population);
        for _ in 0..self.settings.population {
            population.push(self.spawn_random_individual());
        }
        self.population = population;
        self.phase = EvolutionPhase::Running;
        Ok(())
    }
    pub fn stop(&mut self) {
        self.phase = EvolutionPhase::Inactive;
    }
    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    /// A brand-new individual: every UNLOCKED parameter is drawn
    /// uniformly from its full `[min, max]` range (the initial
    /// population's diversity); every LOCKED parameter is pinned at the
    /// engine's own default.
    fn spawn_random_individual(&mut self) -> Individual {
        let mut params = EngineParameters::new();
        for def in self.defs {
            let v = if self.settings.locked_params.contains(def.id) {
                def.default
            } else {
                let t = self.rand_f64();
                def.min + t * (def.max - def.min)
            };
            params.set(def.id, v);
        }
        let id = self.fresh_id();
        Individual {
            id,
            params,
            rating: 1500.0,
            games_played: 0,
            wins: 0,
            draws: 0,
            losses: 0,
            generation: 0,
            parent: None,
        }
    }
    fn mean_rating(&self) -> f64 {
        if self.population.is_empty() {
            return 1500.0;
        }
        self.population.iter().map(|i| i.rating).sum::<f64>() / self.population.len() as f64
    }
    /// `exp(bias * (rating - mean) / 400)`: bias = 0 gives a uniform
    /// weight for everyone; bias > 0 exponentially favours high rating;
    /// a NEGATIVE bias favours low rating (used for victim selection, by
    /// simply negating the same bias value).
    fn elo_weight(rating: f64, mean: f64, bias: f64) -> f64 {
        (bias * (rating - mean) / 400.0).exp()
    }
    /// Weighted-by-ELO roulette selection over the population. `exclude`
    /// masks out an index already claimed by the caller (e.g. the
    /// opponent, or the other parent). Returns `None` only when every
    /// candidate's weight is (numerically) zero.
    fn select_weighted(&mut self, bias: f64, exclude: Option<usize>) -> Option<usize> {
        let mean = self.mean_rating();
        let weights: Vec<f64> = self
        .population
        .iter()
        .enumerate()
        .map(|(i, ind)| {
            if Some(i) == exclude {
                0.0
            } else {
                Self::elo_weight(ind.rating, mean, bias)
            }
        })
        .collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let pick = self.rand_f64() * total;
        let mut acc = 0.0;
        for (i, w) in weights.iter().enumerate() {
            acc += w;
            if pick < acc {
                return Some(i);
            }
        }
        weights.iter().rposition(|&w| w > 0.0)
    }
    /// Draw a pairing of two distinct population indices, biased toward
    /// high-rated individuals playing more often ("play ELO bias").
    pub fn draw_pairing(&mut self) -> Option<(usize, usize)> {
        if self.population.len() < 2 {
            return None;
        }
        let bias = self.settings.play_elo_bias;
        let a = self.select_weighted(bias, None)?;
        let b = self.select_weighted(bias, Some(a))?;
        Some((a, b))
    }
    /// Record a finished game's outcome (`white_score` in `[0,1]`, same
    /// convention as `GameOutcome::white_score()`) between two population
    /// indices, updating ratings with the shared dynamic-K formula. May
    /// trigger a replication event (and, if enabled, an autosave).
    pub fn record_game(&mut self, white_idx: usize, black_idx: usize, white_score: f64) {
        if white_idx >= self.population.len()
            || black_idx >= self.population.len()
            || white_idx == black_idx
            {
                return;
            }
            let r_white = self.population[white_idx].rating;
        let r_black = self.population[black_idx].rating;
        let expected_white = 1.0 / (1.0 + 10f64.powf((r_black - r_white) / 400.0));
        let k_white = dynamic_k(
            self.population[white_idx].games_played,
            self.settings.k_initial,
            self.settings.k_floor,
            self.settings.k_scale,
        );
        let k_black = dynamic_k(
            self.population[black_idx].games_played,
            self.settings.k_initial,
            self.settings.k_floor,
            self.settings.k_scale,
        );
        let white_delta = k_white * (white_score - expected_white);
        let black_delta = k_black * ((1.0 - white_score) - (1.0 - expected_white));
        self.population[white_idx].rating += white_delta;
        self.population[black_idx].rating += black_delta;
        self.population[white_idx].games_played += 1;
        self.population[black_idx].games_played += 1;
        if white_score > 0.75 {
            self.population[white_idx].wins += 1;
            self.population[black_idx].losses += 1;
        } else if white_score < 0.25 {
            self.population[white_idx].losses += 1;
            self.population[black_idx].wins += 1;
        } else {
            self.population[white_idx].draws += 1;
            self.population[black_idx].draws += 1;
        }
        self.games_played += 1;
        if self.settings.games_per_replication > 0
            && self.games_played % self.settings.games_per_replication as u64 == 0
            {
                self.replicate_once();
                self.maybe_autosave();
            }
    }
    /// One replication event: cull a low-rated individual and replace it
    /// (in place, same population slot) with a mutated — optionally
    /// crossed-over — copy of one or two high-rated parents.
    fn replicate_once(&mut self) {
        if self.population.len() < 2 {
            return;
        }
        let bias = self.settings.replication_elo_bias;
        let Some(parent_a) = self.select_weighted(bias, None) else {
            return;
        };
        let parent_b = if self.settings.use_crossover {
            self.select_weighted(bias, Some(parent_a))
        } else {
            None
        };
        // Victim selection uses the SAME weighting function with the
        // bias negated, so high replication_elo_bias makes low-rated
        // individuals much more likely to be culled.
        let Some(victim) = self.select_weighted(-bias, None) else {
            return;
        };
        let child_params = self.make_child_params(parent_a, parent_b);
        let parent_rating = self.population[parent_a].rating;
        let parent_id = self.population[parent_a].id;
        let parent_b_id = parent_b.and_then(|b| self.population.get(b)).map(|i| i.id);
        let gen_a = self.population[parent_a].generation;
        let gen_b = parent_b.map(|b| self.population[b].generation).unwrap_or(0);
        let gener = gen_a.max(gen_b) + 1;
        let victim_id = self.population[victim].id;
        let new_id = self.fresh_id();
        self.population[victim] = Individual {
            id: new_id,
            params: child_params,
            rating: parent_rating,
            games_played: 0,
            wins: 0,
            draws: 0,
            losses: 0,
            generation: gener,
            parent: Some(parent_id),
        };
        match parent_b_id {
            Some(pb) => clog!(
                "🧬 Evolution: individual #{} replaced by offspring #{} of #{} × #{} (gen {}, inherited rating {:.0})",
                              victim_id, new_id, parent_id, pb, gener, parent_rating
            ),
            None => clog!(
                "🧬 Evolution: individual #{} replaced by offspring #{} of #{} (gen {}, inherited rating {:.0})",
                          victim_id, new_id, parent_id, gener, parent_rating
            ),
        }
    }
    /// Build a child's parameter set: per UNLOCKED parameter, pick a base
    /// value from parent `a` (or, with crossover, a 50/50 coin flip
    /// between `a` and `b`), then perturb it by a uniformly-random amount
    /// scaled to `mutation_scale * (max - min)` for that parameter,
    /// clamped back into range. Every LOCKED parameter is forced to the
    /// engine's default regardless of parentage.
    fn make_child_params(&mut self, a: usize, b: Option<usize>) -> EngineParameters {
        let mut child = EngineParameters::new();
        for def in self.defs {
            if self.settings.locked_params.contains(def.id) {
                child.set(def.id, def.default);
                continue;
            }
            let base_value = match b {
                Some(bi) if self.rand_f64() < 0.5 => {
                    self.population[bi].params.get_or_default(def.id, def.default)
                }
                _ => self.population[a].params.get_or_default(def.id, def.default),
            };
            let range = (def.max - def.min).max(1e-9);
            let delta = self.rand_signed() * self.settings.mutation_scale * range;
            let mutated = (base_value + delta).clamp(def.min, def.max);
            child.set(def.id, mutated);
        }
        child
    }
    pub fn best(&self) -> Option<&Individual> {
        self.population
        .iter()
        .max_by(|a, b| a.rating.partial_cmp(&b.rating).unwrap_or(std::cmp::Ordering::Equal))
    }
    pub fn sorted_by_rating(&self) -> Vec<&Individual> {
        let mut v: Vec<&Individual> = self.population.iter().collect();
        v.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
    pub fn find(&self, id: u64) -> Option<&Individual> {
        self.population.iter().find(|i| i.id == id)
    }
    /// Build a playable engine instance from an individual: the base
    /// engine type, with that individual's parameters applied.
    pub fn build_engine(&self, individual: &Individual) -> Option<Box<dyn ChessEngine>> {
        let mut engine = self.base_engine.create()?;
        engine.set_parameters(individual.params.clone());
        Some(engine)
    }
    pub fn print_status(&self) {
        if !self.is_active() && self.population.is_empty() {
            clog!("🧬 Evolution is not running.");
            return;
        }
        clog!(
            "🧬 Evolution status: {} — {} games played ({})",
              self.base_engine.name(),
              self.games_played,
              if self.is_active() { "running" } else { "stopped" }
        );
        clog!(
            "   population {} | play bias {:.2} | repl bias {:.2} | mutation {:.2} | crossover {} | repl every {} games | max plies {} | parallel {}",
            self.population.len(),
              self.settings.play_elo_bias,
              self.settings.replication_elo_bias,
              self.settings.mutation_scale,
              self.settings.use_crossover,
              self.settings.games_per_replication,
              self.settings.max_plies,
              self.settings.parallelism,
        );
        if !self.settings.locked_params.is_empty() {
            let mut locked: Vec<&str> = self.settings.locked_params.iter().map(|s| s.as_str()).collect();
            locked.sort_unstable();
            clog!("   locked params: {}", locked.join(", "));
        }
        if self.settings.autosave_enabled {
            clog!(
                "   autosave: every {} games → {}",
                self.settings.games_per_replication,
                if self.settings.autosave_path.is_empty() {
                    "(no path set)"
                } else {
                    &self.settings.autosave_path
                }
            );
        }
        for ind in self.sorted_by_rating() {
            clog!(
                "   #{:<4} gen {:<3} rating {:>6.0}  {}g (+{} ={} -{})  parent {}",
                  ind.id,
                  ind.generation,
                  ind.rating,
                  ind.games_played,
                  ind.wins,
                  ind.draws,
                  ind.losses,
                  ind.parent.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string()),
            );
        }
    }
    /// Export an individual's parameters as a `.personality` file body
    /// (same `key: value` format `PersonalitySpec::parse` reads), so an
    /// evolved engine instance can be saved and reused like any
    /// hand-tuned personality.
    pub fn export_personality(&self, id: u64) -> Option<String> {
        let ind = self.find(id)?;
        let mut out = String::new();
        out.push_str(&format!(
            "name: {} evolved #{}\n",
            self.base_engine.name(),
                              ind.id
        ));
        out.push_str(&format!("engine: {}\n", self.base_engine.name()));
        for def in self.defs {
            let v = ind.params.get_or_default(def.id, def.default);
            out.push_str(&format!("{}: {}\n", def.id, v));
        }
        Some(out)
    }
    /// If autosave is enabled and a path is set, checkpoint the current
    /// best individual to disk, overwriting any previous checkpoint.
    /// `autosave_path` is expected to already be an absolute, resolved
    /// path — see the module doc.
    fn maybe_autosave(&self) {
        if !self.settings.autosave_enabled {
            return;
        }
        let path = self.settings.autosave_path.trim();
        if path.is_empty() {
            return;
        }
        let Some(best) = self.best() else {
            return;
        };
        let best_id = best.id;
        let best_rating = best.rating;
        let Some(content) = self.export_personality(best_id) else {
            return;
        };
        match std::fs::write(path, &content) {
            Ok(()) => clog!(
                "💾 Evolution autosave: best individual #{} (rating {:.0}) → {}",
                            best_id, best_rating, path
            ),
            Err(e) => clog!("⚠️ Evolution autosave to '{}' failed: {}", path, e),
        }
    }
}
impl Default for EvolutionState {
    fn default() -> Self {
        Self::new()
    }
}
/// Turn an engine's display name into a filesystem-friendly slug, e.g.
/// "Piece Square Table Engine" → "piece_square_table_engine". Shared by
/// the default autosave filename and the "export best" action so both
/// produce consistent, collision-resistant filenames.
pub fn slugify_engine_name(engine: &EngineType) -> String {
    engine
    .name()
    .chars()
    .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
    .collect()
}
/// Filename (not a full path — callers resolve that via
/// `AssetManager::resolve_save_path`) for a one-off export of a specific
/// individual, distinct per engine+id so repeated exports don't silently
/// clobber each other.
pub fn export_filename_for(engine: &EngineType, individual_id: u64) -> String {
    format!("{}_best_{}.personality", slugify_engine_name(engine), individual_id)
}
