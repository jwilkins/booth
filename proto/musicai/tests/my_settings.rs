//! The My Settings files this writes, read by rekordcrate: a reader that
//! shares no code with the writer here, and was made from files rekordbox
//! wrote.

mod common;

use binrw::{io::Cursor, BinRead, BinWrite};
use booth_cli::commands;
use booth_cli::export::settings::{File, Kind, Settings, FIELDS};
use booth_cli::report::Collected;
use common::drive::*;
use rekordcrate::setting::Setting;

fn theirs(bytes: &[u8]) -> Setting {
    Setting::read(&mut Cursor::new(bytes)).expect("rekordcrate did not read it")
}

fn written_back(setting: &Setting) -> Vec<u8> {
    let mut bytes = Vec::new();
    setting.write(&mut Cursor::new(&mut bytes)).unwrap();
    bytes
}

/// Each setting as rekordcrate names it and the variant it read, from its
/// pretty-printed form: the one place it lists them all by name.
fn named(setting: &Setting) -> Vec<(String, String)> {
    format!("{:#?}", setting.data)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').split_once(": "))
        .filter(|(name, value)| {
            name.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                && value.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn every_value_of_every_setting_is_the_one_rekordcrate_reads_in_that_setting() {
    for field in FIELDS {
        let before = named(&theirs(File::rekordbox(field.kind).bytes()));
        for &(value, label) in field.values {
            let mut settings = Settings::rekordbox();
            settings.set(field, value);
            let ours = settings.file(field.kind).bytes();
            let read = theirs(ours);
            // Written back the same, checksum and all: the value is one of
            // the variants rekordcrate has for the byte, and nothing about
            // the file is something it would have written differently.
            assert_eq!(written_back(&read), ours, "{} = {label}", field.id);
            let after = named(&read);
            let moved: Vec<&String> = before
                .iter()
                .zip(&after)
                .filter(|(a, b)| a != b)
                .map(|(_, (name, _))| name)
                .collect();
            match value == field.rekordbox() {
                true => assert!(moved.is_empty(), "{} = {label} moved {moved:?}", field.id),
                false => assert_eq!(moved, [field.id], "{} = {label}", field.id),
            }
        }
    }
}

#[test]
fn rekordboxs_settings_are_rekordcrates_defaults_with_the_versions_rekordbox_writes() {
    for (kind, defaults) in [
        (Kind::Player, Setting::default_mysetting()),
        (Kind::Player2, Setting::default_mysetting2()),
        (Kind::Mixer, Setting::default_djmmysetting()),
        (Kind::General, Setting::default_devsetting()),
    ] {
        let ours = theirs(File::rekordbox(kind).bytes());
        assert_eq!(ours.data, defaults.data, "{}", kind.file_name());
        assert_eq!(ours.brand, defaults.brand, "{}", kind.file_name());
    }
}

/// The 74 files rekordbox 6.6.1 wrote with one setting changed in each, that
/// rekordcrate keeps as fixtures. They live in the registry copy of a
/// dependency, at a path that is this machine's, so this is pointed at them:
///
/// ```sh
/// BOOTH_REKORDCRATE_DATA=~/.cargo/registry/src/*/rekordcrate-0.3.0/data \
///     cargo test -p booth-cli --test my_settings -- --ignored
/// ```
///
/// Every one reads, its checksum holds, exactly one setting differs from
/// rekordbox's, to a value named here, and no other byte does.
#[test]
#[ignore = "needs rekordcrate's fixtures: BOOTH_REKORDCRATE_DATA"]
fn every_file_rekordbox_wrote_with_one_setting_changed_reads_as_that_one_setting() {
    let data = std::path::PathBuf::from(std::env::var("BOOTH_REKORDCRATE_DATA").unwrap());
    let mut read = 0;
    for folder in ["mysetting", "mysetting2", "djmmysetting", "devsetting"] {
        for entry in std::fs::read_dir(data.join(folder)).unwrap() {
            let found = booth_cli::export::settings::read(&entry.unwrap().path());
            assert!(found.refused.is_empty(), "{:?}", found.refused);
            let [file] = &found.files[..] else { panic!("{:?}", found.files) };
            assert!(file.unnamed().is_empty(), "{:?}", file.unnamed());
            let mut settings = Settings::rekordbox();
            settings.take(file.clone());
            let changed = settings.changed();
            assert_eq!(changed.len(), 1, "{changed:?}");
            assert!(changed[0].label(settings.get(changed[0])).is_some());
            read += 1;
        }
    }
    assert_eq!(read, 74);
}

#[test]
fn a_drive_written_with_settings_carries_them_and_a_dry_run_writes_none() {
    let scratch = Scratch::new("my-settings");
    let drive = scratch.path("USB");
    let song = write_song(&scratch, "a.flac");
    let mut settings = Settings::rekordbox();
    settings.set(FIELDS.iter().find(|f| f.id == "tempo_range").unwrap(), 0x83);

    let mut args = args_for(vec![song.clone()]);
    args.drive = Some(drive.clone());
    args.settings = settings.files().to_vec();
    args.dry_run = true;
    commands::export(&args, &Collected::new()).unwrap();
    assert!(!drive.join("PIONEER/MYSETTING.DAT").exists());

    args.dry_run = false;
    commands::export(&args, &Collected::new()).unwrap();
    for file in settings.files() {
        let landed = std::fs::read(drive.join("PIONEER").join(file.kind().file_name())).unwrap();
        assert_eq!(landed, file.bytes(), "{}", file.kind().file_name());
    }
    let shown = commands::settings(
        &booth_cli::cli::SettingsArgs { path: drive.clone() },
        &Collected::new(),
    );
    assert!(shown.is_ok());
}
