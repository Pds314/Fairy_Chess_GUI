// src/engine/analysis.rs

use crate::core::game_state::ExpandedMove;
use crate::core::piece::PieceColor;
use crate::core::position::Position;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MoveEvaluation {
    pub mv: ExpandedMove,
    pub opponent_evaluation: i32,
}

/// Comprehensive position analysis data
#[derive(Debug, Clone)]
pub struct PositionAnalysis {
    /// Total material value by color
    pub material_values: MaterialAnalysis,
    /// Piece square table analysis (PST engines only)
    pub pst_analysis: Option<PstAnalysis>,
    /// Mobility analysis
    pub mobility_analysis: MobilityAnalysis,
    /// Board density and piece distribution
    pub density_analysis: DensityAnalysis,
    /// Value ratios and statistical analysis
    pub statistical_analysis: StatisticalAnalysis,
}

#[derive(Debug, Clone)]
pub struct MaterialAnalysis {
    pub white_total: f64,
    pub black_total: f64,
    pub difference: f64,
    pub piece_counts: HashMap<usize, (u32, u32)>, // piece_type -> (white_count, black_count)
    pub piece_values: HashMap<usize, f64>,        // piece_type -> average_value
}

#[derive(Debug, Clone)]
pub struct PstAnalysis {
    /// Total PST value for each color
    pub white_pst_total: f64,
    pub black_pst_total: f64,
    pub pst_difference: f64,
    /// Per-piece PST statistics
    pub piece_pst_stats: HashMap<usize, PiecePstStats>,
    /// Positional variance and bias analysis
    pub variance_analysis: VarianceAnalysis,
    /// Swarm and tactical factors
    pub swarm_factors: SwarmAnalysis,
}

#[derive(Debug, Clone)]
pub struct PiecePstStats {
    pub piece_type: usize,
    pub min_value: f64,
    pub max_value: f64,
    pub average_value: f64,
    pub variance: f64,
    pub standard_deviation: f64,
    pub current_total: f64,
    pub current_count: u32,
    pub current_average: f64,
}

#[derive(Debug, Clone)]
pub struct VarianceAnalysis {
    /// Directional bias in piece placement
    pub positional_bias: PositionalBias,
    /// Heat map of high-value squares
    pub value_distribution: ValueDistribution,
}

#[derive(Debug, Clone)]
pub struct PositionalBias {
    pub forward_bias: f64,    // Positive = pieces prefer forward squares
    pub backward_bias: f64,   // Positive = pieces prefer back rank
    pub center_bias: f64,     // Positive = pieces prefer center
    pub edge_bias: f64,       // Positive = pieces prefer edges
    pub left_right_bias: f64, // Positive = prefer right, negative = prefer left
}

#[derive(Debug, Clone)]
pub struct ValueDistribution {
    pub highest_value_squares: Vec<(Position, f64)>,
    pub lowest_value_squares: Vec<(Position, f64)>,
    pub value_range: f64,
    pub value_variance: f64,
}

#[derive(Debug, Clone)]
pub struct SwarmAnalysis {
    pub average_swarm_bonus: f64,
    pub max_swarm_position: Option<(Position, f64)>,
    pub swarm_effectiveness: f64,
    pub huddle_factor: f64,
}

#[derive(Debug, Clone)]
pub struct MobilityAnalysis {
    /// Raw mobility counts (correctly calculated per side)
    pub white_mobility: u32,
    pub black_mobility: u32,
    pub mobility_difference: i32,
    /// Mobility per piece type
    pub piece_mobility: HashMap<usize, MobilityStats>,
    /// Value to mobility ratios
    pub value_to_mobility_ratio: f64,
    /// Theoretical mobility analysis
    pub theoretical_mobility: HashMap<usize, TheoreticalMobilityStats>,
    /// Threat analysis
    pub threat_analysis: ThreatAnalysis,
}

#[derive(Debug, Clone)]
pub struct MobilityStats {
    pub total_moves: u32,
    pub piece_count: u32,
    pub average_mobility: f64,
    pub mobility_variance: f64,
    pub attacking_moves: u32,
    pub non_attacking_moves: u32,
}

#[derive(Debug, Clone)]
pub struct TheoreticalMobilityStats {
    pub piece_type: usize,
    /// Average mobility on empty board from center
    pub center_mobility: f64,
    /// Average mobility on empty board from corners  
    pub corner_mobility: f64,
    /// Average mobility on empty board from edges
    pub edge_mobility: f64,
    /// Standard deviation of mobility across different positions
    pub mobility_variance: f64,
    /// Concentration measure (low = diffuse, high = concentrated)
    pub concentration_factor: f64,
    /// Maximum theoretical mobility for this piece type
    pub max_mobility: u32,
    /// Minimum theoretical mobility for this piece type
    pub min_mobility: u32,
}

#[derive(Debug, Clone)]
pub struct ThreatAnalysis {
    /// Value of pieces threatened by current player
    pub white_threats_value: f64,
    /// Value of pieces threatened by opponent
    pub black_threats_value: f64,
    /// Value of pieces threatening (attackers)
    pub white_attackers_value: f64,
    /// Value of pieces threatening (attackers)
    pub black_attackers_value: f64,
    /// Percentage of current mobility that is capturing
    pub white_capture_mobility_percentage: f64,
    pub black_capture_mobility_percentage: f64,
    /// Threat balance (threatened - threatening)
    pub white_threat_balance: f64,
    pub black_threat_balance: f64,
}

#[derive(Debug, Clone)]
pub struct DensityAnalysis {
    pub board_density: f64,       // pieces / total_squares
    pub piece_density_ratio: f64, // value / density
    pub clustering: ClusteringAnalysis,
}

#[derive(Debug, Clone)]
pub struct ClusteringAnalysis {
    pub average_piece_distance: f64,
    pub clustering_coefficient: f64,
    pub isolated_pieces: Vec<Position>,
    pub dense_regions: Vec<(Position, f64)>,
}

#[derive(Debug, Clone)]
pub struct StatisticalAnalysis {
    /// All values normalized to weakest piece value
    pub normalized_values: NormalizedValues,
    /// Statistical measures
    pub statistics: PositionStatistics,
}

#[derive(Debug, Clone)]
pub struct NormalizedValues {
    pub weakest_piece_value: f64,
    pub white_total_normalized: f64,
    pub black_total_normalized: f64,
    pub piece_values_normalized: HashMap<usize, f64>,
}

#[derive(Debug, Clone)]
pub struct PositionStatistics {
    pub total_pieces: u32,
    pub value_per_piece: f64,
    pub value_weighted_average: f64, // Average value weighted by piece count
    pub value_variance: f64,
    pub piece_type_diversity: f64, // Shannon diversity index
    pub position_complexity: f64,  // Composite measure
}
