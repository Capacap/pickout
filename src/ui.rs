//! Drawing.
//!
//! Colours are named as ANSI slots and as the terminal's own foreground, never
//! as hex, so the dialog follows `~/.config/foot/foot.ini` the way helix and
//! yazi do — and follows it into a light theme as well as a dark one. That is
//! why the accent is weight rather than a brighter slot, and why the selected
//! row carries no background of its own: no slot means "brighter than the
//! foreground" under both themes, and a slot-0 bar is either invisible or a
//! slab depending on which one you are in. Chrome signals with prominence and
//! nothing else, and no state here is worth spending a hue on.

use std::ffi::OsStr;
use std::path::Path;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Padding};
use unicode_width::UnicodeWidthChar;

use crate::dialog::{App, Bottom, Focus, Row, abbreviate};
use crate::keys::Action;
use crate::model::{self, Kind, SaveTarget};

/// Furniture: borders, labels, sizes, key hints. Slot 8 is the one slot whose
/// meaning survives a theme flip — lighter than the foreground on a dark
/// terminal, darker than it on a light one, muted against either.
const DIM: Style = Style::new().fg(Color::DarkGray);
/// Body text, in whatever the terminal calls its foreground.
const TEXT: Style = Style::new().fg(Color::Reset);
/// What the eye should land on. Weight rather than a brighter slot, which a
/// terminal renders as extra brightness anyway where it has one to spare.
const BRIGHT: Style = TEXT.add_modifier(Modifier::BOLD);

/// Left inset inside the border, and the right inset that keeps the size column
/// clear of it.
const PAD_LEFT: u16 = 2;
const PAD_RIGHT: u16 = 4;
/// Width of the list's pointer column.
const POINTER: u16 = 2;
/// Width of the mark column, in the modes that have one.
const MARK: u16 = 2;
/// Width of the label on the row under the listing.
const LABEL: u16 = 8;
/// Narrower than this and the padded label alone runs past the border, and a
/// listing has no room to show a name and a size on one row.
const MIN_WIDTH: u16 = 20;

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The jump list takes the whole box: it is a different question, and the
    // Name field cannot be answered until it has been.
    let bottom = if app.jump.is_some() {
        None
    } else {
        app.bottom()
    };

    // Two borders, the path bar, its rule, one row of listing and the footer;
    // a row under the listing costs its own rule as well. Under this the
    // layout sheds rows in whatever order the constraints collapse, and the
    // first to go is the Name field — which stays focused and committable
    // while invisible. Saying so is the only honest thing left to draw.
    let needed = if bottom.is_some() { 8 } else { 6 };
    if frame.area().height < needed || frame.area().width < MIN_WIDTH {
        // Short enough to fit in the window it is complaining about.
        let note = if frame.area().height < needed {
            format!("needs {needed} rows")
        } else {
            format!("needs {MIN_WIDTH} columns")
        };
        frame.render_widget(Line::from(Span::styled(note, TEXT)), frame.area());
        return;
    }

    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());

    let block = Block::bordered().border_style(DIM).title(Line::from(vec![
        Span::styled("─", DIM),
        Span::styled(format!(" {} ", app.title()), TEXT),
    ]));
    let inner = block.inner(body);
    frame.render_widget(block, body);

    let rows: &[Constraint] = if bottom.is_some() {
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
    if let Some(bottom) = bottom {
        rule(frame.buffer_mut(), body, areas[3].y);
        match bottom {
            Bottom::Name => draw_name(frame, app, areas[4]),
            Bottom::Writing => draw_writing(frame, app, areas[4]),
            Bottom::Marked => draw_marked(frame, app, areas[4]),
        }
    }
    draw_footer(frame, app, footer);
}

/// A `├────┤` across the whole box, drawn into the border columns the layout
/// does not own.
fn rule(buf: &mut Buffer, outer: Rect, y: u16) {
    for x in outer.x..outer.right() {
        let symbol = if x == outer.x {
            "├"
        } else if x + 1 == outer.right() {
            "┤"
        } else {
            "─"
        };
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(symbol).set_style(DIM);
        }
    }
}

