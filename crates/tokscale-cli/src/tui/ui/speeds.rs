use ratatui::prelude::*;
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Table,
};

use super::widgets::format_tokens;
use crate::tui::app::App;
use crate::tui::data::SpeedSummary;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Generation Speed Totals (30-1000 tok/s) ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.data.speeds.is_empty() {
        frame.render_widget(
            Paragraph::new("No sane generation speed samples found. TPS needs timestamped Codex model-call boundaries.")
                .style(Style::default().fg(app.theme.muted))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let has_chart_room = inner.height >= 13;
    let chunks = if has_chart_room {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(2),
                Constraint::Min(0),
            ])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(inner)
    };

    app.selected_index = app
        .selected_index
        .min(app.data.speeds.len().saturating_sub(1));
    let selected = {
        let rows = app.get_sorted_speeds();
        rows.get(app.selected_index)
            .copied()
            .unwrap_or(rows[0])
            .clone()
    };

    if has_chart_room {
        render_chart(frame, app, chunks[0], &selected);
        render_summary(frame, app, chunks[1], &selected);
    }
    render_table(frame, app, *chunks.last().unwrap_or(&inner));
}

fn render_chart(frame: &mut Frame, app: &App, area: Rect, selected: &SpeedSummary) {
    let mut points = app
        .data
        .speeds_daily
        .iter()
        .filter(|row| {
            row.model == selected.model
                && row.provider == selected.provider
                && row.thinking_level == selected.thinking_level
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|row| row.date);

    let chart_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(" Daily Average TPS for Selected Total (Sane Samples) ")
        .title_top(
            Line::from(Span::styled(
                format!(
                    " {} / {} ",
                    compact(&selected.model, 24),
                    selected.thinking_level
                ),
                Style::default().fg(app.theme.muted),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));

    if points.is_empty() {
        frame.render_widget(chart_block, area);
        return;
    }

    let series = points
        .iter()
        .enumerate()
        .map(|(idx, row)| (idx as f64, row.tokens_per_second()))
        .collect::<Vec<_>>();
    let max_tps = series
        .iter()
        .map(|(_, value)| *value)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let x_max = (series.len().saturating_sub(1)).max(1) as f64;
    let first = points
        .first()
        .map(|row| row.date.format("%m/%d").to_string())
        .unwrap_or_default();
    let mid = points
        .get(points.len() / 2)
        .map(|row| row.date.format("%m/%d").to_string())
        .unwrap_or_default();
    let last = points
        .last()
        .map(|row| row.date.format("%m/%d").to_string())
        .unwrap_or_default();
    let datasets = vec![Dataset::default()
        .name("tok/s")
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(app.model_color(&selected.model)))
        .data(&series)];

    let chart = Chart::new(datasets)
        .block(chart_block)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(app.theme.muted))
                .bounds([0.0, x_max])
                .labels(vec![Span::raw(first), Span::raw(mid), Span::raw(last)]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(app.theme.muted))
                .bounds([0.0, max_tps * 1.1])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format!("{:.0}", (max_tps * 1.1) / 2.0)),
                    Span::raw(format!("{:.0}", max_tps * 1.1)),
                ]),
        );
    frame.render_widget(chart, area);
}

fn render_summary(frame: &mut Frame, app: &App, area: Rect, selected: &SpeedSummary) {
    let line = Line::from(vec![
        Span::styled("Selected: ", Style::default().fg(app.theme.muted)),
        Span::styled(
            compact(&selected.model, 28),
            Style::default()
                .fg(app.model_color(&selected.model))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{:.1} tok/s", selected.tokens_per_second()),
            Style::default().fg(app.theme.accent),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} total generated",
                format_tokens(selected.generated_tokens)
            ),
            Style::default().fg(app.theme.muted),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} sane samples", selected.sample_count),
            Style::default().fg(app.theme.muted),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height.saturating_sub(1) as usize;
    app.max_visible_items = visible_height.max(1);
    let row_len = app.data.speeds.len();
    let max_scroll = row_len.saturating_sub(app.max_visible_items);
    app.scroll_offset = app.scroll_offset.min(max_scroll);
    if app.selected_index < app.scroll_offset {
        app.scroll_offset = app.selected_index;
    } else if app.selected_index >= app.scroll_offset + app.max_visible_items {
        app.scroll_offset = app.selected_index.saturating_sub(app.max_visible_items - 1);
    }

    let rows = app.get_sorted_speeds();
    let start = app.scroll_offset.min(rows.len().saturating_sub(1));
    let end = (start + app.max_visible_items).min(rows.len());
    let table_rows = rows[start..end]
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            let idx = start + offset;
            let style = if idx == app.selected_index {
                Style::default().bg(app.theme.selection)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(row.model.clone()),
                Cell::from(row.thinking_level.clone()),
                Cell::from(row.provider.clone()),
                Cell::from(format!("{:.1}", row.tokens_per_second())),
                Cell::from(format_tokens(row.generated_tokens)),
                Cell::from(format!(
                    "{:.1}s",
                    row.generation_duration_ms as f64 / 1000.0
                )),
                Cell::from(row.sample_count.to_string()),
                Cell::from(row.latest_date.to_string()),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(24),
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new([
            "Model",
            "Thinking",
            "Provider",
            "Tok/s",
            "Generated",
            "Total Gen",
            "Samples",
            "Latest",
        ])
        .style(
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .column_spacing(1);
    frame.render_widget(table, area);
}

fn compact(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(keep).collect::<String>())
}
