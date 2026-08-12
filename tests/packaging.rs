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
        let dir = std::env::temp_dir().join(format!("musicai-pkg-{name}-{}", std::process::id()));
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

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run the bundler. The two binaries only have to exist and be copied, so the
/// command-line tool stands in for both; building the window's binary here
/// would drag a window toolkit into a test about file layout.
fn bundle(into: &Path, version: &str) -> PathBuf {
    let binary = env!("CARGO_BIN_EXE_musicai");
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
    into.join("musicai.app")
}

#[test]
fn the_bundle_has_everything_macos_looks_for() {
    let dir = Scratch::new("layout");
    let app = bundle(&dir.0, "1.2.3");

    // Launching the app runs Contents/MacOS/<CFBundleExecutable>, and Finder
    // needs the icon and PkgInfo where it expects them.
    assert!(app.join("Contents/MacOS/musicai-gui").exists());
    assert!(app.join("Contents/MacOS/musicai").exists(), "the CLI should ride along");
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
        for name in ["musicai-gui", "musicai"] {
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
    assert!(plist.contains("<string>musicai-gui</string>"));
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
        .args(["/nonexistent/musicai-gui", env!("CARGO_BIN_EXE_musicai")])
        .arg(&dir.0)
        .output()
        .unwrap();

    assert!(!output.status.success(), "a missing binary should fail the build");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no such binary"),
        "unhelpful error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.0.join("musicai.app").exists(), "a half-built app was left behind");
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

/// The `cargo build` invocations `package-macos.sh` uses, as (package, binary).
fn build_invocations() -> Vec<(String, String)> {
    let script = std::fs::read_to_string(repo().join("scripts/package-macos.sh")).unwrap();
    script
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("cargo build"))
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let after = |flag: &str| {
                words.iter().position(|w| *w == flag).map(|i| words[i + 1].to_string())
            };
            (
                after("-p").unwrap_or_else(|| panic!("no -p in: {line}")),
                after("--bin").unwrap_or_else(|| panic!("no --bin in: {line}")),
            )
        })
        .collect()
}

#[test]
fn the_packaging_script_builds_binaries_that_exist() {
    // This is the bug this test is here for: the workspace's default member is
    // the command-line tool alone, so `cargo build --bin musicai-gui` fails with
    // "no bin target named `musicai-gui` in default-run packages". Naming the
    // package is what makes it work, and nothing else in the suite compiles the
    // window, so without this the script can be broken and everything is green.
    let metadata = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(repo())
        .output()
        .expect("running cargo metadata");
    assert!(metadata.status.success());
    let text = String::from_utf8_lossy(&metadata.stdout);

    let invocations = build_invocations();
    assert_eq!(invocations.len(), 2, "expected one build per binary: {invocations:?}");

    for (package, binary) in invocations {
        // Crude but dependency-free: the manifest lists every package name and
        // every target name, and both have to be there.
        assert!(
            text.contains(&format!("\"name\":\"{package}\"")),
            "the script builds package {package}, which is not in the workspace"
        );
        assert!(
            text.contains(&format!("\"name\":\"{binary}\"")),
            "the script builds binary {binary}, which no package produces"
        );
    }
}

#[test]
fn every_build_in_the_packaging_script_names_its_package() {
    // Covered by the test above too, but stated on its own because it is the
    // single rule that keeps the script working: a bare --bin is resolved
    // against the default members, which is not where the window lives.
    for (package, binary) in build_invocations() {
        assert!(!package.is_empty(), "{binary} is built without -p");
    }
}
