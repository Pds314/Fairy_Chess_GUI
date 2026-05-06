// src/engine/personality.rs
//
// Engine "personalities": named parameter presets for a base engine.
//
// A personality is a (name, base engine, parameter overrides) triple loaded
// from a `.personality` file. Once loaded, it behaves exactly like a
// built-in engine — shows up in picklists, can be assigned to white/black,
// can be entered in tournaments — but under the hood it's just the base
// engine with `set_parameters()` called on it at construction time.
//
// The registry lives in a global `OnceLock`. We populate it once at
// startup by scanning `assets/personalities/`, and it's immutable
// thereafter. That makes `EngineType::Personality(id)` cheap to clone (one
// usize) and cheap to compare (integer equality), which matters because
// EngineType is used as a HashMap key throughout the tournament system.

use crate::engine::{ChessEngine, EngineParameters, EngineType};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

// ─────────────────────────────────────────────────────────────────────────────
// Personality spec
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PersonalitySpec {
    pub name: String,
    pub base_engine: EngineType,
    pub overrides: HashMap<String, f64>,
    /// Pre-computed display name ("👤 {name}"). Stored here so that
    /// `EngineType::name()` can return `&'static str` by borrowing from
    /// the global registry without leaking.
    cached_display_name: String,
}

impl PersonalitySpec {
    pub fn parse(content: &str, source: &str) -> Result<Self, String> {
        let mut name: Option<String> = None;
        let mut engine_str: Option<String> = None;
        let mut overrides: HashMap<String, f64> = HashMap::new();

        for (lineno, raw) in content.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }

            let Some((key, value)) = line.split_once(':') else {
                eprintln!(
                    "⚠️  {}:{}: expected `key: value`, got `{}`",
                    source,
                    lineno + 1,
                    line
                );
                continue;
            };

            let key = key.trim();
            let value = value.trim();

