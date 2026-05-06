// src/engine/parameters.rs

use std::collections::HashMap;

/// Describes a single tunable parameter for an engine
#[derive(Debug, Clone)]
pub struct ParameterDef {
    /// Unique identifier for this parameter
    pub id: &'static str,
    /// Human-readable name for display
    pub display_name: &'static str,
    /// Description of what this parameter does
    pub description: &'static str,
    /// Minimum allowed value
    pub min: f64,
    /// Maximum allowed value
    pub max: f64,
    /// Default value
    pub default: f64,
    /// Step size for GUI sliders (0.0 means continuous)
    pub step: f64,
}

impl ParameterDef {
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        description: &'static str,
        min: f64,
        max: f64,
        default: f64,
        step: f64,
    ) -> Self {
        Self {
            id,
            display_name,
            description,
            min,
            max,
            default,
            step,
        }
    }
}

/// Current values for all parameters of an engine
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineParameters {
    values: HashMap<String, f64>,
}

impl EngineParameters {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn from_defaults(defs: &[ParameterDef]) -> Self {
        let mut params = Self::new();
        for def in defs {
            params.set(def.id, def.default);
        }
        params
    }

    pub fn get(&self, id: &str) -> Option<f64> {
        self.values.get(id).copied()
    }

    pub fn get_or_default(&self, id: &str, default: f64) -> f64 {
        self.values.get(id).copied().unwrap_or(default)
    }

    pub fn set(&mut self, id: &str, value: f64) {
        self.values.insert(id.to_string(), value);
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Trait for engines that support tunable parameters
pub trait ParameterizedEngine {
    /// Get the list of parameter definitions this engine supports
    fn parameter_definitions(&self) -> &'static [ParameterDef];

    /// Get current parameter values
    fn get_parameters(&self) -> &EngineParameters;

    /// Set parameter values. Returns true if parameters changed and engine needs reinitialization.
    fn set_parameters(&mut self, params: EngineParameters) -> bool;

    /// Called when parameters change to allow the engine to reinitialize
    fn on_parameters_changed(&mut self);
}