fn draw_path(frame: &mut Frame, app: &App, area: Rect) {
    let left = area.x + PAD_LEFT;
    let width = area.width.saturating_sub(PAD_LEFT + PAD_RIGHT) as usize;

    // The jump list, `Ctrl-L` and `Ctrl-N` borrow this row for a field.
    // Whatever is narrowing the listing belongs directly above it, and there is
    // only ever one such thing at a time.
    let editing = app
        .jump
        .as_ref()
        .map(|jump| &jump.field)
        .or(app.path.as_ref())
        .or(app.mkdir.as_ref());
    if let Some(field) = editing {
        let note = if app.jump.is_some() {
            Some(if app.scanning() {
                "scanning…".to_owned()
            } else {
                format!("{} directories", app.dirs.len())
            })
        } else {
            app.mkdir
                .as_ref()
                .map(|field| mkdir_note(app, field.text()))
        };
        // Half the row at most: the note can name a directory wider than the
        // row, and what is being typed matters more.
        let note = note.map(|note| truncate(&note, width / 2));
        let note_width = note.as_ref().map_or(0, |note| columns(note) + 2);
        let (visible, cursor) = field.view(width.saturating_sub(note_width));
        frame.render_widget(
            Line::from(Span::styled(visible.to_owned(), BRIGHT)),
            Rect { x: left, ..area },
        );
        if let Some(note) = note {
            let x = area
                .right()
                .saturating_sub(PAD_RIGHT + columns(&note) as u16);
            frame.render_widget(
                Line::from(Span::styled(note, DIM)),
                Rect {
                    x,
                    width: area.right().saturating_sub(x),
                    ..area
                },
            );
        }
        frame.set_cursor_position((left + cursor as u16, area.y));
        return;
    }

    // The right-hand end carries whatever is currently true of the listing: an
    // error if the directory would not open, otherwise the filter being typed.
    let note = match (&app.message, app.query.is_empty()) {
        (Some(message), _) => Some(Span::styled(message.clone(), TEXT)),
        (None, false) => Some(Span::styled(format!("/{}", app.query), BRIGHT)),
        (None, true) => None,
    };

    let path = abbreviate(&app.dir, &app.home);
    let note_width = note.as_ref().map_or(0, |s| columns(&s.content) + 2);
    let mut spans = vec![Span::styled(
        truncate(&path, width.saturating_sub(note_width)),
        TEXT,
    )];
    if let Some(note) = note {
        let used = columns(&spans[0].content) + columns(&note.content);
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
        spans.push(note);
    }
    frame.render_widget(Line::from(spans), Rect { x: left, ..area });
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.jump.is_some() {
        return draw_jump(frame, app, area);
    }

    let marking = app.multiple();
    let mark_width = if marking { MARK } else { 0 };
    let width = area
        .width
        .saturating_sub(PAD_LEFT + PAD_RIGHT + POINTER + mark_width) as usize;

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

            // The mark column is blank rather than absent on an unmarked row,
            // so nothing shifts sideways as marks come and go.
            let mark = match row {
                Row::Entry(i)
                    if marking && app.marks.contains(&app.dir.join(&app.entries[*i].name)) =>
                {
                    "•"
                }
                _ => " ",
            };

            let name_width = width.saturating_sub(columns(&size) + 1);
            let label = truncate(&label, name_width);
            let gap = width.saturating_sub(columns(&label) + columns(&size));
            let mut spans = Vec::new();
            if marking {
                spans.push(Span::styled(format!("{mark} "), BRIGHT));
            }
            spans.push(Span::styled(label, if dim { DIM } else { TEXT }));
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(size, DIM));
            ListItem::new(Line::from(spans))
        })
        .collect();

    // The selected row takes the accent only while the list is the pane being
    // typed into. Two things claiming the eye at once is what a Name field
    // alongside a browser risks, and prominence is the only signal in play. The
    // pointer stays either way: it is what says which row Enter would answer.
    let focused = app.focus == Focus::List && app.path.is_none() && app.mkdir.is_none();
    let list = List::new(items)
        .block(Block::new().padding(Padding::new(PAD_LEFT, PAD_RIGHT, 0, 0)))
        .highlight_symbol("> ")
        .highlight_style(if focused { BRIGHT } else { TEXT });
    frame.render_stateful_widget(list, area, &mut app.list);
}

