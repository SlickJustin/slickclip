#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use rusqlite::Connection;

use super::migrations::apply_migrations;

#[derive(Clone)]
pub struct LibraryDatabase {
    path: PathBuf,
}

impl LibraryDatabase {
    pub fn initialize(path: PathBuf) -> Result<(Self, i64), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Could not create the Clips library data directory '{}': {error}",
                    parent.display()
                )
            })?;
        }
        let database = Self { path };
        let mut connection = database.open()?;
        let schema_version = apply_migrations(&mut connection)?;
        Ok((database, schema_version))
    }

    #[cfg(test)]
    pub fn initialize_in_memory() -> Result<(Connection, i64), String> {
        let mut connection = Connection::open_in_memory()
            .map_err(|error| format!("Could not open in-memory Clips database: {error}"))?;
        configure_connection(&connection)?;
        let version = apply_migrations(&mut connection)?;
        Ok((connection, version))
    }

    pub fn open(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.path).map_err(|error| {
            format!(
                "Could not open Clips database '{}': {error}",
                self.path.display()
            )
        })?;
        configure_connection(&connection)?;
        Ok(connection)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn configure_connection(connection: &Connection) -> Result<(), String> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("Could not configure SQLite busy timeout: {error}"))?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )
        .map_err(|error| format!("Could not configure Clips database: {error}"))
}
