// src/engine/parameters.rs
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ParameterDef {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub step: f64,
}

impl ParameterDef {
    pub const fn new(
        id: &'static str, display_name: &'static str, description: &'static str,
        min: f64, max: f64, default: f64, step: f64,
    ) -> Self {
        Self { id, display_name, description, min, max, default, step }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineParameters {
    values: HashMap<String, f64>,
}

impl EngineParameters {
    pub fn new() -> Self { Self { values: HashMap::new() } }

    pub fn from_defaults(defs: &[ParameterDef]) -> Self {
        let mut p = Self::new();
        for d in defs { p.set(d.id, d.default); }
        p
    }

    pub fn get(&self, id: &str) -> Option<f64> { self.values.get(id).copied() }

    pub fn get_or_default(&self, id: &str, default: f64) -> f64 {
        self.values.get(id).copied().unwrap_or(default)
    }

    pub fn set(&mut self, id: &str, value: f64) {
        self.values.insert(id.to_string(), value);
    }

    pub fn is_empty(&self) -> bool { self.values.is_empty() }
}

// ParameterizedEngine trait REMOVED — its methods are now defaults
// on ChessEngine. Engines override them directly.
