use crate::model::Board;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardScope {
    Project,
    Global,
}

#[derive(Debug, Clone)]
pub struct BoardLocation {
    pub path: PathBuf,
    pub scope: BoardScope,
}

pub fn init_project_board(name: Option<String>) -> Result<BoardLocation> {
    let cwd = env::current_dir()?;
    let dir = cwd.join(".postit");
    fs::create_dir_all(&dir).context("failed to create .postit directory")?;
    let path = dir.join("board.yml");
    if !path.exists() {
        let board_name = name.unwrap_or_else(|| {
            cwd.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string()
        });
        let board = Board::default_named(board_name);
        save_board(
            &BoardLocation {
                path: path.clone(),
                scope: BoardScope::Project,
            },
            &board,
        )?;
    }
    Ok(BoardLocation {
        path,
        scope: BoardScope::Project,
    })
}

pub fn locate_board(start: &Path) -> Result<BoardLocation> {
    if let Some(project_path) = find_project_board(start) {
        return Ok(BoardLocation {
            path: project_path,
            scope: BoardScope::Project,
        });
    }
    let global_path = global_board_path()?;
    Ok(BoardLocation {
        path: global_path,
        scope: BoardScope::Global,
    })
}

pub fn load_board(location: &BoardLocation) -> Result<Board> {
    if location.path.exists() {
        let data = fs::read_to_string(&location.path)
            .with_context(|| format!("reading {:?}", location.path))?;
        let board: Board = serde_yaml::from_str(&data).context("parsing board file")?;
        Ok(board)
    } else {
        let fallback_name = match location.scope {
            BoardScope::Project => location
                .path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string(),
            BoardScope::Global => "default".to_string(),
        };
        let board = Board::default_named(fallback_name);
        save_board(location, &board)?;
        Ok(board)
    }
}

pub fn save_board(location: &BoardLocation, board: &Board) -> Result<()> {
    if let Some(parent) = location.path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {:?}", parent))?;
    }
    let serialized = serde_yaml::to_string(board).context("serializing board")?;
    fs::write(&location.path, serialized)
        .with_context(|| format!("writing {:?}", location.path))?;
    Ok(())
}

fn find_project_board(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(".postit/board.yml");
        if candidate.exists() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

fn global_board_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "postit").context("locating data directory")?;
    Ok(dirs.data_dir().join("board.yml"))
}
