use std::collections::BTreeMap;

use chrono::Months;
use ratatui::prelude::*;
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table,
};

use super::widgets::format_cost;
use crate::tui::app::App;
use crate::tui::data::{ThinkingSummary, ThinkingUsage, TokenBreakdown};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Thinking Trends ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let has_chart_room = inner.height >= 14;
    let chart_height = if has_chart_room { 8 } else { 0 };
    let summary_height = if has_chart_room { 2 } else { 0 };

    let chunks = if has_chart_room {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(chart_height),
                Constraint::Length(summary_height),
                Constraint::Min(0),
            ])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(inner)
    };

    let table_area = *chunks.last().unwrap_or(&inner);
    let visible_height = table_area.height.saturating_sub(3) as usize;
    app.max_visible_items = visible_height.max(1);

    let thinking_len = app.data.thinking.len();
    if thinking_len == 0 {
        let empty_msg = Paragraph::new(
            "No output/reasoning trend data found. Trends are based on thinking share over the latest 30d.",
        )
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    app.selected_index = app.selected_index.min(thinking_len.saturating_sub(1));
    let max_scroll = thinking_len.saturating_sub(app.max_visible_items);
    app.scroll_offset = app.scroll_offset.min(max_scroll);
    if app.selected_index < app.scroll_offset {
        app.scroll_offset = app.selected_index;
    } else if app.selected_index >= app.scroll_offset + app.max_visible_items {
        app.scroll_offset = app.selected_index.saturating_sub(app.max_visible_items - 1);
    }

    let thinking = app.get_sorted_thinking();
    let selected_index = app.selected_index;
    let selected_row = thinking.get(selected_index).copied().unwrap_or(thinking[0]);

    if has_chart_room {
        render_thinking_chart(frame, app, chunks[0], selected_row);
        render_thinking_summary(frame, app, chunks[1], selected_row);
    }

    render_thinking_table(frame, app, table_area, &thinking);
}

fn render_thinking_chart(frame: &mut Frame, app: &App, area: Rect, selected: &ThinkingSummary) {
    let series_rows = matching_series_rows(app, &selected.model);
    let chart_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Relative Mix (3M) ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(" {} ", truncate(&selected.model, 28)),
                Style::default().fg(app.theme.muted),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));

    if series_rows.is_empty() {
        frame.render_widget(chart_block.clone(), area);
        let inner = chart_block.inner(area);
        frame.render_widget(
            Paragraph::new("No comparable trend series for this model.")
                .style(Style::default().fg(app.theme.muted))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let output_points: Vec<(f64, f64)> = series_rows
        .iter()
        .enumerate()
        .map(|(idx, row)| (idx as f64, output_per_million_generated(row)))
        .collect();
    let reasoning_points: Vec<(f64, f64)> = series_rows
        .iter()
        .enumerate()
        .map(|(idx, row)| (idx as f64, thinking_per_million_generated(row)))
        .collect();

    let max_tokens = output_points
        .iter()
        .chain(reasoning_points.iter())
        .map(|(_, value)| *value)
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let x_max = (series_rows.len().saturating_sub(1)).max(1) as f64;
    let first_label = series_rows
        .first()
        .map(|row| row.date.format("%m/%d").to_string())
        .unwrap_or_else(|| "—".to_string());
    let mid_idx = series_rows.len() / 2;
    let mid_label = series_rows
        .get(mid_idx)
        .map(|row| row.date.format("%m/%d").to_string())
        .unwrap_or_else(|| "—".to_string());
    let last_label = series_rows
        .last()
        .map(|row| row.date.format("%m/%d").to_string())
        .unwrap_or_else(|| "—".to_string());

    let datasets = vec![
        Dataset::default()
            .name("Output/M")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Rgb(200, 100, 100)))
            .data(&output_points),
        Dataset::default()
            .name("Think/M")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Rgb(100, 150, 240)))
            .data(&reasoning_points),
    ];

    let chart = Chart::new(datasets)
        .block(chart_block)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(app.theme.muted))
                .bounds([0.0, x_max])
                .labels(vec![
                    Span::raw(first_label),
                    Span::raw(mid_label),
                    Span::raw(last_label),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(app.theme.muted))
                .bounds([0.0, max_tokens * 1.1])
                .labels(vec![
                    Span::raw("0/M"),
                    Span::raw(format_rate_per_million((max_tokens * 1.1) / 2.0)),
                    Span::raw(format_rate_per_million(max_tokens * 1.1)),
                ]),
        );

    frame.render_widget(chart, area);
}

