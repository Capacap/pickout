//! Drawing.
//!
//! Colours are named as ANSI slot numbers, never as hex, so the dialog follows
//! `~/.config/foot/foot.ini` the way helix and yazi do. Chrome signals with
//! brightness and nothing else: the selected row is a bright pointer and bright
//! text, because slot 0 against the terminal background is too small a step to
//! read at a glance, and no state here is worth spending a hue on.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Padding};
use unicode_width::UnicodeWidthChar;

use crate::dialog::{App, Focus, Row, abbreviate};
use crate::model::{self, Kind, SaveTarget};

/// Slot 8 — furniture: borders, labels, sizes, key hints.
const DIM: Color = Color::DarkGray;
/// Slot 7 — body text.
const TEXT: Color = Color::Gray;
/// Slot 15 — what the eye should land on.
const BRIGHT: Color = Color::White;

/// Left inset inside the border, and the right inset that keeps the size column
/// clear of it.
const PAD_LEFT: u16 = 2;
const PAD_RIGHT: u16 = 4;
/// Width of the list's pointer column.
const POINTER: u16 = 2;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());

    let block = Block::bordered()
        .border_style(Style::new().fg(DIM))
        .title(Line::from(vec![
            Span::styled("─", Style::new().fg(DIM)),
            Span::styled(format!(" {} ", app.title()), Style::new().fg(TEXT)),
        ]));
    let inner = block.inner(body);
    frame.render_widget(block, body);

    let named = app.saving();
    let rows: &[Constraint] = if named {
        &[
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else {
        &[
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ]
    };
    let areas = Layout::vertical(rows).split(inner);

    draw_path(frame, app, areas[0]);
    rule(frame.buffer_mut(), body, areas[1].y);
    draw_list(frame, app, areas[2]);
    if named {
        rule(frame.buffer_mut(), body, areas[3].y);
        draw_name(frame, app, areas[4]);
    }
    draw_footer(frame, app, footer);
}

/// A `├────┤` across the whole box, drawn into the border columns the layout
/// does not own.
fn rule(buf: &mut Buffer, outer: Rect, y: u16) {
    let style = Style::new().fg(DIM);
    for x in outer.x..outer.right() {
        let symbol = if x == outer.x {
            "├"
        } else if x + 1 == outer.right() {
            "┤"
        } else {
            "─"
        };
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(symbol).set_style(style);
        }
    }
}

fn draw_path(frame: &mut Frame, app: &App, area: Rect) {
    let left = area.x + PAD_LEFT;
    let width = area.width.saturating_sub(PAD_LEFT + PAD_RIGHT) as usize;

    if let Some(field) = &app.path {
        let (visible, cursor) = field.view(width);
        frame.render_widget(
            Line::from(Span::styled(visible.to_owned(), Style::new().fg(BRIGHT))),
            Rect { x: left, ..area },
        );
        frame.set_cursor_position((left + cursor as u16, area.y));
        return;
    }

    // The right-hand end carries whatever is currently true of the listing: an
    // error if the directory would not open, otherwise the filter being typed.
    let note = match (&app.message, app.query.is_empty()) {
        (Some(message), _) => Some(Span::styled(message.clone(), Style::new().fg(TEXT))),
        (None, false) => Some(Span::styled(
            format!("/{}", app.query),
            Style::new().fg(BRIGHT),
        )),
        (None, true) => None,
    };

    let path = abbreviate(&app.dir, &app.home);
    let note_width = note.as_ref().map_or(0, |s| s.content.chars().count() + 2);
    let mut spans = vec![Span::styled(
        truncate(&path, width.saturating_sub(note_width)),
        Style::new().fg(TEXT),
    )];
    if let Some(note) = note {
        let used = spans[0].content.chars().count() + note.content.chars().count();
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
        spans.push(note);
    }
    frame.render_widget(Line::from(spans), Rect { x: left, ..area });
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(PAD_LEFT + PAD_RIGHT + POINTER) as usize;

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let (label, size, dim) = match row {
                Row::Parent => ("..".to_owned(), String::new(), false),
                Row::Entry(i) => {
                    let entry = &app.entries[*i];
                    let mut label = entry.name.to_string_lossy().into_owned();
                    let size = match entry.kind {
                        Kind::Dir => {
                            label.push('/');
                            String::new()
                        }
                        Kind::File => model::format_size(entry.size),
                    };
                    (label, size, entry.hidden)
                }
            };

            let name_width = width.saturating_sub(size.chars().count() + 1);
            let label = truncate(&label, name_width);
            let gap = width.saturating_sub(label.chars().count() + size.chars().count());
            ListItem::new(Line::from(vec![
                Span::styled(label, Style::new().fg(if dim { DIM } else { TEXT })),
                Span::raw(" ".repeat(gap)),
                Span::styled(size, Style::new().fg(DIM)),
            ]))
        })
        .collect();

    // The selected row goes bright only while the list is the pane being
    // typed into. Two things claiming the eye at once is what a Name field
    // alongside a browser risks, and brightness is the only signal in play.
    let focused = app.focus == Focus::List && app.path.is_none();
    let list = List::new(items)
        .block(Block::new().padding(Padding::new(PAD_LEFT, PAD_RIGHT, 0, 0)))
        .highlight_symbol("> ")
        .highlight_style(if focused {
            Style::new()
                .fg(BRIGHT)
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(TEXT).bg(Color::Black)
        });
    frame.render_stateful_widget(list, area, &mut app.list);
}

