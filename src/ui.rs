use crate::model::{Board, Note};
use crate::storage::{save_board, BoardLocation};
use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use rand::{distributions::Alphanumeric, Rng};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::{Alignment, Color, Modifier, Rect, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use std::io::{stdout, Stdout};
use std::time::{Duration, Instant};

pub fn run(board: Board, location: BoardLocation) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new(board, location);
    let result = app.event_loop(&mut terminal);
    teardown_terminal(&mut terminal)?;
    result
}

struct App {
    board: Board,
    location: BoardLocation,
    selected_column: usize,
    selected_note: usize,
    scroll_offsets: Vec<usize>,
    last_save: Instant,
    status: String,
    mode: Mode,
}

enum Mode {
    Normal,
    Creating(NoteForm),
    Editing { note_id: String, form: NoteForm },
    ConfirmDelete { note_id: String },
}

struct NoteForm {
    title: FieldValue,
    body: FieldValue,
    tags: FieldValue,
    due: FieldValue,
    field: FormField,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum FormField {
    Title,
    Body,
    Tags,
    Due,
}

enum FormAction {
    Create,
    Edit(String),
}

#[derive(Clone)]
struct FieldValue {
    value: String,
    cursor: usize,
}

impl FieldValue {
    fn new(value: &str) -> Self {
        FieldValue {
            value: value.to_string(),
            cursor: value.len(),
        }
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = prev_grapheme(self.cursor, &self.value);
    }

    fn move_right(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.cursor = next_grapheme(self.cursor, &self.value);
    }

    fn move_up(&mut self) {
        let (line_starts, line_idx, col) = line_state(&self.value, self.cursor);
        if line_idx == 0 {
            return;
        }
        let target_start = line_starts[line_idx - 1];
        self.cursor = index_at_col(&self.value, target_start, col);
    }

    fn move_down(&mut self) {
        let (line_starts, line_idx, col) = line_state(&self.value, self.cursor);
        if line_idx + 1 >= line_starts.len() {
            return;
        }
        let target_start = line_starts[line_idx + 1];
        self.cursor = index_at_col(&self.value, target_start, col);
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = prev_grapheme(self.cursor, &self.value);
        self.value.drain(prev..self.cursor);
        self.cursor = prev;
    }

