//! Opening a OneLibrary (Device Library Plus) drive, and describing what is in
//! it.
//!
//! # What this is, and what it is not
//!
//! `exportLibrary.db` is the database a CDJ-3000X reads instead of
//! `export.pdb`. It is SQLCipher, like `master.db`, and like `master.db` its
//! key is fixed rather than derived from the machine — so opening one is not a
//! cryptographic problem and never was.
//!
//! It is tempting to conclude from that that support for the newer players is
//! a matter of having the key. It is not, and it is not the schema either any
//! more: AlphaTheta has published nothing, but the twenty-two tables, their
//! DDL from a real export, and the rows a player's browse screen is drawn from
//! have all been documented by other people. `docs/onelibrary.md` is that
//! survey, with its sources.
//!
//! What is missing is evidence. No published test shows a drive carrying only
//! a hand-written `exportLibrary.db` playing on a player that reads only
//! OneLibrary, or records whether such a player used the grids and waveforms
//! it was handed rather than measuring its own. And writing the file means
//! using a key recovered from somebody else's binary, which is a decision this
//! project has not made.
//!
//! So this module reads and describes. Writing one is
//! [`crate::export::onelibrary`]'s job, under the drive key rather than this
//! one;
//! being able to open a drive that rekordbox wrote is how what that writes
//! gets checked against what rekordbox writes.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Where rekordbox puts the OneLibrary database on a drive, in the order worth
/// looking.
const PLACES: [&str; 3] =
    ["PIONEER/rekordbox/exportLibrary.db", "PIONEER/exportLibrary.db", "exportLibrary.db"];

/// Find the OneLibrary database on a mounted drive, if it has one.
pub fn find(drive: &Path) -> Option<PathBuf> {
    PLACES.iter().map(|place| drive.join(place)).find(|path| path.exists())
}

/// What one table looks like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableShape {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: i64,
}

/// Every table in a OneLibrary database, with its columns and row count.
///
/// The first thing worth knowing about an undocumented format, and the thing
/// that has to be written down before anything can be written out.
pub fn describe(connection: &rusqlite::Connection) -> Result<Vec<TableShape>> {
    super::tables(connection)?
        .into_iter()
        .map(|(name, rows)| {
            let columns = super::columns(connection, &name).unwrap_or_default();
            Ok(TableShape { name, columns, rows })
        })
        .collect()
}

/// The description as text, for putting in a bug report or a notes file.
pub fn report(shapes: &[TableShape]) -> String {
    let mut out = format!("{} tables\n", shapes.len());
    for shape in shapes {
        out.push_str(&format!("\n{} ({} rows)\n", shape.name, shape.rows));
        for column in &shape.columns {
            out.push_str(&format!("    {column}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_without_one_says_so_rather_than_guessing_a_path() {
        let dir = std::env::temp_dir().join(format!("musicai-onelib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(find(&dir), None);

        let nested = dir.join("PIONEER/rekordbox");
        std::fs::create_dir_all(&nested).unwrap();
        let db = nested.join("exportLibrary.db");
        std::fs::write(&db, b"").unwrap();
        assert_eq!(find(&dir), Some(db));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_description_reads_as_notes_rather_than_as_a_dump() {
        let shapes = vec![
            TableShape {
                name: "content".into(),
                columns: vec!["id".into(), "title".into()],
                rows: 412,
            },
            TableShape { name: "playlist".into(), columns: vec!["id".into()], rows: 9 },
        ];
        let text = report(&shapes);
        assert!(text.starts_with("2 tables"));
        assert!(text.contains("content (412 rows)"));
        assert!(text.contains("    title"));
    }
}
