use crate::app::ChessGui;
use crate::engine::analysis::PositionAnalysis;
use crate::notation::position_to_algebraic;
use crate::clog;

impl ChessGui {
    pub(crate) fn cmd_analyze(&mut self) {
        clog!("🔬 Analyzing position...");
        self.handle_analyze_position();
    }

    pub(crate) fn print_comprehensive_analysis(&self, analysis: &PositionAnalysis) {
        clog!("\n🔍 MATERIAL ANALYSIS:");
        clog!("  White total: {:.2}", analysis.material_values.white_total);
        clog!("  Black total: {:.2}", analysis.material_values.black_total);
        clog!(
            "  Material difference: {:.2} (+ = White advantage)",
            analysis.material_values.difference
        );

        clog!("\n📊 PIECE STATISTICS:");
        for (&piece_type, &avg_value) in &analysis.material_values.piece_values {
            if let Some(pc) = self.piece_config.get_piece_by_index(piece_type) {
                let (wc, bc) = analysis
                    .material_values
                    .piece_counts
                    .get(&piece_type)
                    .unwrap_or(&(0, 0));
                clog!(
                    "  {}: Avg value {:.2}, Count W:{} B:{}",
                    pc.display_name,
                    avg_value,
                    wc,
                    bc
                );
            }
        }

        if let Some(ref pst) = analysis.pst_analysis {
            clog!("\n🎯 PIECE SQUARE TABLE ANALYSIS:");
            clog!("  White PST total: {:.2}", pst.white_pst_total);
            clog!("  Black PST total: {:.2}", pst.black_pst_total);
            clog!("  PST difference: {:.2}", pst.pst_difference);

            clog!("\n📈 PST STATISTICS BY PIECE:");
            for (piece_type, stats) in &pst.piece_pst_stats {
                if let Some(pc) = self.piece_config.get_piece_by_index(*piece_type) {
                    clog!("  {}:", pc.display_name);
                    clog!(
                        "    Value range: {:.2} to {:.2} (σ={:.2})",
                        stats.min_value,
                        stats.max_value,
                        stats.standard_deviation
                    );
                    if stats.current_count > 0 {
                        clog!(
                            "    Current: {} pieces, avg {:.2}, total {:.2}",
                            stats.current_count,
                            stats.current_average,
                            stats.current_total
                        );
                    }
                }
            }

            let bias = &pst.variance_analysis.positional_bias;
            clog!("\n🧭 POSITIONAL BIAS ANALYSIS:");
            clog!("  Forward bias: {:.3}", bias.forward_bias);
            clog!("  Center bias: {:.3}", bias.center_bias);
            clog!("  Edge bias: {:.3}", bias.edge_bias);
            clog!("  Left/right bias: {:.3}", bias.left_right_bias);

            let dist = &pst.variance_analysis.value_distribution;
            clog!("\n🔥 VALUE DISTRIBUTION:");
            clog!("  Value range: {:.2}", dist.value_range);
            clog!("  Value variance: {:.2}", dist.value_variance);
            clog!("  Highest value squares:");
            for (pos, value) in dist.highest_value_squares.iter().take(3) {
                clog!(
                    "    {} = {:.2}",
                    position_to_algebraic(*pos, self.game_state.board.size()),
                    value
                );
            }

            let swarm = &pst.swarm_factors;
            clog!("\n⚡ SWARM & TACTICAL FACTORS:");
            clog!("  Average swarm bonus: {:.3}", swarm.average_swarm_bonus);
            clog!("  Max swarm effectiveness: {:.3}", swarm.swarm_effectiveness);
            clog!("  Huddle factor: {:.3}", swarm.huddle_factor);
            if let Some((pos, value)) = swarm.max_swarm_position {
                clog!(
                    "  Best attack square: {} (bonus: {:.2})",
                    position_to_algebraic(pos, self.game_state.board.size()),
                    value
                );
            }
        }

        let mob = &analysis.mobility_analysis;
        clog!("\n🏃 MOBILITY ANALYSIS:");
        clog!("  White mobility: {} moves", mob.white_mobility);
        clog!("  Black mobility: {} moves", mob.black_mobility);
        clog!("  Mobility difference: {}", mob.mobility_difference);
        clog!("  Value to mobility ratio: {:.2}", mob.value_to_mobility_ratio);

        clog!("\n🎲 MOBILITY BY PIECE TYPE:");
        for (piece_type, stats) in &mob.piece_mobility {
            if let Some(pc) = self.piece_config.get_piece_by_index(*piece_type) {
                clog!(
                    "  {}: {} total, {:.1} avg ({} attacking, {} non-attacking)",
                    pc.display_name,
                    stats.total_moves,
                    stats.average_mobility,
                    stats.attacking_moves,
                    stats.non_attacking_moves
                );
            }
        }

        clog!("\n🔬 THEORETICAL MOBILITY:");
        for (piece_type, stats) in &mob.theoretical_mobility {
            if let Some(pc) = self.piece_config.get_piece_by_index(*piece_type) {
                clog!("  {}:", pc.display_name);
                clog!(
                    "    Center: {:.1}, Edge: {:.1}, Corner: {:.1}",
                    stats.center_mobility,
                    stats.edge_mobility,
                    stats.corner_mobility
                );
                clog!(
                    "    Range: {} to {} moves",
                    stats.min_mobility,
                    stats.max_mobility
                );
                clog!("    Concentration: {:.3}", stats.concentration_factor);
                clog!("    Positional variance: {:.2}", stats.mobility_variance);
            }
        }

        let threats = &mob.threat_analysis;
        clog!("\n⚔️ THREAT ANALYSIS:");
        clog!(
            "  White threatens: {:.1} value ({:.1}% captures)",
            threats.white_threats_value,
            threats.white_capture_mobility_percentage
        );
        clog!(
            "  Black threatens: {:.1} value ({:.1}% captures)",
            threats.black_threats_value,
            threats.black_capture_mobility_percentage
        );
        clog!(
            "  Threat balance - White: {:.1}, Black: {:.1}",
            threats.white_threat_balance,
            threats.black_threat_balance
        );

        let dens = &analysis.density_analysis;
        clog!("\n🏗️ DENSITY & CLUSTERING:");
        clog!("  Board density: {:.1}%", dens.board_density * 100.0);
        clog!("  Value per density: {:.2}", dens.piece_density_ratio);
        clog!(
            "  Average piece distance: {:.2}",
            dens.clustering.average_piece_distance
        );
        clog!(
            "  Clustering coefficient: {:.2}",
            dens.clustering.clustering_coefficient
        );
        clog!("  Isolated pieces: {}", dens.clustering.isolated_pieces.len());
        clog!("  Dense regions: {}", dens.clustering.dense_regions.len());

        let stats = &analysis.statistical_analysis;
        clog!("\n📊 STATISTICAL SUMMARY:");
        clog!("  Total pieces: {}", stats.statistics.total_pieces);
        clog!("  Avg value per piece: {:.2}", stats.statistics.value_per_piece);
        clog!(
            "  Value-weighted average: {:.2}",
            stats.statistics.value_weighted_average
        );
        clog!("  Piece type diversity: {:.2}", stats.statistics.piece_type_diversity);
        clog!("  Position complexity: {:.2}", stats.statistics.position_complexity);

        clog!("\n💰 NORMALIZED VALUES:");
        clog!(
            "  White total: {:.1}x",
            stats.normalized_values.white_total_normalized
        );
        clog!(
            "  Black total: {:.1}x",
            stats.normalized_values.black_total_normalized
        );
        for (piece_type, &nv) in &stats.normalized_values.piece_values_normalized {
            if let Some(pc) = self.piece_config.get_piece_by_index(*piece_type) {
                clog!("  {}: {:.1}x", pc.display_name, nv);
            }
        }
        clog!("\n=== Analysis Complete ===");
    }
}
