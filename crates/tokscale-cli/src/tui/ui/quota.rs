use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use super::widgets::{format_cost, format_tokens};
use crate::tui::app::App;
use crate::tui::data::{QuotaConfidence, QuotaModelSummary};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Model ROI ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.data.quota_value.sample_count == 0 {
        render_empty(
            frame,
            app,
            inner,
            "No Codex quota observations found. Refresh after a Codex response with rate-limit telemetry.",
        );
        return;
    }
    if app.data.quota_value.model_summaries.is_empty() {
        render_empty(
            frame,
            app,
            inner,
            "Quota samples found, but no model value estimate yet.",
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(inner);

    render_summary(frame, app, chunks[0]);
    render_graph(frame, app, chunks[1]);
    render_table(frame, app, chunks[2]);
}

fn render_empty(frame: &mut Frame, app: &App, area: Rect, message: &str) {
    let paragraph = Paragraph::new(message)
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn render_summary(frame: &mut Frame, app: &App, area: Rect) {
    let top = app.data.quota_value.model_summaries.iter().max_by(|a, b| {
        a.factor
            .unwrap_or(0.0)
            .total_cmp(&b.factor.unwrap_or(0.0))
            .then_with(|| a.api_value_usd.total_cmp(&b.api_value_usd))
    });
    let text = if let Some(row) = top {
        format!(
            "Best ROI {} {}   API {}   Est sub {}   Burn {:.1}%   Obs {} {}",
            row.model,
            format_factor(row.factor),
            format_cost(row.api_value_usd),
            format_cost(row.subscription_cost_burned),
            row.quota_burn_pct,
            row.interval_count,
            row.confidence.as_str()
        )
    } else {
        format!("Samples {}", app.data.quota_value.sample_count)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(app.theme.foreground)),
        area,
    );
}

fn render_graph(frame: &mut Frame, app: &App, area: Rect) {
    let mut summaries = app
        .data
        .quota_value
        .model_summaries
        .iter()
        .collect::<Vec<_>>();
    summaries.sort_by(|a, b| {
        b.factor
            .unwrap_or(0.0)
            .total_cmp(&a.factor.unwrap_or(0.0))
            .then_with(|| b.api_value_usd.total_cmp(&a.api_value_usd))
            .then_with(|| a.model.cmp(&b.model))
    });
    let max_factor = summaries
        .iter()
        .filter_map(|row| row.factor)
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let row_count = area.height.min(4) as usize;
    if row_count == 0 {
        return;
    }
    let row_constraints = vec![Constraint::Length(1); row_count];
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    for (summary, row_area) in summaries.into_iter().take(row_count).zip(rows.iter()) {
        render_factor_bar(frame, app, summary, *row_area, max_factor);
    }
}

fn render_factor_bar(
    frame: &mut Frame,
    app: &App,
    summary: &QuotaModelSummary,
    area: Rect,
    max_factor: f64,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(10)])
        .split(area);
    let label = format!(
        "{} {} {}",
        compact_model(&summary.model, 12),
        format_factor(summary.factor),
        format_cost(summary.api_value_usd)
    );
    let bar_width = chunks[1].width.max(1) as usize;
    let filled = summary
        .factor
        .map(|factor| ((factor / max_factor).clamp(0.0, 1.0) * bar_width as f64) as usize)
        .unwrap_or(0)
        .min(bar_width);
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(bar_width - filled));
    frame.render_widget(
        Paragraph::new(label).style(Style::default().fg(app.theme.muted)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(bar).style(Style::default().fg(app.model_color(&summary.model))),
        chunks[1],
    );
}

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_height = area.height.saturating_sub(1) as usize;
    app.max_visible_items = visible_height;
    let rows = app.get_sorted_quota_model_summaries();
    let start = app.scroll_offset.min(rows.len().saturating_sub(1));
    let end = (start + visible_height).min(rows.len());

    let header = Row::new([
        "Model",
        "API Value",
        "Est Sub",
        "ROI",
        "Burn",
        "$/1%",
        "Tokens",
        "Obs",
        "Window",
        "Confidence",
    ])
    .style(
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
    );

    let table_rows = rows[start..end]
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            let idx = start + offset;
            let style = if idx == app.selected_index {
                Style::default().bg(app.theme.selection)
            } else if row.confidence == QuotaConfidence::Low {
                Style::default().fg(app.theme.muted)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(row.model.clone()),
                Cell::from(format_cost(row.api_value_usd)),
                Cell::from(format_cost(row.subscription_cost_burned)),
                Cell::from(format_factor(row.factor)),
                Cell::from(format!("{:.1}%", row.quota_burn_pct)),
                Cell::from(
                    row.dollars_per_percent
                        .map(format_cost)
                        .unwrap_or_else(|| "-".to_string()),
                ),
                Cell::from(format_tokens(row.tokens.total())),
                Cell::from(row.interval_count.to_string()),
                Cell::from(row.window_kind.clone()),
                Cell::from(row.confidence.as_str()),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();

    let widths = vec![
        Constraint::Length(18),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Min(10),
    ];
    frame.render_widget(Table::new(table_rows, widths).header(header), area);
}

fn format_factor(factor: Option<f64>) -> String {
    factor
        .map(|value| format!("{value:.1}x"))
        .unwrap_or_else(|| "-".to_string())
}

fn compact_model(model: &str, max_len: usize) -> String {
    if model.chars().count() <= max_len {
        model.to_string()
    } else {
        let prefix = model
            .chars()
            .take(max_len.saturating_sub(3))
            .collect::<String>();
        format!("{prefix}...")
    }
}