    fn insert_char(&mut self, ch: char) {
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn with_caret(&self) -> String {
        let mut text = self.value.clone();
        text.insert_str(self.cursor, "▌");
        text
    }
}

impl App {
    fn new(board: Board, location: BoardLocation) -> Self {
        let status = format!("Loaded board from {}", location.path.display());
        let column_count = board.columns.len();
        App {
            board,
            location,
            selected_column: 0,
            selected_note: 0,
            scroll_offsets: vec![0; column_count],
            last_save: Instant::now(),
            status,
            mode: Mode::Normal,
        }
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            terminal.draw(|f| self.draw(f))?;
            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if self.handle_key(key)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match self.mode {
            Mode::Normal => self.handle_normal_key(key),
            Mode::Creating(_) | Mode::Editing { .. } => self.handle_form_key(key),
            Mode::ConfirmDelete { .. } => self.handle_confirm_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Left | KeyCode::Char('h') => self.prev_column(),
            KeyCode::Right | KeyCode::Char('l') => self.next_column(),
            KeyCode::Up | KeyCode::Char('k') => self.prev_note(),
            KeyCode::Down | KeyCode::Char('j') => self.next_note(),
            KeyCode::Char('m') | KeyCode::Char('>') => self.move_selected(1)?,
            KeyCode::Char('b') | KeyCode::Char('<') => self.move_selected(-1)?,
            KeyCode::Char('n') => {
                self.mode = Mode::Creating(NoteForm::new());
                self.status =
                    "Creating new task (Tab/Shift-Tab move, Ctrl+Enter save, Esc cancel)".into();
            }
            KeyCode::Char('e') => {
                if let Some((id, note)) = self.current_note() {
                    let id_owned = id.to_string();
                    let form = NoteForm::from_note(note);
                    self.mode = Mode::Editing {
                        note_id: id_owned.clone(),
                        form,
                    };
                    self.status = format!("Editing {}", id_owned);
                } else {
                    self.status = "No note selected to edit".into();
                }
            }
            KeyCode::Char('d') => {
                if let Some((id, _)) = self.current_note() {
                    let id_owned = id.to_string();
                    self.mode = Mode::ConfirmDelete {
                        note_id: id_owned.clone(),
                    };
                    self.status = format!("Delete {}? (y to confirm, n/Esc to cancel)", id_owned);
                } else {
                    self.status = "No note selected to delete".into();
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_form_key(&mut self, key: KeyEvent) -> Result<bool> {
        let mut close_form = false;
        let mut mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match &mut mode {
            Mode::Creating(form) => {
                close_form = self.process_form_key(FormAction::Create, form, key)?;
            }
            Mode::Editing { note_id, form } => {
                let id = note_id.clone();
                close_form = self.process_form_key(FormAction::Edit(id), form, key)?;
            }
            Mode::ConfirmDelete { .. } => {}
            Mode::Normal => {}
        }
        self.mode = if close_form { Mode::Normal } else { mode };
        Ok(false)
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> Result<bool> {
        let note_id = match &self.mode {
            Mode::ConfirmDelete { note_id } => note_id.clone(),
            _ => return Ok(false),
        };
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                if let Err(err) = self.delete_note(&note_id) {
                    self.status = format!("Delete failed: {}", err);
                } else {
                    self.persist(format!("Deleted {}", note_id))?;
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.status = "Delete canceled".into();
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        Ok(false)
    }

    fn process_form_key(
        &mut self,
        action: FormAction,
        form: &mut NoteForm,
        key: KeyEvent,
    ) -> Result<bool> {
        let mut close_form = false;
        match key.code {
            KeyCode::Esc => {
                close_form = true;
                self.status = "Canceled".into();
            }
            KeyCode::Tab => form.next_field(),
            KeyCode::BackTab => form.prev_field(),
            KeyCode::Left => form.active_field_mut().move_left(),
            KeyCode::Right => form.active_field_mut().move_right(),
            KeyCode::Up => form.active_field_mut().move_up(),
            KeyCode::Down => form.active_field_mut().move_down(),
            KeyCode::Enter => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                if form.field == FormField::Body && !control {
                    form.active_field_mut().insert_char('\n');
                } else {
                    close_form = self.try_submit(action, form)?;
                }
            }
            KeyCode::Backspace => form.active_field_mut().backspace(),
            KeyCode::Char(c) => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    form.active_field_mut().insert_char(c);
                }
            }
            _ => {}
        }
        Ok(close_form)
    }

    fn try_submit(&mut self, action: FormAction, form: &mut NoteForm) -> Result<bool> {
        match action {
            FormAction::Create => {
                if let Err(err) = self.create_note_from_form(form) {
                    self.status = format!("Could not create: {}", err);
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
            FormAction::Edit(note_id) => {
                if let Err(err) = self.edit_note_from_form(&note_id, form) {
                    self.status = format!("Could not edit: {}", err);
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
        }
    }

    fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(4),
            ])
            .split(f.size());

        self.draw_header(f, layout[0]);
        self.draw_columns(f, layout[1]);
        self.draw_footer(f, layout[2]);

        match &self.mode {
            Mode::Creating(form) => self.draw_form(f, "New Task", form),
            Mode::Editing { form, .. } => self.draw_form(f, "Edit Task", form),
            Mode::ConfirmDelete { note_id } => self.draw_confirm(f, note_id),
            Mode::Normal => {}
        }
    }

    fn draw_header(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let scope = match self.location.scope {
            crate::storage::BoardScope::Project => "project",
            crate::storage::BoardScope::Global => "global",
        };
        let title = Line::from(vec![
            Span::styled(
                "postit ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                &self.board.name,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  •  "),
            Span::styled(scope, Style::default().fg(Color::Green)),
            Span::raw("  •  "),
            Span::styled(
                format!("{}", self.location.path.display()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  •  "),
            Span::styled(
                format!("saved {}", format_elapsed(self.last_save)),
                Style::default().fg(Color::Gray),
            ),
        ]);

        let block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray));
        let paragraph = Paragraph::new(title)
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(paragraph, area);
    }

    fn draw_columns(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        if self.board.columns.is_empty() {
            let msg = Paragraph::new("No columns defined")
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL).title("postit"));
            f.render_widget(Clear, area);
            f.render_widget(msg, area);
            return;
        }

        if self.scroll_offsets.len() < self.board.columns.len() {
            self.scroll_offsets.resize(self.board.columns.len(), 0);
        }

        let chunk_constraints = self
            .board
            .columns
            .iter()
            .map(|_| Constraint::Percentage((100 / self.board.columns.len() as u16).max(1)))
            .collect::<Vec<_>>();

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(chunk_constraints)
            .split(area);

        for (idx, column) in self.board.columns.iter().enumerate() {
            let accent = color_for_index(idx);
            let note_width = chunks[idx].width.saturating_sub(2);
            let notes = column
                .note_ids
                .iter()
                .filter_map(|id| self.board.notes.get(id))
                .enumerate()
                .map(|(n_idx, note)| {
                    note_item(
                        note,
                        note_width,
                        idx == self.selected_column && n_idx == self.selected_note,
                    )
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default();
            let mut offset = *self.scroll_offsets.get(idx).unwrap_or(&0);
            let viewport = chunks[idx].height.saturating_sub(2) as usize;
            let selected = if idx == self.selected_column {
                Some(self.selected_note)
            } else {
                None
            };
            if let Some(sel) = selected {
                offset = adjust_offset(sel, offset, viewport, 1, notes.len());
                self.scroll_offsets[idx] = offset;
                state.select(Some(sel));
                *state.offset_mut() = offset;
            } else {
                *state.offset_mut() = offset.min(notes.len().saturating_sub(1));
            }

            let mut title = format!("{} [{}]", column.name, column.id);
            if let Some(limit) = column.wip_limit {
                title.push_str(&format!(" ({} / {})", column.note_ids.len(), limit));
            } else {
                title.push_str(&format!(" ({})", column.note_ids.len()));
            }

            let block = Block::default()
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(accent)
                        .add_modifier(if idx == self.selected_column {
                            Modifier::BOLD | Modifier::UNDERLINED
                        } else {
                            Modifier::BOLD
                        }),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent))
                .style(Style::default().bg(Color::Rgb(16, 18, 24)));

            let list = List::new(notes).block(block);
            f.render_stateful_widget(list, chunks[idx], &mut state);
        }
    }

    fn draw_footer(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(area);

        let help = Line::from(vec![
            Span::styled("←↑↓→ / h j k l", Style::default().fg(Color::LightCyan)),
            Span::raw(" move  "),
            Span::styled("m / >", Style::default().fg(Color::LightGreen)),
            Span::raw(" forward  "),
            Span::styled("b / <", Style::default().fg(Color::LightGreen)),
            Span::raw(" back  "),
            Span::styled("n", Style::default().fg(Color::LightMagenta)),
            Span::raw(" new  "),
            Span::styled("e", Style::default().fg(Color::LightYellow)),
            Span::raw(" edit  "),
            Span::styled("d", Style::default().fg(Color::LightRed)),
            Span::raw(" delete  "),
            Span::styled("q", Style::default().fg(Color::LightRed)),
            Span::raw(" quit"),
        ]);
        let help_bar = Paragraph::new(help).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        f.render_widget(help_bar, rows[0]);

        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(rows[1]);

        let status = Paragraph::new(self.status.clone())
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
        f.render_widget(status, bottom[0]);

        let detail = if let Some((_, note)) = self.current_note() {
            Paragraph::new(selected_note_detail(note)).wrap(Wrap { trim: true })
        } else {
            Paragraph::new("No note selected")
        }
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray))
                .title("Selected"),
        );
        f.render_widget(detail, bottom[1]);
    }