fn render_thinking_summary(frame: &mut Frame, app: &App, area: Rect, selected: &ThinkingSummary) {
    let thinking_rate = thinking_per_million_generated_from_tokens(&selected.tokens);
    let thinking_share = thinking_share_percent_from_tokens(&selected.tokens);

    let summary = Line::from(vec![
        Span::styled("Selected: ", Style::default().fg(app.theme.muted)),
        Span::styled(
            truncate(&selected.model, 28),
            Style::default()
                .fg(app.model_color_for("mixed", &selected.model))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("output {}", format_tokens(selected.tokens.output)),
            Style::default().fg(Color::Rgb(200, 100, 100)),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("thinking {}", format_tokens(selected.tokens.reasoning)),
            Style::default().fg(Color::Rgb(100, 150, 240)),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("think rate {}", format_rate_per_million(thinking_rate)),
            Style::default().fg(Color::Rgb(100, 150, 240)),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("share {:.1}%", thinking_share),
            Style::default().fg(app.theme.muted),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!(
                "30d {}",
                format_trend_percent(selected.thirty_day_trend_pct)
            ),
            Style::default().fg(trend_color(selected.thirty_day_trend_pct)),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("cost {}", format_cost(selected.cost)),
            Style::default().fg(Color::Green),
        ),
    ]);

    frame.render_widget(Paragraph::new(summary), area);
}

