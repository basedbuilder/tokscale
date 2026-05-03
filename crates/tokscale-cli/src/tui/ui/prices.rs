use chrono::Months;
use ratatui::prelude::*;
use ratatui::symbols::Marker;
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table,
};

use super::widgets::{
    format_cost, format_price_per_million, format_tokens, get_provider_display_name,
};
use crate::tui::app::{App, SortDirection, SortField};
use crate::tui::data::{PriceSummary, PriceUsage};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Model Prices ",
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

    let prices_len = app.data.prices.len();
    if prices_len == 0 {
        let empty_msg =
            Paragraph::new("No resolved model pricing data found. Press 'r' to refresh.")
                .style(Style::default().fg(app.theme.muted))
                .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    app.selected_index = app.selected_index.min(prices_len.saturating_sub(1));
    let max_scroll = prices_len.saturating_sub(app.max_visible_items);
    app.scroll_offset = app.scroll_offset.min(max_scroll);
    if app.selected_index < app.scroll_offset {
        app.scroll_offset = app.selected_index;
    } else if app.selected_index >= app.scroll_offset + app.max_visible_items {
        app.scroll_offset = app.selected_index.saturating_sub(app.max_visible_items - 1);
    }

    let prices = app.get_sorted_prices();
    let selected_index = app.selected_index;
    let selected_row = prices.get(selected_index).copied().unwrap_or(prices[0]);

    if has_chart_room {
        render_price_chart(frame, app, chunks[0], selected_row);
        render_price_summary(frame, app, chunks[1], selected_row);
    }

    render_price_table(frame, app, table_area, &prices);
}

fn render_price_chart(frame: &mut Frame, app: &App, area: Rect, selected: &PriceSummary) {
    let series_rows = matching_series_rows(app, selected);
    let chart_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Price Graph (3M) ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_top(
            Line::from(Span::styled(
                format!(
                    " {} / {} ",
                    truncate(&selected.model, 28),
                    format_provider_list(&selected.provider)
                ),
                Style::default().fg(app.theme.muted),
            ))
            .right_aligned(),
        )
        .style(Style::default().bg(app.theme.background));

    if series_rows.is_empty() {
        frame.render_widget(chart_block.clone(), area);
        let inner = chart_block.inner(area);
        frame.render_widget(
            Paragraph::new("No comparable price series for this model.")
                .style(Style::default().fg(app.theme.muted))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let input_points: Vec<(f64, f64)> = series_rows
        .iter()
        .enumerate()
        .filter_map(|(idx, row)| row.input_price_per_million.map(|price| (idx as f64, price)))
        .collect();
    let output_points: Vec<(f64, f64)> = series_rows
        .iter()
        .enumerate()
        .filter_map(|(idx, row)| {
            row.output_price_per_million
                .map(|price| (idx as f64, price))
        })
        .collect();
    let cache_read_points: Vec<(f64, f64)> = series_rows
        .iter()
        .enumerate()
        .filter_map(|(idx, row)| {
            row.cache_read_price_per_million
                .map(|price| (idx as f64, price))
        })
        .collect();

    let max_price = input_points
        .iter()
        .chain(output_points.iter())
        .chain(cache_read_points.iter())
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
            .name("Input")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Rgb(100, 200, 100)))
            .data(&input_points),
        Dataset::default()
            .name("Output")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Rgb(200, 100, 100)))
            .data(&output_points),
        Dataset::default()
            .name("Cache R")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Rgb(100, 150, 200)))
            .data(&cache_read_points),
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
                .bounds([0.0, max_price * 1.1])
                .labels(vec![
                    Span::raw("$0"),
                    Span::raw(format!("${:.2}", (max_price * 1.1) / 2.0)),
                    Span::raw(format!("${:.2}", max_price * 1.1)),
                ]),
        );

    frame.render_widget(chart, area);
}

fn render_price_summary(frame: &mut Frame, app: &App, area: Rect, selected: &PriceSummary) {
    let summary = Line::from(vec![
        Span::styled("Selected: ", Style::default().fg(app.theme.muted)),
        Span::styled(
            truncate(&selected.model, 32),
            Style::default()
                .fg(app.model_color_for(&selected.provider, &selected.model))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!(
                "source {}",
                selected.pricing_source.as_deref().unwrap_or("—")
            ),
            Style::default().fg(app.theme.muted),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("cost {}", format_cost(selected.cost)),
            Style::default().fg(Color::Green),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("input {}", format_tokens(selected.tokens.input)),
            Style::default().fg(Color::Rgb(100, 200, 100)),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("output {}", format_tokens(selected.tokens.output)),
            Style::default().fg(Color::Rgb(200, 100, 100)),
        ),
    ]);

    frame.render_widget(Paragraph::new(summary), area);
}