fn draw_jump(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(PAD_LEFT + PAD_RIGHT + POINTER) as usize;
    let App {
        jump, dirs, home, ..
    } = app;
    let Some(jump) = jump else { return };

    let items: Vec<ListItem> = jump
        .shown
        .iter()
        .map(|i| {
            ListItem::new(Line::from(Span::styled(
                truncate(&abbreviate(&dirs[*i], home), width),
                TEXT,
            )))
        })
        .collect();

    let list = List::new(items)
        .block(Block::new().padding(Padding::new(PAD_LEFT, PAD_RIGHT, 0, 0)))
        .highlight_symbol("> ")
        .highlight_style(BRIGHT);
    frame.render_stateful_widget(list, area, &mut jump.list);
}

/// Draws the label and the right-hand hint of the row under the listing, and
/// returns where the value goes and how much room it has.
fn label_row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    label_style: Style,
    hint: &str,
    hint_style: Style,
) -> (u16, usize) {
    let left = area.x + PAD_LEFT;
    frame.render_widget(
        Line::from(Span::styled(
            format!("{label:<width$}", width = LABEL as usize),
            label_style,
        )),
        Rect { x: left, ..area },
    );

    let value_x = left + LABEL;
    // A hint placed by saturating subtraction lands on the label once the row
    // is too narrow for both. The hint is commentary and the value is the
    // answer, so a hint that would leave the value nothing to show is dropped
    // instead and the room goes back to the value.
    let hint_x = area
        .right()
        .checked_sub(PAD_RIGHT + columns(hint) as u16)
        .filter(|x| !hint.is_empty() && *x > value_x + LABEL);
    if let Some(x) = hint_x {
        frame.render_widget(
            Line::from(Span::styled(hint.to_owned(), hint_style)),
            Rect {
                x,
                width: area.right().saturating_sub(x),
                ..area
            },
        );
    }

    let reserved = hint_x.map_or(0, |_| columns(hint) as u16 + 2);
    let width = area
        .right()
        .saturating_sub(value_x + PAD_RIGHT)
        .saturating_sub(reserved) as usize;
    (value_x, width)
}

/// The names `SaveFiles` will write, where the Name field would be. The
/// application chose them, so they are shown rather than offered.
fn draw_writing(frame: &mut Frame, app: &App, area: Rect) {
    let (names, existing) = app.writing();
    let hint = match existing {
        0 => String::new(),
        1 => "1 exists".to_owned(),
        n => format!("{n} exist"),
    };
    let bright = existing > 0;
    let (x, width) = label_row(
        frame,
        area,
        "Writing",
        if bright { BRIGHT } else { DIM },
        &hint,
        TEXT,
    );
    frame.render_widget(
        Line::from(Span::styled(
            truncate(&names.join(", "), width),
            if bright { BRIGHT } else { TEXT },
        )),
        Rect {
            x,
            width: area.right().saturating_sub(x),
            ..area
        },
    );
}

fn draw_marked(frame: &mut Frame, app: &App, area: Rect) {
    let names = app.marked();
    let hint = format!("{} selected", names.len());
    let (x, width) = label_row(frame, area, "Marked", DIM, &hint, DIM);
    frame.render_widget(
        Line::from(Span::styled(truncate(&names.join(", "), width), TEXT)),
        Rect {
            x,
            width: area.right().saturating_sub(x),
            ..area
        },
    );
}

fn draw_name(frame: &mut Frame, app: &App, area: Rect) {
    // Arming an overwrite changes the label as well as the weight of the row.
    // A warning that only appends words to the end of a line is a mode you can
    // be in without noticing.
    let (label, accent) = if app.armed {
        ("Replace", BRIGHT)
    } else {
        ("Name", TEXT)
    };
    let hint = hint(app);
    let (value_x, width) = label_row(
        frame,
        area,
        label,
        if app.armed { BRIGHT } else { DIM },
        &hint,
        if app.armed { TEXT } else { DIM },
    );

    let (visible, cursor) = app.name.view(width);
    frame.render_widget(
        Line::from(Span::styled(visible.to_owned(), accent)),
        Rect {
            x: value_x,
            width: area.right().saturating_sub(value_x),
            ..area
        },
    );

    if app.focus == Focus::Name && app.path.is_none() && app.mkdir.is_none() {
        frame.set_cursor_position((value_x + cursor as u16, area.y));
    }
}

