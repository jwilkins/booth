//! Reading rekordbox's own libraries.
//!
//! Two different databases wear the same encryption and are constantly
//! confused for each other, so it is worth being exact about which is which:
//!
//! - **`master.db`** is rekordbox's library *on your computer* — every track
//!   you have ever added, with its cues, grids, playlists and play counts.
//!   Reading it is what [`master`] does, and it is how a library moves into
//!   this program without being re-analysed from scratch.
//! - **`exportLibrary.db`** is the OneLibrary database *on a USB drive*, which
//!   is what a CDJ-3000X reads instead of `export.pdb`. [`onelibrary`] can open
//!   one and describe it. That is all it can do, and the reason is in that
//!   module.
//!
//! Both are SQLCipher, and for both the key is fixed rather than derived from
//! the machine or the licence — so neither is a cryptographic problem. What
//! stands between this program and writing a drive a CDJ-3000X will play is
//! the *schema*, which is not published for OneLibrary.

pub mod master;
pub mod onelibrary;

use std::path::Path;

use anyhow::{bail, Context, Result};

/// The key this build was compiled with, if any.
///
/// Empty by default. The key is not this project's to publish, and the
/// reference implementation everyone uses — `pyrekordbox` — deliberately ships
/// without it and fetches it on demand for the same reason. Everything works
/// without filling this in: see [`resolve`] for the two ways to supply one at
/// runtime.
///
/// If you would rather have it baked into the binary, put it here. It is a
/// 64-character hex string, and it is the same for every installation.
pub const BUNDLED_KEY: &str = "";

/// Where to look for the key when none was passed in.
pub const KEY_ENV: &str = "REKORDBOX_KEY";

/// Work out which key to use.
///
/// In order: what the caller was given, the environment, then whatever this
/// build was compiled with. The error says what to do rather than that
/// something went wrong, because "no key" is the normal first-run state and
/// not a fault.
pub fn resolve(supplied: Option<&str>) -> Result<String> {
    let candidates = [
        supplied.map(str::to_string),
        std::env::var(KEY_ENV).ok(),
        (!BUNDLED_KEY.is_empty()).then(|| BUNDLED_KEY.to_string()),
    ];
    for candidate in candidates.into_iter().flatten() {
        let trimmed = candidate.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    bail!(
        "no rekordbox database key. rekordbox encrypts its library with SQLCipher, using a \
         key that is the same on every installation. Put it in Settings, or set {KEY_ENV}. \
         `python -m pyrekordbox download-key` will fetch and print one."
    )
}

/// Open an encrypted rekordbox database read-only.
///
/// The key is offered as a passphrase first, which is how rekordbox itself
/// keys these files and therefore what works; the raw-bytes reading of the
/// same hex string is tried second. Both are cheap, and trying both turns the
/// most likely mistake — a key that is right but in the other form — from an
/// unreadable "file is not a database" into something that simply works.
pub fn open(path: &Path, key: &str) -> Result<rusqlite::Connection> {
    if !path.exists() {
        bail!("{} is not there", path.display());
    }
    let mut last = None;
    for form in [Form::Passphrase, Form::RawBytes] {
        match try_open(path, key, form) {
            Ok(connection) => return Ok(connection),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("could not open {}", path.display()))).context(
        format!(
            "{} would not open. Either the key is wrong, or the file is not an encrypted \
             rekordbox database.",
            path.display()
        ),
    )
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Form {
    /// The key is text, and SQLCipher derives the real one from it.
    Passphrase,
    /// The key is the bytes that hex string spells, used as-is.
    RawBytes,
}

fn try_open(path: &Path, key: &str, form: Form) -> Result<rusqlite::Connection> {
    use rusqlite::OpenFlags;

    if form == Form::RawBytes && !is_hex_key(key) {
        bail!("not a raw key");
    }
    // Read-only, and that is not a detail. This is somebody's whole library as
    // rekordbox left it; nothing here has any business writing to it, and a
    // flag is a better guarantee of that than good intentions.
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let pragma = match form {
        Form::Passphrase => format!("PRAGMA key = '{}'", key.replace('\'', "''")),
        Form::RawBytes => format!("PRAGMA key = \"x'{key}'\""),
    };
    connection.execute_batch(&pragma)?;

    // Nothing above proves the key was right — SQLCipher does not check one,
    // it just decrypts and hands back whatever that produces. Reading a page
    // is the test, and this is the smallest read that touches one.
    connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get::<_, i64>(0))
        .context("the key did not decrypt this file")?;
    Ok(connection)
}

fn is_hex_key(key: &str) -> bool {
    key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit())
}

/// The tables in an open database, with how many rows each holds.
///
/// The first thing worth knowing about a database nobody has documented.
pub fn tables(connection: &rusqlite::Connection) -> Result<Vec<(String, i64)>> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names: Vec<String> =
        statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;

    let mut found = Vec::with_capacity(names.len());
    for name in names {
        // The name comes from `sqlite_master` in this same file, so it cannot
        // be anything but a real table name here; quoted anyway, because a
        // table called `Order` is a perfectly legal thing to be handed.
        let count = connection
            .query_row(&format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(-1);
        found.push((name, count));
    }
    Ok(found)
}

/// The column names of one table, for reading a schema that changes between
/// rekordbox versions without guessing which version this is.
pub fn columns(connection: &rusqlite::Connection, table: &str) -> Result<Vec<String>> {
    let mut statement =
        connection.prepare(&format!("PRAGMA table_info(\"{}\")", table.replace('"', "\"\"")))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<_, _>>()?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_can_come_from_the_argument_or_the_environment() {
        assert_eq!(resolve(Some("abc")).unwrap(), "abc");
        // Surrounding space is the commonest thing to paste by accident, and
        // is never part of a key.
        assert_eq!(resolve(Some("  abc  ")).unwrap(), "abc");
    }

    #[test]
    fn no_key_says_what_to_do_about_it() {
        // Only when nothing is set anywhere, which is the normal first run.
        if std::env::var(KEY_ENV).is_ok() || !BUNDLED_KEY.is_empty() {
            return;
        }
        let error = format!("{:#}", resolve(None).unwrap_err());
        assert!(error.contains("Settings"), "{error}");
        assert!(error.contains(KEY_ENV), "{error}");
    }

    #[test]
    fn a_raw_key_is_recognised_by_shape() {
        assert!(is_hex_key(&"a".repeat(64)));
        assert!(!is_hex_key(&"a".repeat(63)), "too short to be one");
        assert!(!is_hex_key(&"z".repeat(64)), "not hex");
        assert!(!is_hex_key("hunter2"));
    }

    #[test]
    fn a_file_that_is_not_there_says_so_rather_than_that_the_key_is_wrong() {
        let error = format!("{:#}", open(Path::new("/no/such/master.db"), "k").unwrap_err());
        assert!(error.contains("not there"), "{error}");
    }
}