fn render_price_table(frame: &mut Frame, app: &App, area: Rect, prices: &[&PriceSummary]) {
    let visible_height = area.height.saturating_sub(3) as usize;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Unique Model Prices ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let is_narrow = app.is_narrow();
    let is_very_narrow = app.is_very_narrow();
    let sort_field = app.sort_field;
    let sort_direction = app.sort_direction;
    let scroll_offset = app.scroll_offset;
    let selected_index = app.selected_index;
    let theme_accent = app.theme.accent;
    let theme_selection = app.theme.selection;

    let header_cells = if is_very_narrow {
        vec!["Model", "I/O", "Cost"]
    } else if is_narrow {
        vec!["Model", "In/M", "Out/M", "Cost"]
    } else {
        vec![
            "Model",
            "Provider",
            "Source",
            "Input $/M",
            "Output $/M",
            "Cache R/M",
            "Input",
            "Output",
            "Cost",
        ]
    };

    let sort_indicator = |field: SortField| -> &'static str {
        if sort_field == field {
            match sort_direction {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            }
        } else {
            ""
        }
    };

    let header = Row::new(
        header_cells
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let indicator = match (i, is_narrow, is_very_narrow) {
                    (0, _, _) => sort_indicator(SortField::Model),
                    (2, true, true) => sort_indicator(SortField::Cost),
                    (3, true, false) => sort_indicator(SortField::Cost),
                    (8, false, false) => sort_indicator(SortField::Cost),
                    (6, false, false) => sort_indicator(SortField::Tokens),
                    _ => "",
                };
                Cell::from(format!("{label}{indicator}"))
            })
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(theme_accent)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let prices_len = prices.len();
    let start = scroll_offset.min(prices_len);
    let end = (start + visible_height).min(prices_len);

    if start >= prices_len {
        return;
    }

    let rows: Vec<Row> = prices[start..end]
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let idx = i + start;
            let is_selected = idx == selected_index;
            let is_striped = idx % 2 == 1;
            let model_color = app.model_color_for(&row.provider, &row.model);
            let pricing_source = row.pricing_source.as_deref().unwrap_or("—");

            let cells: Vec<Cell> = if is_very_narrow {
                vec![
                    Cell::from(truncate(&row.model, 18)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format!(
                        "{}/{}",
                        format_price_per_million(row.input_price_per_million),
                        format_price_per_million(row.output_price_per_million)
                    ))
                    .style(Style::default().fg(Color::Green)),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else if is_narrow {
                vec![
                    Cell::from(truncate(&row.model, 22)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format_price_per_million(row.input_price_per_million))
                        .style(Style::default().fg(Color::Rgb(100, 200, 100))),
                    Cell::from(format_price_per_million(row.output_price_per_million))
                        .style(Style::default().fg(Color::Rgb(200, 100, 100))),
                    Cell::from(format_cost(row.cost)).style(Style::default().fg(Color::Green)),
                ]
            } else {
                vec![
                    Cell::from(truncate(&row.model, 30)).style(
                        Style::default()
                            .fg(model_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format_provider_list(&row.provider)),
                    Cell::from(pricing_source),
                    Cell::from(format_price_per_million(row.input_price_per_million))
                        .style(Style::default().fg(Color::Rgb(100, 200, 100))),
                    Cell::from(format_price_per_million(row.output_price_per_million))
                        .style(Style::default().fg(Color::Rgb(200, 100, 100))),
                    Cell::from(format_price_per_million(row.cache_read_price_per_million))
                        .style(Style::default().fg(Color::Rgb(100, 150, 200))),
                    Cell::from(format_tokens(row.tokens.input)),
                    Cell::from(format_tokens(row.tokens.output)),
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
            Constraint::Percentage(48),
            Constraint::Percentage(32),
            Constraint::Length(10),
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
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(theme_selection));

    frame.render_widget(table, inner);

    if prices_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = ScrollbarState::new(prices_len).position(scroll_offset);

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

fn matching_series_rows<'a>(app: &'a App, selected: &PriceSummary) -> Vec<&'a PriceUsage> {
    let selected_category = selected.matched_key.as_deref().unwrap_or(&selected.model);
    let trailing_cutoff = selected
        .latest_date
        .checked_sub_months(Months::new(3))
        .unwrap_or(selected.latest_date);

    let mut rows: Vec<&PriceUsage> = app
        .data
        .prices_daily
        .iter()
        .filter(|row| {
            row.matched_key.as_deref().unwrap_or(&row.model) == selected_category
                && row.date >= trailing_cutoff
                && row.date <= selected.latest_date
        })
        .collect();
    rows.sort_by_key(|row| row.date);
    rows
}

fn format_provider_list(provider: &str) -> String {
    provider
        .split(", ")
        .map(get_provider_display_name)
        .collect::<Vec<_>>()
        .join(", ")
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

#[cfg(test)]
mod tests {
    use super::matching_series_rows;
    use crate::tui::app::{App, TuiConfig};
    use crate::tui::data::{PriceSummary, PriceUsage, TokenBreakdown, UsageData};
    use chrono::NaiveDate;
    use std::collections::BTreeSet;

    fn make_price_row(date: &str) -> PriceUsage {
        PriceUsage {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            model: "gpt-5.4".to_string(),
            provider: "openai".to_string(),
            pricing_source: Some("LiteLLM".to_string()),
            matched_key: Some("gpt-5.4".to_string()),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            clients: BTreeSet::new(),
            message_count: 0,
            input_price_per_million: Some(1.0),
            output_price_per_million: Some(2.0),
            cache_read_price_per_million: Some(0.1),
            cache_write_price_per_million: None,
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
        app.data.prices = vec![PriceSummary {
            model: "gpt-5.4".to_string(),
            provider: "openai".to_string(),
            pricing_source: Some("LiteLLM".to_string()),
            matched_key: Some("gpt-5.4".to_string()),
            latest_date: NaiveDate::parse_from_str("2026-04-21", "%Y-%m-%d").unwrap(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            clients: BTreeSet::new(),
            message_count: 0,
            input_price_per_million: Some(1.0),
            output_price_per_million: Some(2.0),
            cache_read_price_per_million: Some(0.1),
            cache_write_price_per_million: None,
        }];
        app.data.prices_daily = vec![
            make_price_row("2026-01-20"),
            make_price_row("2026-01-21"),
            make_price_row("2026-02-21"),
            make_price_row("2026-04-21"),
            make_price_row("2026-04-22"),
        ];

        let selected = &app.data.prices[0];
        let rows = matching_series_rows(&app, selected);
        let dates: Vec<String> = rows.iter().map(|row| row.date.to_string()).collect();

        assert_eq!(dates, vec!["2026-01-21", "2026-02-21", "2026-04-21"]);
    }
}
