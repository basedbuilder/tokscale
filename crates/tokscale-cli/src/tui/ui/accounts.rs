use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
};

use super::widgets::{format_cost, format_tokens};
use crate::tui::app::{format_codex_account_label, App, SortDirection, SortField};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(Span::styled(
            " Accounts ",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height.saturating_sub(1) as usize;
    app.max_visible_items = visible_height;

    let is_narrow = app.is_narrow();
    let is_very_narrow = app.is_very_narrow();
    let sort_field = app.sort_field;
    let sort_direction = app.sort_direction;
    let scroll_offset = app.scroll_offset;
    let selected_index = app.selected_index;
    let theme_accent = app.theme.accent;
    let theme_muted = app.theme.muted;
    let theme_selection = app.theme.selection;

    let accounts = app.get_sorted_codex_accounts();
    if accounts.is_empty() {
        let empty_msg = Paragraph::new(
            "No Codex usage for these sources.\nPress 's' to change sources or 'r' to refresh.",
        )
        .style(Style::default().fg(theme_muted))
        .alignment(Alignment::Center);
        frame.render_widget(empty_msg, inner);
        return;
    }

    let header_cells = if is_very_narrow {
        vec!["Account", "Sub"]
    } else if is_narrow {
        vec!["Account", "API", "Sub"]
    } else {
        vec![
            "#",
            "Account",
            "Tokens",
            "API Cost",
            "Sub Spend",
            "Months",
            "Sessions",
            "Range",
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
            .map(|(i, h)| {
                let indicator = match i {
                    2 if !is_narrow => sort_indicator(SortField::Tokens),
                    3 if !is_narrow => sort_indicator(SortField::Cost),
                    7 if !is_narrow => sort_indicator(SortField::Date),
                    1 if is_very_narrow => sort_indicator(SortField::Cost),
                    1 if is_narrow && !is_very_narrow => sort_indicator(SortField::Cost),
                    _ => "",
                };
                Cell::from(format!("{}{}", h, indicator))
            })
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(theme_accent)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let accounts_len = accounts.len();
    let start = scroll_offset.min(accounts_len.saturating_sub(1));
    let end = (start + visible_height).min(accounts_len);

    if start >= accounts_len {
        return;
    }

    let rows: Vec<Row> = accounts[start..end]
        .iter()
        .enumerate()
        .map(|(i, account)| {
            let idx = i + start;
            let is_selected = idx == selected_index;
            let is_striped = idx % 2 == 1;
            let label = format_codex_account_label(&account.account_hash);

            let cells: Vec<Cell> = if is_very_narrow {
                vec![
                    Cell::from(truncate(&label, 18))
                        .style(Style::default().fg(app.theme.foreground)),
                    Cell::from(format_optional_cost(account.paid_cost))
                        .style(Style::default().fg(Color::Green)),
                ]
            } else if is_narrow {
                vec![
                    Cell::from(truncate(&label, 18))
                        .style(Style::default().fg(app.theme.foreground)),
                    Cell::from(format_cost(account.cost)).style(Style::default().fg(Color::Cyan)),
                    Cell::from(format_optional_cost(account.paid_cost))
                        .style(Style::default().fg(Color::Green)),
                ]
            } else {
                vec![
                    Cell::from(format!("{}", idx + 1)).style(Style::default().fg(theme_muted)),
                    Cell::from(truncate(&label, 22)).style(
                        Style::default()
                            .fg(app.theme.foreground)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(format_tokens(account.tokens.total())),
                    Cell::from(format_cost(account.cost)).style(Style::default().fg(Color::Cyan)),
                    Cell::from(format_optional_cost(account.paid_cost))
                        .style(Style::default().fg(Color::Green)),
                    Cell::from(format_optional_months(account.active_month_count))
                        .style(Style::default().fg(theme_muted)),
                    Cell::from(account.session_count.to_string())
                        .style(Style::default().fg(theme_muted)),
                    Cell::from(date_range(account.first_date, account.latest_date))
                        .style(Style::default().fg(theme_muted)),
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
        vec![Constraint::Percentage(70), Constraint::Percentage(30)]
    } else if is_narrow {
        vec![
            Constraint::Percentage(45),
            Constraint::Percentage(27),
            Constraint::Percentage(28),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(23),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(theme_selection));

    frame.render_widget(table, inner);

    if accounts_len > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = ScrollbarState::new(accounts_len).position(scroll_offset);

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

fn format_optional_cost(cost: Option<f64>) -> String {
    cost.map(format_cost).unwrap_or_else(|| "-".to_string())
}

fn format_optional_months(months: Option<u32>) -> String {
    months
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn date_range(first: Option<chrono::NaiveDate>, latest: Option<chrono::NaiveDate>) -> String {
    match (first, latest) {
        (Some(first), Some(latest)) if first == latest => first.to_string(),
        (Some(first), Some(latest)) => format!("{first}..{latest}"),
        _ => "-".to_string(),
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 3 {
        s.chars().take(max_chars).collect()
    } else {
        let head: String = s.chars().take(max_chars - 3).collect();
        format!("{}...", head)
    }
}
