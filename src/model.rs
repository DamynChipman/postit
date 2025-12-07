use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type NoteId = String;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Board {
    pub name: String,
    pub columns: Vec<Column>,
    pub notes: HashMap<NoteId, Note>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Column {
    pub id: String,
    pub name: String,
    pub wip_limit: Option<u32>,
    pub note_ids: Vec<NoteId>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    pub body: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub due: Option<DateTime<Utc>>,
}

#[derive(thiserror::Error, Debug)]
pub enum BoardError {
    #[error("column not found: {0}")]
    ColumnNotFound(String),
    #[error("note not found: {0}")]
    NoteNotFound(String),
    #[error("note {0} not present in any column")]
    NoteLocationMissing(String),
    #[error("wip limit reached for column {0}")]
    WipLimitReached(String),
}

impl Board {
    pub fn default_named(name: impl Into<String>) -> Self {
        Board {
            name: name.into(),
            columns: vec![
                Column {
                    id: "todo".into(),
                    name: "To Do".into(),
                    wip_limit: None,
                    note_ids: Vec::new(),
                },
                Column {
                    id: "doing".into(),
                    name: "Doing".into(),
                    wip_limit: None,
                    note_ids: Vec::new(),
                },
                Column {
                    id: "waiting".into(),
                    name: "Waiting".into(),
                    wip_limit: None,
                    note_ids: Vec::new(),
                },
                Column {
                    id: "done".into(),
                    name: "Done".into(),
                    wip_limit: None,
                    note_ids: Vec::new(),
                },
            ],
            notes: HashMap::new(),
        }
    }

    pub fn find_column_index(&self, id: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.id == id)
    }

    pub fn find_note_column_index(&self, note_id: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.note_ids.iter().any(|id| id == note_id))
    }

    pub fn add_note(&mut self, note: Note, column_id: &str) -> Result<(), BoardError> {
        let target_idx = self
            .find_column_index(column_id)
            .ok_or_else(|| BoardError::ColumnNotFound(column_id.to_string()))?;
        self.ensure_wip(target_idx)?;
        self.notes.insert(note.id.clone(), note.clone());
        self.columns[target_idx].note_ids.push(note.id.clone());
        Ok(())
    }

    pub fn move_note(&mut self, note_id: &str, dest_column_id: &str) -> Result<(), BoardError> {
        if !self.notes.contains_key(note_id) {
            return Err(BoardError::NoteNotFound(note_id.to_string()));
        }
        let dest_idx = self
            .find_column_index(dest_column_id)
            .ok_or_else(|| BoardError::ColumnNotFound(dest_column_id.to_string()))?;
        let src_idx = match self.find_note_column_index(note_id) {
            Some(idx) => idx,
            None => return Err(BoardError::NoteLocationMissing(note_id.to_string())),
        };
        if src_idx == dest_idx {
            return Ok(());
        }
        self.ensure_wip(dest_idx)?;
        self.columns[src_idx].note_ids.retain(|id| id != note_id);
        self.columns[dest_idx].note_ids.push(note_id.to_string());
        self.touch(note_id)?;
        Ok(())
    }

    pub fn update_note<F>(&mut self, note_id: &str, mut f: F) -> Result<(), BoardError>
    where
        F: FnMut(&mut Note),
    {
        let note = self
            .notes
            .get_mut(note_id)
            .ok_or_else(|| BoardError::NoteNotFound(note_id.to_string()))?;
        f(note);
        note.updated_at = Utc::now();
        Ok(())
    }

    fn touch(&mut self, note_id: &str) -> Result<(), BoardError> {
        self.update_note(note_id, |_| {})
    }

    fn ensure_wip(&self, column_idx: usize) -> Result<(), BoardError> {
        if let Some(limit) = self.columns[column_idx].wip_limit {
            if self.columns[column_idx].note_ids.len() as u32 >= limit {
                return Err(BoardError::WipLimitReached(
                    self.columns[column_idx].id.clone(),
                ));
            }
        }
        Ok(())
    }
}

impl Note {
    pub fn new(
        id: NoteId,
        title: String,
        body: Option<String>,
        tags: Vec<String>,
        due: Option<DateTime<Utc>>,
    ) -> Self {
        let now = Utc::now();
        Note {
            id,
            title,
            body,
            tags,
            created_at: now,
            updated_at: now,
            due,
        }
    }
}