fn render_thinking_table(frame: &mut Frame, app: &App, area: Rect, thinking: &[&ThinkingSummary]) {
    let visible_height = area.height.saturating_sub(3) as usize;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Model Thinking Summary ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let is_narrow = app.is_narrow();
    let is_very_narrow = app.is_very_narrow();
    let scroll_offset = app.scroll_offset;
    let selected_index = app.selected_index;
    let theme_accent = app.theme.accent;
    let theme_selection = app.theme.selection;

    let header_cells = if is_very_narrow {
        vec!["Model", "T/M", "30d"]
    } else if is_narrow {
        vec!["Model", "Think%", "30d", "Cost"]
    } else {
        vec![
            "Model",
            "Think/M Gen",
            "Think %",
            "30d Trend",
            "Output",
            "Thinking",
            "Cost",
        ]
    };

    let header = Row::new(
        header_cells
            .iter()
            .map(|label| Cell::from(*label))
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(theme_accent)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let thinking_len = thinking.len();
    let start = scroll_offset.min(thinking_len);
    let end = (start + visible_height).min(thinking_len);

    if start >= thinking_len {
        return;
    }

    let rows: Vec<Row> = thinking[start..end]
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let idx = i + start;
            let is_selected = idx == selected_index;
            let is_striped = idx % 2 == 1;
            let model_color = app.model_color_for("mixed", &row.model);
            let thinking_rate = thinking_per_million_generated_from_tokens(&row.tokens);
            let thinking_share = thinking_share_percent_from_tokens(&row.tokens);
            let trend_text = format_trend_percent(row.thirty_day_trend_pct);
            let trend_style = Style::default().fg(trend_color(row.thirty_day_trend_pct));

            let cells: Vec<Cell> = if is_very_narrow {
                vec![
                    Cell::from(truncate(&row.model, 18)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format_rate_per_million(thinking_rate))
                        .style(Style::default().fg(Color::Rgb(100, 150, 240))),
                    Cell::from(trend_text).style(trend_style),
                ]
            } else if is_narrow {
                vec![
                    Cell::from(truncate(&row.model, 20)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format!("{thinking_share:.1}%"))
                        .style(Style::default().fg(app.theme.muted)),
                    Cell::from(trend_text).style(trend_style),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else {
                vec![
                    Cell::from(truncate(&row.model, 28)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format_rate_per_million(thinking_rate))
                        .style(Style::default().fg(Color::Rgb(100, 150, 240))),
                    Cell::from(format!("{thinking_share:.1}%"))
                        .style(Style::default().fg(app.theme.muted)),
                    Cell::from(trend_text).style(trend_style),
                    Cell::from(format_tokens(row.tokens.output))
                        .style(Style::default().fg(Color::Rgb(200, 100, 100))),
                    Cell::from(format_tokens(row.tokens.reasoning))
                        .style(Style::default().fg(Color::Rgb(100, 150, 240))),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            };

            let row_style = if is_selected {
                Style::default().bg(theme_selection)
            } else if is_striped {
                Style::default().bg(Color::Rgb(20, 24, 30))
            } else {
                Style::default()
            };

            Row::new(cells).style(row_style).height(1)
        })
        .collect();

    let widths = if is_very_narrow {
        vec![
            Constraint::Percentage(52),
            Constraint::Length(10),
            Constraint::Length(8),
        ]
    } else if is_narrow {
        vec![
            Constraint::Percentage(42),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    } else {
        vec![
            Constraint::Min(20),
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(theme_selection));

    frame.render_widget(table, inner);

    if thinking_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = ScrollbarState::new(thinking_len).position(scroll_offset);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut scrollbar_state,
        );
    }
}

fn matching_series_rows(app: &App, model: &str) -> Vec<ThinkingUsage> {
    let mut daily: BTreeMap<chrono::NaiveDate, ThinkingUsage> = BTreeMap::new();

    for row in app
        .data
        .thinking_daily
        .iter()
        .filter(|row| row.model == model)
    {
        let entry = daily.entry(row.date).or_insert_with(|| ThinkingUsage {
            date: row.date,
            model: row.model.clone(),
            provider: String::new(),
            thinking_level: "All".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            clients: Default::default(),
            message_count: 0,
        });
        entry.tokens.input = entry.tokens.input.saturating_add(row.tokens.input);
        entry.tokens.output = entry.tokens.output.saturating_add(row.tokens.output);
        entry.tokens.cache_read = entry
            .tokens
            .cache_read
            .saturating_add(row.tokens.cache_read);
        entry.tokens.cache_write = entry
            .tokens
            .cache_write
            .saturating_add(row.tokens.cache_write);
        entry.tokens.reasoning = entry.tokens.reasoning.saturating_add(row.tokens.reasoning);
        entry.cost += row.cost;
        entry.clients.extend(row.clients.iter().cloned());
        entry.message_count = entry.message_count.saturating_add(row.message_count);
    }

    let mut rows: Vec<ThinkingUsage> = daily.into_values().collect();
    let Some(latest_date) = rows.last().map(|row| row.date) else {
        return rows;
    };
    let trailing_cutoff = latest_date
        .checked_sub_months(Months::new(3))
        .unwrap_or(latest_date);
    rows.retain(|row| row.date >= trailing_cutoff && row.date <= latest_date);
    rows
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 1 {
        "…".to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}

fn generated_tokens_from_tokens(tokens: &TokenBreakdown) -> u64 {
    tokens.output.saturating_add(tokens.reasoning)
}

fn thinking_per_million_generated(row: &ThinkingUsage) -> f64 {
    thinking_per_million_generated_from_tokens(&row.tokens)
}

fn thinking_per_million_generated_from_tokens(tokens: &TokenBreakdown) -> f64 {
    let generated = generated_tokens_from_tokens(tokens);
    if generated == 0 {
        0.0
    } else {
        (tokens.reasoning as f64 / generated as f64) * 1_000_000.0
    }
}

fn output_per_million_generated(row: &ThinkingUsage) -> f64 {
    let generated = generated_tokens_from_tokens(&row.tokens);
    if generated == 0 {
        0.0
    } else {
        (row.tokens.output as f64 / generated as f64) * 1_000_000.0
    }
}

fn thinking_share_percent_from_tokens(tokens: &TokenBreakdown) -> f64 {
    thinking_per_million_generated_from_tokens(tokens) / 10_000.0
}

fn format_rate_per_million(rate: f64) -> String {
    if rate >= 100_000.0 {
        format!("{rate:.0}/M")
    } else if rate >= 10_000.0 {
        format!("{:.1}k/M", rate / 1_000.0)
    } else {
        format!("{rate:.0}/M")
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.1}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn format_trend_percent(trend: Option<f64>) -> String {
    trend
        .map(|value| format!("{value:+.1}%"))
        .unwrap_or_else(|| "—".to_string())
}

fn trend_color(trend: Option<f64>) -> Color {
    match trend {
        Some(value) if value > 0.5 => Color::Green,
        Some(value) if value < -0.5 => Color::Red,
        Some(_) => Color::Yellow,
        None => Color::DarkGray,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_trend_percent, matching_series_rows, thinking_per_million_generated_from_tokens,
        thinking_share_percent_from_tokens,
    };
    use crate::tui::app::{App, TuiConfig};
    use crate::tui::data::{ThinkingSummary, ThinkingUsage, TokenBreakdown, UsageData};
    use chrono::NaiveDate;
    use std::collections::BTreeSet;

    fn make_row(date: &str) -> ThinkingUsage {
        ThinkingUsage {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            model: "claude-sonnet-4-5".to_string(),
            provider: "anthropic".to_string(),
            thinking_level: "Thinking High".to_string(),
            tokens: TokenBreakdown {
                input: 0,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 5,
            },
            cost: 0.0,
            clients: BTreeSet::new(),
            message_count: 1,
        }
    }

    #[test]
    fn matching_series_rows_limits_to_trailing_three_months() {
        let config = TuiConfig {
            theme: "blue".to_string(),
            refresh: 0,
            sessions_path: None,
            clients: None,
            since: None,
            until: None,
            year: None,
            initial_tab: None,
        };
        let mut app = App::new_with_cached_data(config, Some(UsageData::default())).unwrap();
        app.data.thinking = vec![ThinkingSummary {
            model: "claude-sonnet-4-5".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            clients: BTreeSet::new(),
            message_count: 1,
            thirty_day_trend_pct: Some(0.0),
        }];
        app.data.thinking_daily = vec![
            make_row("2026-01-20"),
            make_row("2026-01-21"),
            make_row("2026-02-21"),
            make_row("2026-04-21"),
            make_row("2026-04-22"),
        ];

        let rows = matching_series_rows(&app, "claude-sonnet-4-5");
        let dates: Vec<String> = rows.iter().map(|row| row.date.to_string()).collect();

        assert_eq!(dates, vec!["2026-02-21", "2026-04-21", "2026-04-22"]);
    }

    #[test]
    fn thinking_rate_normalizes_by_generated_tokens() {
        let tokens = TokenBreakdown {
            input: 0,
            output: 800,
            cache_read: 0,
            cache_write: 0,
            reasoning: 200,
        };

        assert!((thinking_per_million_generated_from_tokens(&tokens) - 200_000.0).abs() < 0.001);
        assert!((thinking_share_percent_from_tokens(&tokens) - 20.0).abs() < 0.001);
    }

    #[test]
    fn trend_format_uses_em_dash_for_missing_values() {
        assert_eq!(format_trend_percent(None), "—");
        assert_eq!(format_trend_percent(Some(12.34)), "+12.3%");
        assert_eq!(format_trend_percent(Some(-7.89)), "-7.9%");
    }
}
