use crate::app::ChessGui;
use crate::engine::analysis::PositionAnalysis;
use crate::messages::Message;
use crate::ui::etext;
use iced::widget::{column, container, text};
use iced::{Element, Length};

impl ChessGui {
    pub(crate) fn create_analysis_display(
        &self,
        analysis: &PositionAnalysis,
    ) -> Element<'_, Message> {
        let mut content = column![etext("🔬 Position Analysis").size(16),].spacing(5);

        content = content.push(
            text(format!(
                "Material: W:{:.1} B:{:.1} Diff:{:.1}",
                analysis.material_values.white_total,
                analysis.material_values.black_total,
                analysis.material_values.difference
            ))
            .size(12),
        );

        if let Some(ref pst) = analysis.pst_analysis {
            content = content.push(
                text(format!(
                    "PST Values: W:{:.1} B:{:.1} Diff:{:.1}",
                    pst.white_pst_total, pst.black_pst_total, pst.pst_difference
                ))
                .size(12),
            );
            let bias = &pst.variance_analysis.positional_bias;
            content = content.push(
                text(format!(
                    "Bias - Forward:{:.2} Center:{:.2} Edge:{:.2}",
                    bias.forward_bias, bias.center_bias, bias.edge_bias
                ))
                .size(11),
            );
            content = content.push(
                text(format!(
                    "Swarm: Avg:{:.3} Max:{:.3} Huddle:{:.3}",
                    pst.swarm_factors.average_swarm_bonus,
                    pst.swarm_factors.swarm_effectiveness,
                    pst.swarm_factors.huddle_factor
                ))
                .size(11),
            );
        }

        let mob = &analysis.mobility_analysis;
        content = content.push(
            text(format!(
                "Mobility: W:{} B:{} Diff:{} Ratio:{:.2}",
                mob.white_mobility,
                mob.black_mobility,
                mob.mobility_difference,
                mob.value_to_mobility_ratio
            ))
            .size(12),
        );

        let threats = &mob.threat_analysis;
        content = content.push(
            text(format!(
                "Threats: W:{:.1}v ({:.0}% capt) B:{:.1}v ({:.0}% capt)",
                threats.white_threats_value,
                threats.white_capture_mobility_percentage,
                threats.black_threats_value,
                threats.black_capture_mobility_percentage
            ))
            .size(11),
        );

        let dens = &analysis.density_analysis;
        content = content.push(
            text(format!(
                "Density: {:.1}% Clustering:{:.2} Isolated:{}",
                dens.board_density * 100.0,
                dens.clustering.clustering_coefficient,
                dens.clustering.isolated_pieces.len()
            ))
            .size(12),
        );

        let stats = &analysis.statistical_analysis;
        content = content.push(
            text(format!(
                "Stats: {} pieces, {:.1} avg, {:.1} weighted, {:.1}x complexity",
                stats.statistics.total_pieces,
                stats.statistics.value_per_piece,
                stats.statistics.value_weighted_average,
                stats.statistics.position_complexity
            ))
            .size(12),
        );

        let nv = &stats.normalized_values;
        content = content.push(
            text(format!(
                "Normalized (vs weakest {:.2}): W:{:.1}x B:{:.1}x",
                nv.weakest_piece_value, nv.white_total_normalized, nv.black_total_normalized
            ))
            .size(11),
        );

        content = content.push(etext("📊 Piece Analysis:").size(12));
        for (&piece_type, &avg_value) in analysis.material_values.piece_values.iter().take(6) {
            if let Some(pc) = self.piece_config.get_piece_by_index(piece_type) {
                let (wc, bc) = analysis
                    .material_values
                    .piece_counts
                    .get(&piece_type)
                    .unwrap_or(&(0, 0));
                let mobility_info = analysis
                    .mobility_analysis
                    .theoretical_mobility
                    .get(&piece_type)
                    .map(|t| {
                        format!(
                            " [C:{:.1} E:{:.1} σ:{:.2}]",
                            t.center_mobility, t.edge_mobility, t.concentration_factor
                        )
                    })
                    .unwrap_or_default();
                content = content.push(
                    text(format!(
                        "  {}: {:.1}v (W:{} B:{}){}",
                        pc.display_name, avg_value, wc, bc, mobility_info
                    ))
                    .size(10),
                );
            }
        }

        container(content)
            .padding(10)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                    248, 248, 255,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgb8(200, 200, 220),
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }
}