            match key {
                "name" => {
                    name = Some(value.to_string());
                }
                "engine" | "base" | "base_engine" => {
                    engine_str = Some(value.to_string());
                }
                _ => match value.parse::<f64>() {
                    Ok(v) => {
                        overrides.insert(key.to_string(), v);
                    }
                    Err(_) => {
                        eprintln!(
                            "⚠️  {}:{}: parameter `{}` has non-numeric value `{}`, skipping",
                            source,
                            lineno + 1,
                            key,
                            value
                        );
                    }
                },
            }
        }

        let engine_str = engine_str.ok_or_else(|| {
            format!(
                "{}: missing `engine:` line (which base engine does this wrap?)",
                source
            )
        })?;

        let base_engine = parse_engine_name(&engine_str).ok_or_else(|| {
            format!(
                "{}: unknown engine `{}` — use 'engine' terminal command to list valid names",
                source, engine_str
            )
        })?;

        if base_engine.is_human() {
            return Err(format!(
                "{}: can't make a personality out of a human",
                source
            ));
        }

        let name = name.unwrap_or_else(|| {
            Path::new(source)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unnamed")
                .to_string()
        });

        let cached_display_name = format!("👤 {}", name);

        Ok(Self {
            name,
            base_engine,
            overrides,
            cached_display_name,
        })
    }

    pub fn create(&self) -> Option<Box<dyn ChessEngine>> {
        let mut engine = self.base_engine.create()?;

        if self.overrides.is_empty() {
            return Some(engine);
        }

        let mut params = engine
            .get_parameters()
            .unwrap_or_else(EngineParameters::new);
        for (k, &v) in &self.overrides {
            params.set(k, v);
        }
        engine.set_parameters(params);

        Some(engine)
    }

    /// Display name for picklists and logs. Returns a reference into the
    /// `'static` registry, so callers of `EngineType::name()` get
    /// `&'static str` without leaking.
    pub fn display_name(&self) -> &str {
        &self.cached_display_name
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Engine-name parsing — fully dynamic
// ─────────────────────────────────────────────────────────────────────────────

/// Normalize a string for fuzzy matching: lowercase, strip everything
/// except alphanumerics. "Piece Square Table" → "piecesquaretable".
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Map a human-friendly engine name to `EngineType`.
///
/// This is **dynamic**: it derives matches from the display names returned
/// by `EngineType::name()` for every built-in variant, plus personality
/// names from the registry if it has been loaded. Adding a new engine to
/// `EngineType::all()` automatically makes it matchable here — no
/// hardcoded match arms to update.
///
/// Matching is case-insensitive, ignores separators, and supports
/// substring matching (e.g., "gravity" matches "Gravity Engine (Center
/// of Mass)"). A small alias table covers abbreviations that can't be
/// derived from display names (e.g., "pst" → PieceSquareTable).
///
/// Safe to call during personality loading: `EngineType::all()` returns
/// only built-ins when the personality registry hasn't been populated yet.
pub fn parse_engine_name(s: &str) -> Option<EngineType> {
    let input_norm = normalize(s);
    if input_norm.is_empty() {
        return None;
    }

    // ── Short aliases ────────────────────────────────────────────────────
    // Abbreviations that don't naturally arise from normalized display
    // names. Keep this list minimal — dynamic matching handles the rest.
    match input_norm.as_str() {
        "pst" => return Some(EngineType::PieceSquareTable),
        "mcts" | "montecarlo" => return Some(EngineType::Mcts),
        _ => {}
    }

    // ── Built-in engines ─────────────────────────────────────────────────
    let all = EngineType::all();
    let builtins: Vec<&EngineType> = all
        .iter()
        .filter(|e| !e.is_human() && !matches!(e, EngineType::Personality(_)))
        .collect();

    // Pass 1: exact match on normalized display name.
    for et in &builtins {
        if normalize(et.name()) == input_norm {
            return Some((*et).clone());
        }
    }

    // Pass 2: input is a substring of a normalized name. Prefer the
    // shortest matching name to avoid "flow" accidentally matching
    // "probabilisticsearchenginebestfirstpst" via some shared substring.
    let mut best: Option<(usize, EngineType)> = None;
    for et in &builtins {
        let name_norm = normalize(et.name());
        if name_norm.contains(&input_norm) {
            let len = name_norm.len();
            if best.as_ref().map_or(true, |(bl, _)| len < *bl) {
                best = Some((len, (*et).clone()));
            }
        }
    }
    if let Some((_, et)) = best {
        return Some(et);
    }

    // ── Personalities ────────────────────────────────────────────────────
    // If the registry is populated (i.e., we're past startup), also search
    // personality names. During personality loading this section is a no-op
    // because the OnceLock hasn't been set yet — exactly right, since
    // personality files shouldn't reference other personalities.
    if let Some(specs) = REGISTRY.get() {
        // Exact match on personality name.
        for (id, spec) in specs.iter().enumerate() {
            if normalize(&spec.name) == input_norm {
                return Some(EngineType::Personality(id));
            }
        }
        // Substring match.
        for (id, spec) in specs.iter().enumerate() {
            let name_norm = normalize(&spec.name);
            if name_norm.contains(&input_norm) || input_norm.contains(&name_norm) {
                return Some(EngineType::Personality(id));
            }
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Global registry
// ─────────────────────────────────────────────────────────────────────────────

static REGISTRY: OnceLock<Vec<PersonalitySpec>> = OnceLock::new();

pub type PersonalityId = usize;

pub fn load_from_dir(dir: &Path) -> usize {
    if let Some(existing) = REGISTRY.get() {
        return existing.len();
    }

    let mut specs = Vec::new();
    let mut seen_names: HashMap<String, String> = HashMap::new();

    if let Ok(entries) = fs::read_dir(dir) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("personality"))
            .collect();
        files.sort();

        for path in files {
            let source = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();

            match fs::read_to_string(&path) {
                Ok(content) => match PersonalitySpec::parse(&content, &source) {
                    Ok(spec) => {
                        if let Some(prev) = seen_names.get(&spec.name) {
                            eprintln!(
                                "⚠️  {}: personality name `{}` already used by {}; skipping",
                                source, spec.name, prev
                            );
                            continue;
                        }
                        seen_names.insert(spec.name.clone(), source.clone());

                        println!(
                            "👤 Loaded personality `{}` ({} + {} overrides) from {}",
                            spec.name,
                            spec.base_engine.name(),
                            spec.overrides.len(),
                            source
                        );
                        specs.push(spec);
                    }
                    Err(e) => {
                        eprintln!("❌ {}", e);
                    }
                },
                Err(e) => {
                    eprintln!("❌ {}: read failed: {}", source, e);
                }
            }
        }
    }

    let count = specs.len();
    let _ = REGISTRY.set(specs);
    count
}

pub fn get(id: PersonalityId) -> Option<&'static PersonalitySpec> {
    REGISTRY.get()?.get(id)
}

pub fn all_ids() -> Vec<PersonalityId> {
    match REGISTRY.get() {
        Some(v) => (0..v.len()).collect(),
        None => Vec::new(),
    }
}

pub fn count() -> usize {
    REGISTRY.get().map(|v| v.len()).unwrap_or(0)
}
