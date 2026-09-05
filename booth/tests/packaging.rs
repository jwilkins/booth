//! Checks the macOS bundle is assembled correctly.
//!
//! The bundle layout is the part of packaging most likely to be quietly wrong —
//! a plist key that never got substituted, a binary in the wrong directory —
//! and it is also the part that needs no macOS tooling. `make-bundle.sh` was
//! split out of the packaging script precisely so this could run anywhere.
//!
//! What is *not* covered here: `lipo`, `codesign`, `hdiutil` and notarization.
//! Those need a Mac, and this suite does not have one.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-pkg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The repository, which is the directory above this crate's own.
/// A file to stand in for a built binary.
fn stub(dir: &Path) -> PathBuf {
    let at = dir.join("stub-binary");
    std::fs::write(&at, b"#!/bin/sh\necho stub\n").expect("writing a stub binary");
    at
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("booth is in a workspace")
        .to_path_buf()
}

/// Run the bundler.
///
/// The binaries are stubs, and deliberately: `make-bundle.sh` copies whatever
/// it is handed, so what is under test is the layout and nothing about the
/// files themselves. Handing it the real ones meant copying most of a gigabyte
/// of unstripped debug binary six times over to learn nothing — enough, on a
/// small disk, to fail the test for having no room rather than for being
/// wrong.
fn bundle(into: &Path, version: &str) -> PathBuf {
    let binary = stub(into);
    let binary = binary.to_str().expect("a temp path is utf-8");
    let output = Command::new("bash")
        .arg(repo().join("scripts/make-bundle.sh"))
        .args([binary, binary])
        .arg(into)
        .arg(version)
        .output()
        .expect("running make-bundle.sh");

    assert!(
        output.status.success(),
        "make-bundle.sh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    into.join("Booth.app")
}

#[test]
fn the_bundle_has_everything_macos_looks_for() {
    let dir = Scratch::new("layout");
    let app = bundle(&dir.0, "1.2.3");

    // Launching the app runs Contents/MacOS/<CFBundleExecutable>, and Finder
    // needs the icon and PkgInfo where it expects them.
    assert!(app.join("Contents/MacOS/booth").exists());
    assert!(app.join("Contents/MacOS/booth-cli").exists(), "the CLI should ride along");
    assert!(app.join("Contents/Resources/icon.icns").exists());
    assert!(app.join("Contents/Info.plist").exists());
    assert_eq!(std::fs::read_to_string(app.join("Contents/PkgInfo")).unwrap(), "APPL????");
}

#[test]
fn the_binaries_are_executable() {
    let dir = Scratch::new("modes");
    let app = bundle(&dir.0, "1.2.3");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["booth", "booth-cli"] {
            let mode = std::fs::metadata(app.join("Contents/MacOS").join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "{name} is not executable: {mode:o}");
        }
    }
}

#[test]
fn the_version_is_substituted_into_the_plist() {
    let dir = Scratch::new("version");
    let app = bundle(&dir.0, "9.9.9");
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();

    assert!(plist.contains("<string>9.9.9</string>"), "version not substituted");
    assert!(!plist.contains("__VERSION__"), "a placeholder survived into the bundle");
    // The executable named in the plist has to be the one that is there.
    assert!(plist.contains("<string>booth</string>"));
}

#[test]
fn rebuilding_over_an_existing_bundle_replaces_it() {
    let dir = Scratch::new("rebuild");
    let app = bundle(&dir.0, "1.0.0");

    // Something left over from a previous build must not survive, or a stale
    // binary ships inside an otherwise fresh app.
    let stray = app.join("Contents/MacOS/leftover");
    std::fs::write(&stray, b"old").unwrap();

    let app = bundle(&dir.0, "1.0.1");
    assert!(!stray.exists(), "the old bundle was not cleared out");
    assert!(std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap().contains("1.0.1"));
}

#[test]
fn a_missing_binary_is_an_error_rather_than_a_broken_app() {
    let dir = Scratch::new("missing");
    let output = Command::new("bash")
        .arg(repo().join("scripts/make-bundle.sh"))
        .args(["/nonexistent/booth", stub(&dir.0).to_str().unwrap()])
        .arg(&dir.0)
        .output()
        .unwrap();

    assert!(!output.status.success(), "a missing binary should fail the build");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no such binary"),
        "unhelpful error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.0.join("Booth.app").exists(), "a half-built app was left behind");
}

