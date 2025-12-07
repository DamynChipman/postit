use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "postit", version, about = "Terminal sticky-note kanban board")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize a project board in the current directory
    Init {
        /// Optional board name
        #[arg(long)]
        name: Option<String>,
    },
    /// List notes in the current board
    List {
        /// Filter by column id
        #[arg(long)]
        column: Option<String>,
    },
    /// Add a new note
    Add {
        /// Title of the note
        title: String,
        /// Optional body/description
        #[arg(long)]
        body: Option<String>,
        /// Tags for the note (repeatable)
        #[arg(long = "tag", short = 't')]
        tags: Vec<String>,
        /// Column id to place the note (defaults to first column)
        #[arg(long)]
        column: Option<String>,
        /// Due date in YYYY.MM.DD@hh:mm format
        #[arg(long)]
        due: Option<String>,
    },
    /// Move a note to a different column
    Move {
        /// Note id to move
        note_id: String,
        /// Destination column id
        column_id: String,
    },
    /// Edit an existing note
    Edit {
        /// Note id to edit
        note_id: String,
        /// New title
        #[arg(long)]
        title: Option<String>,
        /// New body
        #[arg(long)]
        body: Option<String>,
        /// Replace tags (repeatable)
        #[arg(long = "tag", short = 't')]
        tags: Vec<String>,
        /// Clear existing tags
        #[arg(long)]
        clear_tags: bool,
        /// Move to column id
        #[arg(long)]
        column: Option<String>,
        /// Set due date (YYYY.MM.DD@hh:mm)
        #[arg(long)]
        due: Option<String>,
        /// Clear due date
        #[arg(long)]
        clear_due: bool,
    },
    /// Launch the interactive TUI
    Tui,
}