fn draw_name(frame: &mut Frame, app: &App, area: Rect) {
    // Arming an overwrite changes the label as well as brightening the row.
    // A warning that only appends words to the end of a line is a mode you can
    // be in without noticing.
    let (label, accent) = if app.armed {
        ("Replace", BRIGHT)
    } else {
        ("Name", TEXT)
    };
    let hint = hint(app);

    let left = area.x + PAD_LEFT;
    let label_width = 8u16;
    frame.render_widget(
        Line::from(Span::styled(
            format!("{label:<width$}", width = label_width as usize),
            Style::new().fg(if app.armed { BRIGHT } else { DIM }),
        )),
        Rect { x: left, ..area },
    );

    let value_x = left + label_width;
    let width = area
        .right()
        .saturating_sub(value_x + PAD_RIGHT)
        .saturating_sub(hint.chars().count() as u16 + 2) as usize;
    let (visible, cursor) = app.name.view(width);
    frame.render_widget(
        Line::from(Span::styled(
            visible.to_owned(),
            Style::new().fg(accent).add_modifier(if app.armed {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        )),
        Rect {
            x: value_x,
            width: area.right().saturating_sub(value_x),
            ..area
        },
    );

    if !hint.is_empty() {
        let x = area
            .right()
            .saturating_sub(PAD_RIGHT + hint.chars().count() as u16);
        frame.render_widget(
            Line::from(Span::styled(
                hint,
                Style::new().fg(if app.armed { TEXT } else { DIM }),
            )),
            Rect {
                x,
                width: area.right().saturating_sub(x),
                ..area
            },
        );
    }

    if app.focus == Focus::Name && app.path.is_none() {
        frame.set_cursor_position((value_x + cursor as u16, area.y));
    }
}

/// What committing the current name would do, in as few words as it takes.
fn hint(app: &App) -> String {
    match &app.target {
        SaveTarget::Overwrite(_) if app.armed => "Enter again to replace".to_owned(),
        SaveTarget::Overwrite(_) => "exists".to_owned(),
        SaveTarget::IsDirectory(_) => "is a directory".to_owned(),
        SaveTarget::Blocked { obstacle, .. } => format!(
            "{} is a file",
            obstacle
                .file_name()
                .unwrap_or(obstacle.as_os_str())
                .to_string_lossy()
        ),
        SaveTarget::New { parents, .. } if parents.len() == 1 => "creates 1 directory".to_owned(),
        SaveTarget::New { parents, .. } if !parents.is_empty() => {
            format!("creates {} directories", parents.len())
        }
        SaveTarget::New { .. } | SaveTarget::Empty => String::new(),
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let accept = app.accept_label().to_lowercase();
    let mut keys: Vec<(&str, String)> = Vec::new();
    if app.saving() {
        keys.push(("Tab", "pane".to_owned()));
        keys.push(("Enter", format!("open/{accept}")));
    } else if app.picking_dir() {
        keys.push(("^S", format!("{accept} here")));
        keys.push(("Enter", "open".to_owned()));
    } else {
        keys.push(("Enter", accept));
    }
    keys.push(("^L", "path".to_owned()));
    keys.push(("M-h", "hidden".to_owned()));
    keys.push(("Esc", "cancel".to_owned()));

    let mut spans = vec![Span::raw("   ")];
    for (key, what) in keys {
        spans.push(Span::styled(key, Style::new().fg(TEXT)));
        spans.push(Span::styled(format!(" {what}   "), Style::new().fg(DIM)));
    }
    frame.render_widget(Line::from(spans), area);
}

/// Cuts to `width` columns, marking the cut so a truncated name never reads as
/// a real one.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}
