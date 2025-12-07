mod cli;
mod commands;
mod model;
mod storage;
mod ui;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let args = cli::Cli::parse();
    let command = args.command.unwrap_or(cli::Command::Tui);
    match command {
        cli::Command::Init { name } => commands::init(name),
        cli::Command::List { column } => commands::list(column),
        cli::Command::Add {
            title,
            body,
            tags,
            column,
            due,
        } => commands::add(title, body, tags, column, due),
        cli::Command::Move { note_id, column_id } => commands::move_note(note_id, column_id),
        cli::Command::Edit {
            note_id,
            title,
            body,
            tags,
            clear_tags,
            column,
            due,
            clear_due,
        } => commands::edit(
            note_id, title, body, tags, clear_tags, column, due, clear_due,
        ),
        cli::Command::Tui => commands::tui(),
    }
}
