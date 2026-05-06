// src/engine/registry.rs
use crate::engine::personality;
use crate::engine::{
    ChessEngine, ControlEngine, DiffusionEngine, FlowEngine, GravityEngine, InfluenceEngine,
    MctsEngine, OutpostEngine, PressureEngine, ProbabilisticSearchEngine, PstEngine,
    PurePolicyEngine, RandomEngine, SimpleEngine, StaticScoringEngine, SwarmEngine, TacticalEngine,
    TerritoryEngine, VanguardEngine,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EngineType {
    Human,
    Simple,
    Random,
    Tactical,
    PieceSquareTable,
    Swarm,
    Gravity,
    Flow,
    Pressure,
    Outpost,
    Diffusion,
    ProbabilisticSearch,
    Mcts,
    StaticScoring,
    PurePolicy,
    Vanguard,
    Territory,
    Influence, // <-- New
    Control,   // <-- New

    Personality(personality::PersonalityId),
}

impl EngineType {
    pub fn all() -> Vec<Self> {
        let mut v = vec![
            EngineType::Human,
            EngineType::Simple,
            EngineType::Random,
            EngineType::Tactical,
            EngineType::PieceSquareTable,
            EngineType::Swarm,
            EngineType::Gravity,
            EngineType::Flow,
            EngineType::Pressure,
            EngineType::Outpost,
            EngineType::Diffusion,
            EngineType::ProbabilisticSearch,
            EngineType::Mcts,
            EngineType::StaticScoring,
            EngineType::PurePolicy,
            EngineType::Vanguard,
            EngineType::Territory,
            EngineType::Influence, // <-- New
            EngineType::Control,   // <-- New
        ];
        v.extend(
            personality::all_ids()
                .into_iter()
                .map(EngineType::Personality),
        );
        v
    }

    pub fn name(&self) -> &str {
        match self {
            EngineType::Human => "Human Player",
            EngineType::Simple => "Simple Minimax Engine",
            EngineType::Random => "Random Move Engine",
            EngineType::Tactical => "Tactical Priority Engine",
            EngineType::PieceSquareTable => "Piece Square Table Engine",
            EngineType::Swarm => "Swarm Engine",
            EngineType::Gravity => "Gravity Engine (Center of Mass)",
            EngineType::Flow => "Flow Engine (Connectivity)",
            EngineType::Pressure => "Pressure Engine (Zone Control)",
            EngineType::Outpost => "Outpost Engine (Territory Control)",
            EngineType::Diffusion => "Diffusion Engine (Probabilistic Future)",
            EngineType::ProbabilisticSearch => "Probabilistic Search Engine (Best-First PST)",
            EngineType::Mcts => "MCTS Engine (Temperature Tree Search)",
            EngineType::StaticScoring => "Static Scoring Engine",
            EngineType::PurePolicy => "Pure Policy Engine",
            EngineType::Vanguard => "Vanguard Engine (Pure Geometric Policy)",
            EngineType::Territory => "Territory Control Engine",
            EngineType::Influence => "Influence Engine (Raycast Territory)", // <-- New
            EngineType::Control => "Control Engine (Diminishing Territory)", // <-- New

            EngineType::Personality(id) => personality::get(*id)
                .map(|spec| spec.display_name())
                .unwrap_or("👤 (unknown personality)"),
        }
    }

    pub fn create(&self) -> Option<Box<dyn ChessEngine>> {
        match self {
            EngineType::Human => None,
            EngineType::Simple => Some(Box::new(SimpleEngine::new())),
            EngineType::Random => Some(Box::new(RandomEngine::new())),
            EngineType::Tactical => Some(Box::new(TacticalEngine::new())),
            EngineType::PieceSquareTable => Some(Box::new(PstEngine::new())),
            EngineType::Swarm => Some(Box::new(SwarmEngine::new())),
            EngineType::Gravity => Some(Box::new(GravityEngine::new())),
            EngineType::Flow => Some(Box::new(FlowEngine::new())),
            EngineType::Pressure => Some(Box::new(PressureEngine::new())),
            EngineType::Outpost => Some(Box::new(OutpostEngine::new())),
            EngineType::Diffusion => Some(Box::new(DiffusionEngine::new())),
            EngineType::ProbabilisticSearch => Some(Box::new(ProbabilisticSearchEngine::new())),
            EngineType::Mcts => Some(Box::new(MctsEngine::new())),
            EngineType::StaticScoring => Some(Box::new(StaticScoringEngine::new())),
            EngineType::PurePolicy => Some(Box::new(PurePolicyEngine::new())),
            EngineType::Vanguard => Some(Box::new(VanguardEngine::new())),
            EngineType::Territory => Some(Box::new(TerritoryEngine::new())),
            EngineType::Influence => Some(Box::new(InfluenceEngine::new())), // <-- New
            EngineType::Control => Some(Box::new(ControlEngine::new())),     // <-- New

            EngineType::Personality(id) => personality::get(*id).and_then(|spec| spec.create()),
        }
    }

    pub fn is_human(&self) -> bool {
        matches!(self, EngineType::Human)
    }
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Default for EngineType {
    fn default() -> Self {
        EngineType::Human
    }
}
