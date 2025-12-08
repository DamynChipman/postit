use crate::model::{BoardError, Note};
use crate::storage::{init_project_board, load_board, locate_board, save_board, BoardLocation};
use crate::ui;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rand::{distributions::Alphanumeric, Rng};
use std::env;

pub fn init(name: Option<String>) -> Result<()> {
    let location = init_project_board(name)?;
    println!("Initialized board at {}", location.path.display());
    Ok(())
}

pub fn list(column: Option<String>) -> Result<()> {
    let (board, location) = load_current_board()?;
    println!(
        "Board: {} ({})",
        board.name,
        match location.scope {
            crate::storage::BoardScope::Project => "project",
            crate::storage::BoardScope::Global => "global",
        }
    );
    for col in board.columns {
        if let Some(ref filter) = column {
            if &col.id != filter {
                continue;
            }
        }
        println!("{}", col.id);
        if col.note_ids.is_empty() {
            println!("  (empty)");
        }
        for id in col.note_ids {
            if let Some(note) = board.notes.get(&id) {
                print_note(note);
            } else {
                println!("  - {} (missing)", id);
            }
        }
        println!();
    }
    Ok(())
}

pub fn add(
    title: String,
    body: Option<String>,
    tags: Vec<String>,
    column: Option<String>,
    due: Option<String>,
) -> Result<()> {
    let (mut board, location) = load_current_board()?;
    let column_id = column
        .or_else(|| board.columns.first().map(|c| c.id.clone()))
        .ok_or_else(|| anyhow!("board has no columns"))?;
    let due_dt = parse_due(due.as_deref())?;
    let id = generate_id();
    let note = Note::new(id.clone(), title, body, tags, due_dt);
    board
        .add_note(note, &column_id)
        .with_context(|| format!("adding note to column {}", column_id))?;
    save_board(&location, &board)?;
    println!("Added note {} to {}", id, column_id);
    Ok(())
}

pub fn move_note(note_id: String, column_id: String) -> Result<()> {
    let (mut board, location) = load_current_board()?;
    board
        .move_note(&note_id, &column_id)
        .with_context(|| format!("moving note {} to {}", note_id, column_id))?;
    save_board(&location, &board)?;
    println!("Moved note {} to {}", note_id, column_id);
    Ok(())
}

pub fn edit(
    note_id: String,
    title: Option<String>,
    body: Option<String>,
    tags: Vec<String>,
    clear_tags: bool,
    column: Option<String>,
    due: Option<String>,
    clear_due: bool,
) -> Result<()> {
    let (mut board, location) = load_current_board()?;
    let due_dt = parse_due(due.as_deref())?;
    let mut found = false;
    board
        .update_note(&note_id, |note| {
            if let Some(t) = title.clone() {
                note.title = t;
            }
            if let Some(b) = body.clone() {
                note.body = Some(b);
            }
            if clear_tags {
                note.tags.clear();
            }
            if !tags.is_empty() {
                note.tags = tags.clone();
            }
            if clear_due {
                note.due = None;
            }
            if let Some(d) = due_dt {
                note.due = Some(d);
            }
            found = true;
        })
        .or_else(|err| match err {
            BoardError::NoteNotFound(_) => Ok::<(), anyhow::Error>(()),
            other => Err::<(), anyhow::Error>(other.into()),
        })?;
    if !found {
        bail!("note {} not found", note_id);
    }
    if let Some(col) = column {
        board
            .move_note(&note_id, &col)
            .with_context(|| format!("moving note {} to {}", note_id, col))?;
    }
    save_board(&location, &board)?;
    println!("Updated note {}", note_id);
    Ok(())
}

pub fn tui() -> Result<()> {
    let (board, location) = load_current_board()?;
    ui::run(board, location)
}

fn load_current_board() -> Result<(crate::model::Board, BoardLocation)> {
    let cwd = env::current_dir()?;
    let location = locate_board(&cwd)?;
    let board = load_board(&location)?;
    Ok((board, location))
}

fn parse_due(input: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    let raw = match input {
        Some(r) => r.trim(),
        None => return Ok(None),
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let dt = NaiveDateTime::parse_from_str(raw, "%Y.%m.%d@%H:%M")
        .map_err(|_| anyhow!("invalid date format (use YYYY.MM.DD@hh:mm): {}", raw))?;
    return Ok(Some(Utc.from_utc_datetime(&dt)));
}

fn format_due(dt: &DateTime<Utc>) -> String {
    dt.format("%Y.%m.%d@%H:%M").to_string()
}

fn generate_id() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(char::from)
        .collect()
}

fn print_note(note: &Note) {
    println!("  - {}: {}", note.id, note.title);
    if let Some(body) = &note.body {
        println!("    {}", body);
    }
    if !note.tags.is_empty() {
        println!("    tags: {}", note.tags.join(", "));
    }
    if let Some(due) = note.due {
        println!("    due: {}", format_due(&due));
    }
}