/// Every `cargo build` in the packaging script names a package and a binary
/// that this workspace actually has.
///
/// The bug this is here for: the script said `--bin musicai` with no `-p`, and
/// a bare `--bin` resolves against the default member only — so the release
/// build failed with "no bin target named `musicai` in default-run packages"
/// on a Mac, having passed everything on Linux. The bundle tests could not see
/// it, because they hand `make-bundle.sh` binaries that already exist. This
/// reads the script instead.
#[test]
fn the_packaging_script_builds_things_that_exist() {
    let script = std::fs::read_to_string(repo().join("scripts/package-macos.sh")).unwrap();
    let known = binaries();

    let mut checked = 0;
    for line in script.lines().map(str::trim) {
        if !line.starts_with("cargo build") {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let after = |flag: &str| {
            words.iter().position(|word| *word == flag).and_then(|at| words.get(at + 1)).copied()
        };
        let package = after("-p").unwrap_or_else(|| {
            panic!("this names no package, so --bin has nothing to search: {line}")
        });
        let binary = after("--bin").unwrap_or_else(|| panic!("this names no binary: {line}"));
        assert!(
            known.contains(&(package.to_string(), binary.to_string())),
            "{line}\nbuilds `{binary}` from `{package}`, which this workspace does not have: \
             {known:?}"
        );
        checked += 1;
    }
    assert_eq!(checked, 2, "the script should build the app and the command-line tool");
}

/// Every (package, binary) pair in the workspace, read out of the manifests.
fn binaries() -> Vec<(String, String)> {
    let root = std::fs::read_to_string(repo().join("Cargo.toml")).unwrap();
    let members: Vec<String> = root
        .lines()
        .find(|line| line.trim_start().starts_with("members ="))
        .map(|line| line.split('"').skip(1).step_by(2).map(str::to_string).collect())
        .expect("the workspace lists its members");

    let mut found = Vec::new();
    for member in members {
        let manifest = std::fs::read_to_string(repo().join(&member).join("Cargo.toml")).unwrap();
        let named = |section: &str| -> Vec<String> {
            let mut names = Vec::new();
            let mut inside = false;
            for line in manifest.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    inside = line == section;
                    continue;
                }
                if inside {
                    if let Some(rest) = line.strip_prefix("name = ") {
                        names.push(rest.trim_matches('"').to_string());
                    }
                }
            }
            names
        };
        let package = named("[package]").first().cloned().expect("a package has a name");
        let bins = named("[[bin]]");
        // No `[[bin]]` section means cargo's own default: one binary named
        // after the package, built from src/main.rs.
        let bins = match bins.is_empty() {
            true if repo().join(&member).join("src/main.rs").exists() => vec![package.clone()],
            _ => bins,
        };
        for bin in bins {
            found.push((package.clone(), bin));
        }
    }
    found
}

#[test]
fn the_icon_is_a_real_icns_with_the_sizes_macos_wants() {
    // A malformed .icns shows up as a blank Dock icon and nothing else, so it
    // is worth checking the container rather than trusting the generator.
    let data = std::fs::read(repo().join("packaging/macos/icon.icns")).unwrap();
    assert_eq!(&data[0..4], b"icns", "not an icns file");

    let declared = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
    assert_eq!(declared, data.len(), "the length in the header is wrong");

    let mut kinds = Vec::new();
    let mut offset = 8;
    while offset + 8 <= data.len() {
        let kind = String::from_utf8_lossy(&data[offset..offset + 4]).into_owned();
        let length = u32::from_be_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        assert!(length >= 8 && offset + length <= data.len(), "entry {kind} overruns the file");
        // Every modern entry type carries a PNG.
        assert_eq!(&data[offset + 8..offset + 12], b"\x89PNG", "entry {kind} is not a PNG");
        kinds.push(kind);
        offset += length;
    }
    assert_eq!(offset, data.len(), "trailing bytes after the last entry");

    // The Dock, the Finder list view and the Retina variants.
    for wanted in ["ic04", "ic07", "ic08", "ic09", "ic10", "ic13", "ic14"] {
        assert!(kinds.iter().any(|k| k == wanted), "missing {wanted}; have {kinds:?}");
    }
}