    fn draw_form(&self, f: &mut ratatui::Frame<'_>, title: &str, form: &NoteForm) {
        let area = centered_rect(70, 60, f.size());
        let mut fields = Vec::new();
        fields.extend(field_lines(
            "Title",
            &form.title,
            form.field == FormField::Title,
        ));
        fields.extend(field_lines(
            "Body",
            &form.body,
            form.field == FormField::Body,
        ));
        fields.extend(field_lines(
            "Tags",
            &form.tags,
            form.field == FormField::Tags,
        ));
        fields.extend(field_lines(
            "Due (YYYY.MM.DD@hh:mm)",
            &form.due,
            form.field == FormField::Due,
        ));
        fields.push(Line::from(Span::styled(
            "Ctrl+Enter to save • Esc to cancel • Tab/Shift-Tab to move • Enter adds newline in Body",
            Style::default().fg(Color::Gray),
        )));
        let dialog = Paragraph::new(fields)
            .block(
                Block::default()
                    .title(Span::styled(
                        title,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: true });

        f.render_widget(Clear, area);
        f.render_widget(dialog, area);
    }

    fn draw_confirm(&self, f: &mut ratatui::Frame<'_>, note_id: &str) {
        let area = centered_rect(50, 30, f.size());
        let title = self
            .board
            .notes
            .get(note_id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| note_id.to_string());
        let body = vec![
            Line::from(Span::styled(
                format!("Delete \"{}\"?", title),
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Press y to confirm, n or Esc to cancel"),
        ];
        let dialog = Paragraph::new(body).alignment(Alignment::Center).block(
            Block::default()
                .title(Span::styled(
                    "Confirm Delete",
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightRed)),
        );
        f.render_widget(Clear, area);
        f.render_widget(dialog, area);
    }

    fn prev_column(&mut self) {
        if self.selected_column > 0 {
            self.selected_column -= 1;
            self.selected_note = 0;
        }
    }

    fn next_column(&mut self) {
        if self.selected_column + 1 < self.board.columns.len() {
            self.selected_column += 1;
            self.selected_note = 0;
        }
    }

    fn prev_note(&mut self) {
        if self.selected_note > 0 {
            self.selected_note -= 1;
        }
    }

    fn next_note(&mut self) {
        if let Some(column) = self.board.columns.get(self.selected_column) {
            if self.selected_note + 1 < column.note_ids.len() {
                self.selected_note += 1;
            }
        }
    }

    fn move_selected(&mut self, delta: isize) -> Result<()> {
        if self.board.columns.is_empty() {
            self.status = "No columns to move between".into();
            return Ok(());
        }
        if self.current_note().is_none() {
            self.status = "No note selected to move".into();
            return Ok(());
        }
        let current = self.selected_column as isize;
        let max = (self.board.columns.len() as isize).saturating_sub(1);
        let target = (current + delta).clamp(0, max) as usize;
        if target == self.selected_column {
            return Ok(());
        }
        if let Err(err) = self.move_to_column(target) {
            self.status = format!("Move failed: {}", err);
            return Ok(());
        }
        let dest = self
            .board
            .columns
            .get(target)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        self.persist(format!("Moved to {}", dest))?;
        Ok(())
    }

    fn move_to_column(&mut self, target_idx: usize) -> Result<()> {
        if self.board.columns.is_empty() {
            return Ok(());
        }
        let current_col = match self.board.columns.get(self.selected_column) {
            Some(c) => c,
            None => return Ok(()),
        };
        let note_id = match current_col.note_ids.get(self.selected_note) {
            Some(id) => id.clone(),
            None => return Ok(()),
        };
        let dest_id = self
            .board
            .columns
            .get(target_idx)
            .map(|c| c.id.clone())
            .ok_or_else(|| anyhow!("unknown destination column"))?;
        self.board.move_note(&note_id, &dest_id)?;
        self.selected_column = target_idx;
        self.selected_note = self
            .board
            .columns
            .get(target_idx)
            .map(|c| c.note_ids.len().saturating_sub(1))
            .unwrap_or(0);
        Ok(())
    }

    fn delete_note(&mut self, note_id: &str) -> Result<()> {
        let col_idx = self
            .board
            .find_note_column_index(note_id)
            .ok_or_else(|| anyhow!("note {} not found", note_id))?;
        self.board.columns[col_idx]
            .note_ids
            .retain(|id| id != note_id);
        self.board.notes.remove(note_id);
        self.selected_note = self
            .selected_note
            .min(self.board.columns[col_idx].note_ids.len().saturating_sub(1));
        Ok(())
    }

    fn current_note(&self) -> Option<(&str, &Note)> {
        let column = self.board.columns.get(self.selected_column)?;
        let note_id = column.note_ids.get(self.selected_note)?;
        let note = self.board.notes.get(note_id)?;
        Some((note_id.as_str(), note))
    }

    fn current_column_id(&self) -> Option<String> {
        self.board
            .columns
            .get(self.selected_column)
            .map(|c| c.id.clone())
    }

    fn create_note_from_form(&mut self, form: &NoteForm) -> Result<()> {
        let title = form.title.value.trim();
        if title.is_empty() {
            return Err(anyhow!("title is required"));
        }
        let column_id = self
            .current_column_id()
            .ok_or_else(|| anyhow!("no columns available to place the note"))?;
        let tags = parse_tags(&form.tags.value);
        let due = parse_due_string(&form.due.value)?;
        let body = if form.body.value.trim().is_empty() {
            None
        } else {
            Some(form.body.value.clone())
        };
        let id = generate_id();
        let note = Note::new(id.clone(), title.to_string(), body, tags, due);
        self.board
            .add_note(note, &column_id)
            .map_err(|err| anyhow!(err))?;
        self.selected_note = self
            .board
            .columns
            .get(self.selected_column)
            .map(|c| c.note_ids.len().saturating_sub(1))
            .unwrap_or(0);
        self.persist(format!("Created note {}", id))?;
        Ok(())
    }

    fn edit_note_from_form(&mut self, note_id: &str, form: &NoteForm) -> Result<()> {
        let title = form.title.value.trim();
        if title.is_empty() {
            return Err(anyhow!("title is required"));
        }
        let tags = parse_tags(&form.tags.value);
        let due = parse_due_string(&form.due.value)?;
        let body = if form.body.value.trim().is_empty() {
            None
        } else {
            Some(form.body.value.clone())
        };
        let title_owned = title.to_string();
        let body_owned = body.clone();
        let tags_owned = tags.clone();
        let due_owned = due.clone();

        self.board
            .update_note(note_id, move |note| {
                note.title = title_owned.clone();
                note.body = body_owned.clone();
                note.tags = tags_owned.clone();
                note.due = due_owned;
            })
            .map_err(|err| anyhow!(err))?;

        self.persist(format!("Updated {}", note_id))?;
        Ok(())
    }

    fn persist(&mut self, message: impl Into<String>) -> Result<()> {
        save_board(&self.location, &self.board)?;
        self.last_save = Instant::now();
        self.status = message.into();
        Ok(())
    }
}

impl NoteForm {
    fn new() -> Self {
        NoteForm {
            title: FieldValue::new(""),
            body: FieldValue::new(""),
            tags: FieldValue::new(""),
            due: FieldValue::new(""),
            field: FormField::Title,
        }
    }

    fn from_note(note: &Note) -> Self {
        NoteForm {
            title: FieldValue::new(&note.title),
            body: FieldValue::new(note.body.as_deref().unwrap_or_default()),
            tags: FieldValue::new(&note.tags.join(" ")),
            due: FieldValue::new(&note.due.as_ref().map(|d| format_due(d)).unwrap_or_default()),
            field: FormField::Title,
        }
    }

    fn next_field(&mut self) {
        self.field = match self.field {
            FormField::Title => FormField::Body,
            FormField::Body => FormField::Tags,
            FormField::Tags => FormField::Due,
            FormField::Due => FormField::Title,
        };
    }

    fn prev_field(&mut self) {
        self.field = match self.field {
            FormField::Title => FormField::Due,
            FormField::Body => FormField::Title,
            FormField::Tags => FormField::Body,
            FormField::Due => FormField::Tags,
        };
    }

    fn active_field_mut(&mut self) -> &mut FieldValue {
        match self.field {
            FormField::Title => &mut self.title,
            FormField::Body => &mut self.body,
            FormField::Tags => &mut self.tags,
            FormField::Due => &mut self.due,
        }
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn teardown_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

fn parse_due_string(input: &str) -> Result<Option<DateTime<Utc>>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let dt = NaiveDateTime::parse_from_str(trimmed, "%Y.%m.%d@%H:%M")
        .map_err(|_| anyhow!("invalid date format (use YYYY.MM.DD@hh:mm): {}", trimmed))?;
    Ok(Some(Utc.from_utc_datetime(&dt)))
}

fn parse_tags(input: &str) -> Vec<String> {
    input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .collect()
}

fn generate_id() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(char::from)
        .collect()
}

fn color_for_index(idx: usize) -> Color {
    let palette = [
        Color::Cyan,
        Color::LightGreen,
        Color::LightMagenta,
        Color::LightBlue,
        Color::LightYellow,
        Color::LightRed,
    ];
    palette[idx % palette.len()]
}

fn adjust_offset(
    selected: usize,
    current_offset: usize,
    viewport: usize,
    scrolloff: usize,
    len: usize,
) -> usize {
    if viewport == 0 || len == 0 {
        return 0;
    }
    let max_offset = len.saturating_sub(viewport);
    let margin = scrolloff.min(viewport.saturating_sub(1));
    let mut offset = current_offset.min(max_offset);
    if selected < offset.saturating_add(margin) {
        offset = selected.saturating_sub(margin);
    } else {
        let upper = offset
            .saturating_add(viewport.saturating_sub(1))
            .saturating_sub(margin);
        if selected > upper {
            offset = selected.saturating_add(margin + 1).saturating_sub(viewport);
        }
    }
    offset.min(max_offset)
}

fn format_due(dt: &DateTime<Utc>) -> String {
    dt.format("%Y.%m.%d@%H:%M").to_string()
}

fn prev_grapheme(cursor: usize, text: &str) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut prev = 0;
    for (idx, _) in text.char_indices() {
        if idx >= cursor {
            break;
        }
        prev = idx;
    }
    prev
}

fn next_grapheme(cursor: usize, text: &str) -> usize {
    for (idx, ch) in text.char_indices() {
        if idx > cursor {
            return idx;
        }
        if idx == cursor {
            return cursor + ch.len_utf8();
        }
    }
    text.len()
}

fn line_state(text: &str, cursor: usize) -> (Vec<usize>, usize, usize) {
    let mut starts = vec![0];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    let mut line_idx = 0;
    for (i, start) in starts.iter().enumerate() {
        if *start <= cursor {
            line_idx = i;
        } else {
            break;
        }
    }
    let col = text[start_of_line(line_idx, &starts)..cursor]
        .chars()
        .count();
    (starts, line_idx, col)
}

fn start_of_line(line_idx: usize, starts: &[usize]) -> usize {
    *starts.get(line_idx).unwrap_or(&0)
}

fn index_at_col(text: &str, start: usize, target_col: usize) -> usize {
    let slice = &text[start..];
    let limit = slice
        .find('\n')
        .map(|idx| idx)
        .unwrap_or_else(|| slice.len());
    let mut col = 0;
    for (idx, _) in slice[..limit].char_indices() {
        if col == target_col {
            return start + idx;
        }
        col += 1;
    }
    start + limit
}

fn truncate_text(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if out.chars().count() >= max.saturating_sub(3) {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    if out.chars().count() > max {
        out.truncate(max);
    }
    out
}

fn note_item(note: &Note, width: u16, selected: bool) -> ListItem<'static> {
    let inner_width = width.saturating_sub(4).max(10) as usize;
    let border_char = if selected { "=" } else { "-" };
    let horiz = border_char.repeat(inner_width);
    let top = format!("+{}+", horiz);
    let title = truncate_text(&note.title, inner_width.saturating_sub(2));
    let due_line = note
        .due
        .as_ref()
        .map(|d| format!("due {}", format_due(d)))
        .unwrap_or_default();
    let tags_line = if note.tags.is_empty() {
        String::new()
    } else {
        format!("#{}", note.tags.join(" #"))
    };
    let due_line = truncate_text(&due_line, inner_width.saturating_sub(2));
    let tags_line = truncate_text(&tags_line, inner_width.saturating_sub(2));
    let lines = vec![
        Line::raw(top.clone()),
        Line::raw(format!("| {:width$} |", title, width = inner_width)),
        Line::raw(format!("| {:width$} |", due_line, width = inner_width)),
        Line::raw(format!("| {:width$} |", tags_line, width = inner_width)),
        Line::raw(top),
    ];
    let base = Style::default().bg(Color::Rgb(22, 24, 30)).fg(Color::Gray);
    let mut item = ListItem::new(lines).style(base);
    if selected {
        item = item.style(
            Style::default()
                .bg(Color::Rgb(252, 214, 112))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    }
    item
}

fn field_lines(label: &str, field: &FieldValue, active: bool) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD | Modifier::DIM);
    let value_style = Style::default().fg(if active { Color::Cyan } else { Color::White });
    let prefix = format!("{}: ", label);
    let spacer = " ".repeat(prefix.chars().count());
    let text = if active {
        field.with_caret()
    } else {
        field.value.clone()
    };
    let segments: Vec<&str> = if text.is_empty() {
        vec![""]
    } else {
        text.split('\n').collect()
    };
    segments
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = Vec::new();
            spans.push(Span::styled(
                if idx == 0 {
                    prefix.clone()
                } else {
                    spacer.clone()
                },
                label_style,
            ));
            spans.push(Span::styled((*line).to_string(), value_style));
            Line::from(spans)
        })
        .collect()
}

fn selected_note_detail(note: &Note) -> Line<'static> {
    let mut spans = vec![Span::styled(
        note.title.clone(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(due) = note.due.as_ref() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format_due(due),
            Style::default().fg(Color::LightRed),
        ));
    }
    if !note.tags.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("#{}", note.tags.join(" #")),
            Style::default().fg(Color::LightMagenta),
        ));
    }
    if let Some(body) = &note.body {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            body.to_string(),
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

fn format_elapsed(last: Instant) -> String {
    let secs = last.elapsed().as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
