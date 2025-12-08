use crate::model::{Board, Note};
use crate::storage::{save_board, BoardLocation};
use anyhow::{anyhow, Result};
use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, NaiveDate, NaiveDateTime, TimeZone, Utc,
};
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
use std::collections::{BTreeMap, HashMap};
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
    view: ViewMode,
    timeline: TimelineState,
    project: ProjectState,
}

enum Mode {
    Normal,
    Creating(NoteForm),
    Editing { note_id: String, form: NoteForm },
    ConfirmDelete { note_id: String },
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ViewMode {
    Board,
    Timeline,
    Project,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TimelineFocus {
    Unassigned,
    Assigned,
    Calendar,
}

struct TimelineState {
    focus: TimelineFocus,
    unassigned_idx: usize,
    assigned_idx: usize,
    calendar_cursor: NaiveDate,
    unassigned_offset: usize,
    assigned_offset: usize,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ProjectFocus {
    Tags,
    Notes,
}

struct ProjectState {
    focus: ProjectFocus,
    tag_idx: usize,
    note_idx: usize,
}

impl TimelineState {
    fn new(board: &Board) -> Self {
        let today = Utc::now().date_naive();
        let earliest_due = board
            .notes
            .values()
            .filter_map(|n| n.due.map(|d| d.date_naive()))
            .min();
        let cursor = earliest_due.unwrap_or(today);
        TimelineState {
            focus: TimelineFocus::Assigned,
            unassigned_idx: 0,
            assigned_idx: 0,
            calendar_cursor: cursor,
            unassigned_offset: 0,
            assigned_offset: 0,
        }
    }

    fn next_focus(&mut self) {
        self.focus = match self.focus {
            TimelineFocus::Unassigned => TimelineFocus::Assigned,
            TimelineFocus::Assigned => TimelineFocus::Calendar,
            TimelineFocus::Calendar => TimelineFocus::Unassigned,
        };
    }

    fn prev_focus(&mut self) {
        self.focus = match self.focus {
            TimelineFocus::Unassigned => TimelineFocus::Calendar,
            TimelineFocus::Assigned => TimelineFocus::Unassigned,
            TimelineFocus::Calendar => TimelineFocus::Assigned,
        };
    }
}

impl ViewMode {
    fn label(&self) -> &'static str {
        match self {
            ViewMode::Board => "Board",
            ViewMode::Timeline => "Timeline",
            ViewMode::Project => "Project",
        }
    }
}

impl ProjectState {
    fn new() -> Self {
        ProjectState {
            focus: ProjectFocus::Tags,
            tag_idx: 0,
            note_idx: 0,
        }
    }

    fn focus_notes(&mut self) {
        self.focus = ProjectFocus::Notes;
    }

    fn focus_tags(&mut self) {
        self.focus = ProjectFocus::Tags;
    }
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
        let timeline = TimelineState::new(&board);
        App {
            board,
            location,
            selected_column: 0,
            selected_note: 0,
            scroll_offsets: vec![0; column_count],
            last_save: Instant::now(),
            status,
            mode: Mode::Normal,
            view: ViewMode::Board,
            timeline,
            project: ProjectState::new(),
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
            KeyCode::Char('1') => {
                self.set_view(ViewMode::Board);
                return Ok(false);
            }
            KeyCode::Char('2') => {
                self.set_view(ViewMode::Timeline);
                return Ok(false);
            }
            KeyCode::Char('3') => {
                self.set_view(ViewMode::Project);
                return Ok(false);
            }
            KeyCode::Char('n') => {
                self.mode = Mode::Creating(NoteForm::new());
                self.status =
                    "Creating new task (Tab/Shift-Tab move, Ctrl+Enter save, Esc cancel)".into();
                return Ok(false);
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
                return Ok(false);
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
                return Ok(false);
            }
            _ => {}
        }

        match self.view {
            ViewMode::Board => self.handle_board_key(key),
            ViewMode::Timeline => self.handle_timeline_key(key),
            ViewMode::Project => self.handle_project_key(key),
        }
    }

    fn handle_board_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.prev_column(),
            KeyCode::Right | KeyCode::Char('l') => self.next_column(),
            KeyCode::Up | KeyCode::Char('k') => self.prev_note(),
            KeyCode::Down | KeyCode::Char('j') => self.next_note(),
            KeyCode::Char('m') | KeyCode::Char('>') => self.move_selected(1)?,
            KeyCode::Char('b') | KeyCode::Char('<') => self.move_selected(-1)?,
            _ => {}
        }
        Ok(false)
    }

    fn handle_timeline_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Tab => self.timeline.next_focus(),
            KeyCode::BackTab => self.timeline.prev_focus(),
            KeyCode::Left | KeyCode::Char('h') => match self.timeline.focus {
                TimelineFocus::Calendar => self.shift_calendar(-1),
                TimelineFocus::Assigned => self.timeline.prev_focus(),
                TimelineFocus::Unassigned => {}
            },
            KeyCode::Right | KeyCode::Char('l') => match self.timeline.focus {
                TimelineFocus::Calendar => self.shift_calendar(1),
                TimelineFocus::Unassigned => self.timeline.next_focus(),
                TimelineFocus::Assigned => self.timeline.next_focus(),
            },
            KeyCode::Up | KeyCode::Char('k') => match self.timeline.focus {
                TimelineFocus::Unassigned => {
                    if self.timeline.unassigned_idx > 0 {
                        self.timeline.unassigned_idx -= 1;
                    }
                }
                TimelineFocus::Assigned => {
                    if self.timeline.assigned_idx > 0 {
                        self.timeline.assigned_idx -= 1;
                    }
                }
                TimelineFocus::Calendar => self.shift_calendar(-7),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.timeline.focus {
                TimelineFocus::Unassigned => self.timeline.unassigned_idx += 1,
                TimelineFocus::Assigned => self.timeline.assigned_idx += 1,
                TimelineFocus::Calendar => self.shift_calendar(7),
            },
            KeyCode::Enter => {
                if self.timeline.focus == TimelineFocus::Calendar {
                    if let Some(idx) = self.first_due_on_cursor() {
                        self.timeline.assigned_idx = idx;
                        self.timeline.focus = TimelineFocus::Assigned;
                        self.status = format!(
                            "Viewing tasks due {}",
                            self.timeline.calendar_cursor.format("%Y-%m-%d").to_string()
                        );
                    } else {
                        self.status = "No tasks due on that day".into();
                    }
                }
            }
            _ => {}
        }
        self.ensure_timeline_bounds();
        Ok(false)
    }

    fn handle_project_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Tab => match self.project.focus {
                ProjectFocus::Tags => self.project.focus_notes(),
                ProjectFocus::Notes => self.project.focus_tags(),
            },
            KeyCode::Left | KeyCode::Char('h') => self.project.focus_tags(),
            KeyCode::Right | KeyCode::Char('l') => self.project.focus_notes(),
            KeyCode::Up | KeyCode::Char('k') => match self.project.focus {
                ProjectFocus::Tags => {
                    if self.project.tag_idx > 0 {
                        self.project.tag_idx -= 1;
                        self.project.note_idx = 0;
                    }
                }
                ProjectFocus::Notes => {
                    if self.project.note_idx > 0 {
                        self.project.note_idx -= 1;
                    }
                }
            },
            KeyCode::Down | KeyCode::Char('j') => match self.project.focus {
                ProjectFocus::Tags => {
                    self.project.tag_idx += 1;
                    self.project.note_idx = 0;
                }
                ProjectFocus::Notes => {
                    self.project.note_idx += 1;
                }
            },
            _ => {}
        }
        self.ensure_project_bounds();
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

    fn set_view(&mut self, view: ViewMode) {
        if self.view != view {
            self.view = view;
            self.status = format!("Switched to {} view", view.label());
        }
        self.ensure_timeline_bounds();
        self.ensure_project_bounds();
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
        match self.view {
            ViewMode::Board => self.draw_board(f, layout[1]),
            ViewMode::Timeline => self.draw_timeline(f, layout[1]),
            ViewMode::Project => self.draw_project(f, layout[1]),
        }
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
            Span::raw("  •  "),
            Span::styled(
                format!("view {}", self.view.label().to_lowercase()),
                Style::default().fg(Color::Magenta),
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

    fn draw_board(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
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

    fn draw_timeline(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        self.ensure_timeline_bounds();
        let (unassigned, assigned) = self.timeline_lists();
        let outer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);
        let left = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(outer[0]);

        let unassigned_offset = self.draw_timeline_column(
            f,
            left[0],
            "Unassigned Tasks",
            &unassigned,
            self.timeline.focus == TimelineFocus::Unassigned,
            self.timeline.unassigned_offset,
            self.timeline.unassigned_idx,
            false,
        );
        let assigned_offset = self.draw_timeline_column(
            f,
            left[1],
            "Assigned Tasks",
            &assigned,
            self.timeline.focus == TimelineFocus::Assigned,
            self.timeline.assigned_offset,
            self.timeline.assigned_idx,
            true,
        );

        let counts = self.timeline_due_counts();
        self.draw_timeline_calendar(
            f,
            outer[1],
            &counts,
            self.timeline.focus == TimelineFocus::Calendar,
        );
        drop(unassigned);
        drop(assigned);
        self.timeline.unassigned_offset = unassigned_offset;
        self.timeline.assigned_offset = assigned_offset;
    }

    fn draw_timeline_column(
        &self,
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        title: &str,
        notes: &[(&str, &Note)],
        focused: bool,
        offset: usize,
        selected_idx: usize,
        show_due: bool,
    ) -> usize {
        let mut state = ListState::default();
        let viewport = area.height.saturating_sub(2) as usize;
        let effective_idx = selected_idx.min(notes.len().saturating_sub(1));
        let new_offset = adjust_offset(effective_idx, offset, viewport, 1, notes.len());
        *state.offset_mut() = new_offset;
        if focused && !notes.is_empty() {
            state.select(Some(effective_idx));
        }

        let items = if notes.is_empty() {
            vec![ListItem::new("No tasks")]
        } else {
            notes
                .iter()
                .map(|(id, note)| timeline_list_item(id, note, show_due))
                .collect()
        };
        let block = Block::default()
            .title(Span::styled(
                format!("{} ({})", title, notes.len()),
                Style::default()
                    .fg(if focused { Color::Cyan } else { Color::Gray })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::LightCyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);
        new_offset
    }

    fn draw_timeline_calendar(
        &self,
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        counts: &HashMap<NaiveDate, usize>,
        focused: bool,
    ) {
        let cursor = self.timeline.calendar_cursor;
        let month_start =
            NaiveDate::from_ymd_opt(cursor.year(), cursor.month(), 1).unwrap_or(cursor);
        let days = days_in_month(month_start.year(), month_start.month());
        let start_offset = month_start.weekday().num_days_from_monday();
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            format!("{} {}", month_start.format("%B"), month_start.year()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        let headings = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
        let header_spans: Vec<Span<'static>> = headings
            .iter()
            .map(|h| Span::styled(format!("{:^6}", h), Style::default().fg(Color::Gray)))
            .collect();
        lines.push(Line::from(header_spans));

        let mut day: i32 = 1 - start_offset as i32;
        while day <= days as i32 {
            let mut spans = Vec::new();
            for _ in 0..7 {
                if day < 1 || day > days as i32 {
                    spans.push(Span::raw("      "));
                } else if let Some(date) =
                    NaiveDate::from_ymd_opt(month_start.year(), month_start.month(), day as u32)
                {
                    let count = counts.get(&date).copied().unwrap_or(0);
                    let mut text = if count > 0 {
                        format!("{:>2}({:>2})", day, count)
                    } else {
                        format!("{:>2}     ", day)
                    };
                    text = format!("{:>6}", text);
                    let mut style = Style::default().fg(if count > 0 {
                        Color::LightYellow
                    } else {
                        Color::Gray
                    });
                    if date == cursor {
                        style = style
                            .bg(if focused { Color::Cyan } else { Color::Blue })
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD);
                    }
                    spans.push(Span::styled(text, style));
                }
                spans.push(Span::raw(" "));
                day += 1;
            }
            lines.push(Line::from(spans));
        }

        let block = Block::default()
            .title(Span::styled(
                "Calendar",
                Style::default()
                    .fg(if focused { Color::Cyan } else { Color::Gray })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(paragraph, area);
    }

    fn draw_project(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        self.ensure_project_bounds();
        let tags = self.project_tags();
        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(area);
        self.draw_project_tags(f, sections[0], &tags);
        self.draw_project_notes(f, sections[1], &tags);
    }

    fn draw_project_tags(
        &self,
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        tags: &[(String, Vec<(&str, &Note)>)],
    ) {
        let mut state = ListState::default();
        let viewport = area.height.saturating_sub(2) as usize;
        let selected = self.project.tag_idx.min(tags.len().saturating_sub(1));
        let offset = adjust_offset(selected, 0, viewport, 1, tags.len());
        *state.offset_mut() = offset;
        if self.project.focus == ProjectFocus::Tags && !tags.is_empty() {
            state.select(Some(selected));
        }

        let items = if tags.is_empty() {
            vec![ListItem::new("No tags yet")]
        } else {
            tags.iter()
                .map(|(tag, notes)| {
                    ListItem::new(format!("{} ({})", tag, notes.len()))
                        .style(Style::default().fg(Color::White))
                })
                .collect()
        };
        let block = Block::default()
            .title(Span::styled(
                "Project Tags",
                Style::default()
                    .fg(if self.project.focus == ProjectFocus::Tags {
                        Color::Cyan
                    } else {
                        Color::Gray
                    })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(
                Style::default().fg(if self.project.focus == ProjectFocus::Tags {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }),
            );
        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::LightCyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn draw_project_notes(
        &self,
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        tags: &[(String, Vec<(&str, &Note)>)],
    ) {
        let notes = tags
            .get(self.project.tag_idx)
            .map(|(_, notes)| notes.as_slice())
            .unwrap_or(&[]);
        let mut state = ListState::default();
        let viewport = area.height.saturating_sub(2) as usize;
        let selected = self.project.note_idx.min(notes.len().saturating_sub(1));
        let offset = adjust_offset(selected, 0, viewport, 1, notes.len());
        *state.offset_mut() = offset;
        if self.project.focus == ProjectFocus::Notes && !notes.is_empty() {
            state.select(Some(selected));
        }

        let items = if notes.is_empty() {
            vec![ListItem::new("No tasks for this tag")]
        } else {
            notes
                .iter()
                .map(|(id, note)| project_note_item(id, note))
                .collect()
        };

        let block = Block::default()
            .title(Span::styled(
                "Tagged Tasks",
                Style::default()
                    .fg(if self.project.focus == ProjectFocus::Notes {
                        Color::Cyan
                    } else {
                        Color::Gray
                    })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(
                Style::default().fg(if self.project.focus == ProjectFocus::Notes {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }),
            );
        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::LightCyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn draw_footer(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(2)])
            .split(area);

        let help_bar = Paragraph::new(self.footer_help_line())
            .alignment(Alignment::Center)
            .block(
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

        let (detail_lines, title) = self.detail_content();
        let detail = Paragraph::new(detail_lines)
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(title),
            );
        f.render_widget(detail, bottom[1]);
    }

    fn footer_help_line(&self) -> Line<'static> {
        let mut spans = vec![
            Span::styled("1", Style::default().fg(Color::LightCyan)),
            Span::raw(" board  "),
            Span::styled("2", Style::default().fg(Color::LightCyan)),
            Span::raw(" timeline  "),
            Span::styled("3", Style::default().fg(Color::LightCyan)),
            Span::raw(" project  "),
        ];
        match self.view {
            ViewMode::Board => spans.extend([
                Span::styled("←↑↓→ / h j k l", Style::default().fg(Color::LightCyan)),
                Span::raw(" move  "),
                Span::styled("m/>", Style::default().fg(Color::LightGreen)),
                Span::raw(" forward  "),
                Span::styled("b/<", Style::default().fg(Color::LightGreen)),
                Span::raw(" back  "),
                Span::styled("n", Style::default().fg(Color::LightMagenta)),
                Span::raw(" new  "),
                Span::styled("e", Style::default().fg(Color::LightYellow)),
                Span::raw(" edit  "),
                Span::styled("d", Style::default().fg(Color::LightRed)),
                Span::raw(" delete  "),
                Span::styled("q", Style::default().fg(Color::LightRed)),
                Span::raw(" quit"),
            ]),
            ViewMode::Timeline => spans.extend([
                Span::styled("Tab", Style::default().fg(Color::LightCyan)),
                Span::raw(" focus  "),
                Span::styled("←→", Style::default().fg(Color::LightCyan)),
                Span::raw(" move focus/day  "),
                Span::styled("↑↓", Style::default().fg(Color::LightCyan)),
                Span::raw(" browse  "),
                Span::styled("Enter", Style::default().fg(Color::LightYellow)),
                Span::raw(" jump to day  "),
                Span::styled("n", Style::default().fg(Color::LightMagenta)),
                Span::raw(" new  "),
                Span::styled("e", Style::default().fg(Color::LightYellow)),
                Span::raw(" edit  "),
                Span::styled("d", Style::default().fg(Color::LightRed)),
                Span::raw(" delete  "),
                Span::styled("q", Style::default().fg(Color::LightRed)),
                Span::raw(" quit"),
            ]),
            ViewMode::Project => spans.extend([
                Span::styled("Tab", Style::default().fg(Color::LightCyan)),
                Span::raw(" focus  "),
                Span::styled("←→", Style::default().fg(Color::LightCyan)),
                Span::raw(" switch pane  "),
                Span::styled("↑↓", Style::default().fg(Color::LightCyan)),
                Span::raw(" browse  "),
                Span::styled("n", Style::default().fg(Color::LightMagenta)),
                Span::raw(" new  "),
                Span::styled("e", Style::default().fg(Color::LightYellow)),
                Span::raw(" edit  "),
                Span::styled("d", Style::default().fg(Color::LightRed)),
                Span::raw(" delete  "),
                Span::styled("q", Style::default().fg(Color::LightRed)),
                Span::raw(" quit"),
            ]),
        }
        Line::from(spans)
    }

    fn detail_content(&self) -> (Vec<Line<'static>>, String) {
        match self.view {
            ViewMode::Board => self.board_detail_content(),
            ViewMode::Timeline => self.timeline_detail_content(),
            ViewMode::Project => self.project_detail_content(),
        }
    }

    fn board_detail_content(&self) -> (Vec<Line<'static>>, String) {
        if let Some((_, note)) = self.current_note() {
            (vec![selected_note_detail(note)], "Selected".into())
        } else {
            (vec![Line::from("No note selected")], "Selected".into())
        }
    }

    fn timeline_detail_content(&self) -> (Vec<Line<'static>>, String) {
        if self.timeline.focus == TimelineFocus::Calendar {
            let date = self.timeline.calendar_cursor;
            let mut lines = vec![Line::from(Span::styled(
                format!("Due {}", date.format("%Y-%m-%d")),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))];
            let notes = self.notes_due_on(date);
            if notes.is_empty() {
                lines.push(Line::from("No tasks due on this date"));
            } else {
                for (id, note) in notes {
                    lines.push(Line::from(format!("{}: {}", id, note.title)));
                }
            }
            (lines, "Calendar".into())
        } else if let Some((_, note)) = self.current_timeline_note() {
            (vec![selected_note_detail(note)], "Selected".into())
        } else {
            (vec![Line::from("No note selected")], "Selected".into())
        }
    }

    fn project_detail_content(&self) -> (Vec<Line<'static>>, String) {
        let tags = self.project_tags();
        if self.project.focus == ProjectFocus::Notes {
            if let Some((_, note)) = self.current_project_note() {
                return (vec![selected_note_detail(note)], "Selected".into());
            }
            return (vec![Line::from("No task selected")], "Selected".into());
        }

        if let Some((tag, notes)) = tags.get(self.project.tag_idx) {
            let mut lines = vec![Line::from(Span::styled(
                tag.clone(),
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            ))];
            lines.push(Line::from(format!("{} task(s)", notes.len())));
            (lines, "Tag".into())
        } else {
            (vec![Line::from("No tags yet")], "Tag".into())
        }
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
        match self.view {
            ViewMode::Board => self.current_board_note(),
            ViewMode::Timeline => self.current_timeline_note(),
            ViewMode::Project => self.current_project_note(),
        }
    }

    fn current_board_note(&self) -> Option<(&str, &Note)> {
        let column = self.board.columns.get(self.selected_column)?;
        let note_id = column.note_ids.get(self.selected_note)?;
        let note = self.board.notes.get(note_id)?;
        Some((note_id.as_str(), note))
    }

    fn current_timeline_note(&self) -> Option<(&str, &Note)> {
        let (unassigned, assigned) = self.timeline_lists();
        match self.timeline.focus {
            TimelineFocus::Unassigned => unassigned.get(self.timeline.unassigned_idx).copied(),
            TimelineFocus::Assigned => assigned.get(self.timeline.assigned_idx).copied(),
            TimelineFocus::Calendar => None,
        }
    }

    fn current_project_note(&self) -> Option<(&str, &Note)> {
        if self.project.focus != ProjectFocus::Notes {
            return None;
        }
        let tags = self.project_tags();
        let (_, notes) = tags.get(self.project.tag_idx)?;
        notes.get(self.project.note_idx).copied()
    }

    fn current_column_id(&self) -> Option<String> {
        self.board
            .columns
            .get(self.selected_column)
            .map(|c| c.id.clone())
    }

    fn timeline_lists(&self) -> (Vec<(&str, &Note)>, Vec<(&str, &Note)>) {
        let mut unassigned = Vec::new();
        let mut assigned = Vec::new();
        for (id, note) in &self.board.notes {
            if note.due.is_some() {
                assigned.push((id.as_str(), note));
            } else {
                unassigned.push((id.as_str(), note));
            }
        }
        unassigned.sort_by_key(|(_, note)| (note.created_at, note.title.to_lowercase()));
        assigned.sort_by_key(|(_, note)| {
            (note.due.unwrap_or_else(Utc::now), note.title.to_lowercase())
        });
        (unassigned, assigned)
    }

    fn notes_due_on(&self, date: NaiveDate) -> Vec<(&str, &Note)> {
        let mut notes = self
            .board
            .notes
            .iter()
            .filter_map(|(id, note)| {
                if let Some(due) = note.due {
                    if due.date_naive() == date {
                        return Some((id.as_str(), note));
                    }
                }
                None
            })
            .collect::<Vec<_>>();
        notes.sort_by_key(|(_, note)| (note.due, note.title.to_lowercase()));
        notes
    }

    fn timeline_due_counts(&self) -> HashMap<NaiveDate, usize> {
        let mut counts = HashMap::new();
        for note in self.board.notes.values() {
            if let Some(due) = note.due {
                *counts.entry(due.date_naive()).or_insert(0) += 1;
            }
        }
        counts
    }

    fn first_due_on_cursor(&self) -> Option<usize> {
        let (_, assigned) = self.timeline_lists();
        let target = self.timeline.calendar_cursor;
        assigned
            .iter()
            .position(|(_, note)| note.due.map(|d| d.date_naive()) == Some(target))
    }

    fn project_tags(&self) -> Vec<(String, Vec<(&str, &Note)>)> {
        let mut buckets: BTreeMap<String, Vec<(&str, &Note)>> = BTreeMap::new();
        for (id, note) in &self.board.notes {
            if note.tags.is_empty() {
                buckets
                    .entry("(untagged)".to_string())
                    .or_default()
                    .push((id.as_str(), note));
            } else {
                for tag in &note.tags {
                    buckets
                        .entry(tag.clone())
                        .or_default()
                        .push((id.as_str(), note));
                }
            }
        }

        let mut tags = Vec::new();
        for (tag, mut notes) in buckets {
            notes.sort_by_key(|(_, note)| (note.updated_at, note.title.to_lowercase()));
            tags.push((tag, notes));
        }
        tags
    }

    fn ensure_timeline_bounds(&mut self) {
        let (unassigned_len, assigned_len) = {
            let (unassigned, assigned) = self.timeline_lists();
            (unassigned.len(), assigned.len())
        };
        if unassigned_len == 0 {
            self.timeline.unassigned_idx = 0;
            self.timeline.unassigned_offset = 0;
        } else {
            self.timeline.unassigned_idx = self
                .timeline
                .unassigned_idx
                .min(unassigned_len.saturating_sub(1));
            self.timeline.unassigned_offset = self
                .timeline
                .unassigned_offset
                .min(unassigned_len.saturating_sub(1));
        }
        if assigned_len == 0 {
            self.timeline.assigned_idx = 0;
            self.timeline.assigned_offset = 0;
        } else {
            self.timeline.assigned_idx = self
                .timeline
                .assigned_idx
                .min(assigned_len.saturating_sub(1));
            self.timeline.assigned_offset = self
                .timeline
                .assigned_offset
                .min(assigned_len.saturating_sub(1));
        }
    }

    fn ensure_project_bounds(&mut self) {
        let tags = self.project_tags();
        let tag_count = tags.len();
        if tag_count == 0 {
            self.project.tag_idx = 0;
            self.project.note_idx = 0;
            self.project.focus_tags();
            return;
        }
        let tag_idx = self.project.tag_idx.min(tag_count.saturating_sub(1));
        let note_len = tags.get(tag_idx).map(|(_, notes)| notes.len()).unwrap_or(0);
        drop(tags);
        self.project.tag_idx = tag_idx;
        if note_len == 0 && self.project.focus == ProjectFocus::Notes {
            self.project.focus_tags();
        }
        self.project.note_idx = if note_len == 0 {
            0
        } else {
            self.project.note_idx.min(note_len.saturating_sub(1))
        };
    }

    fn shift_calendar(&mut self, days: i64) {
        if let Some(new_date) = self
            .timeline
            .calendar_cursor
            .checked_add_signed(ChronoDuration::days(days))
        {
            self.timeline.calendar_cursor = new_date;
        }
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
        self.ensure_timeline_bounds();
        self.ensure_project_bounds();
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

fn days_in_month(year: i32, month: u32) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_else(|| Utc::now().date_naive());
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .unwrap_or(first);
    next.pred_opt().map(|d| d.day()).unwrap_or(28)
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

fn timeline_list_item(id: &str, note: &Note, show_due: bool) -> ListItem<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!("[{}]", id),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        truncate_text(&note.title, 40),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));
    if show_due {
        if let Some(due) = note.due.as_ref() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                due.format("%Y-%m-%d").to_string(),
                Style::default().fg(Color::LightYellow),
            ));
        }
    }
    if !note.tags.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("#{}", note.tags.join(" #")),
            Style::default().fg(Color::LightMagenta),
        ));
    }
    ListItem::new(Line::from(spans)).style(Style::default().fg(Color::Gray))
}

fn project_note_item(id: &str, note: &Note) -> ListItem<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!("[{}]", id),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        truncate_text(&note.title, 36),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(due) = note.due.as_ref() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            due.format("%Y-%m-%d").to_string(),
            Style::default().fg(Color::LightYellow),
        ));
    }
    let tag_text = if note.tags.is_empty() {
        "(no tags)".to_string()
    } else {
        format!("#{}", note.tags.join(" #"))
    };
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        tag_text,
        Style::default().fg(Color::LightMagenta),
    ));
    ListItem::new(Line::from(spans)).style(Style::default().fg(Color::Gray))
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