/// What committing the current name would do, in as few words as it takes.
fn hint(app: &App) -> String {
    match &app.target {
        SaveTarget::Overwrite(_) if app.armed => "Enter again to replace".to_owned(),
        SaveTarget::Overwrite(_) => "exists".to_owned(),
        SaveTarget::IsDirectory(_) => "is a directory, Enter goes in".to_owned(),
        SaveTarget::NewDirectory { parents, .. } => {
            format!("{}, Enter goes in", creates(parents.len() + 1))
        }
        SaveTarget::Blocked { obstacle, .. } => is_a_file(obstacle),
        SaveTarget::New { parents, .. } if !parents.is_empty() => creates(parents.len()),
        SaveTarget::New { .. } | SaveTarget::Empty => String::new(),
    }
}

/// What accepting the `Ctrl-N` field would do. A relative path lands under the
/// directory being browsed, which the field has taken the place of on screen,
/// so that is named until the path typed says where it goes by itself.
fn mkdir_note(app: &App, typed: &str) -> String {
    let here = abbreviate(&app.dir, &app.home);
    if typed.is_empty() {
        return format!("new directory in {here}");
    }
    let absolute = typed.starts_with('/') || typed == "~" || typed.starts_with("~/");
    let as_dir = if typed.ends_with('/') {
        typed.to_owned()
    } else {
        format!("{typed}/")
    };
    match model::save_target(&app.dir, &app.home, OsStr::new(&as_dir)) {
        SaveTarget::IsDirectory(_) => "exists, Enter goes in".to_owned(),
        SaveTarget::NewDirectory { parents, .. } if absolute => creates(parents.len() + 1),
        SaveTarget::NewDirectory { parents, .. } => {
            format!("{} in {here}", creates(parents.len() + 1))
        }
        SaveTarget::Blocked { obstacle, .. } => is_a_file(&obstacle),
        // A name with a trailing slash is never a file to write.
        SaveTarget::New { .. } | SaveTarget::Overwrite(_) | SaveTarget::Empty => String::new(),
    }
}

fn creates(directories: usize) -> String {
    if directories == 1 {
        "creates 1 directory".to_owned()
    } else {
        format!("creates {directories} directories")
    }
}

fn is_a_file(obstacle: &Path) -> String {
    format!(
        "{} is a file",
        obstacle
            .file_name()
            .unwrap_or(obstacle.as_os_str())
            .to_string_lossy()
    )
}

/// What the footer offers, in the order it offers it. Which of these are shown
/// is not decided here: the dialog is asked, and it is asked the same question
/// the dispatcher asks, so the footer cannot advertise a key that does nothing.
const OFFERED: &[Action] = &[
    Action::Pane,
    // The answer leads, where it is not simply `Enter` on the row.
    Action::Commit,
    Action::Activate,
    Action::Go,
    Action::Mark,
    Action::Filter,
    Action::Jump,
    Action::Path,
    Action::Mkdir,
    Action::Hidden,
    Action::Dismiss,
    Action::Cancel,
    Action::Close,
];

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let context = app.context();
    let keys = OFFERED.iter().filter_map(|action| {
        let key = app.keys.key(context, *action)?;
        if !app.advertises(context, *action) {
            return None;
        }
        let label = app.label(context, *action);
        (!label.is_empty()).then(|| (key.to_string(), label))
    });

    let mut spans = vec![Span::raw("   ")];
    for (key, what) in keys {
        spans.push(Span::styled(key, TEXT));
        spans.push(Span::styled(format!(" {what}   "), DIM));
    }
    frame.render_widget(Line::from(spans), area);
}

/// Display columns, which is what a terminal lays out in. A CJK name is twice
/// as wide as it is long, so measuring one part of a row in chars and another
/// in columns overruns the row by the difference.
fn columns(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Cuts to `width` columns, marking the cut so a truncated name never reads as
/// a real one.
fn truncate(s: &str, width: usize) -> String {
    if columns(s) <= width {
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
