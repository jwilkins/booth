//! My Settings: what the players and the mixer are set to, in the four files
//! rekordbox puts in a drive's `PIONEER` folder.
//!
//! `MYSETTING.DAT` and `MYSETTING2.DAT` hold the player's DJ and display
//! settings, `DJMMYSETTING.DAT` the mixer's, and `DEVSETTING.DAT` the waveform
//! and key display rekordbox keeps on its General page. A player takes them
//! off a stick with LOAD under MY SETTINGS, and puts its own on one with SAVE.
//!
//! Each file is a header naming what wrote it, a body of one byte a setting,
//! and a CRC16-XMODEM checksum. What each byte means is rekordcrate's work
//! (`rekordcrate::setting`, MPL-2.0), and it was checked against the 74 files
//! rekordcrate keeps from rekordbox 6.6.1 with one setting changed in each:
//! every one differs from rekordbox's own default file in the byte and to the
//! value written here, and in nothing else but the checksum. pyrekordbox's
//! tables disagree in two places, hot cue auto load's "rekordbox setting" and
//! the languages, and the files side with rekordcrate. All 82 files' checksums
//! are the CRC of the body, or of everything before it for the mixer's file.
//!
//! A file is kept as its bytes, and only the bytes of settings named here are
//! ever changed. rekordcrate reads a file into fields and refuses one with
//! anything unexpected where no setting has been named yet, and a file a
//! player saved, or a newer rekordbox wrote, is exactly where a setting nobody
//! has named would turn up. Written back from fields, it would be gone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// The length field, three names of 32 bytes each, and the body's length.
const HEADER: usize = 104;
/// The checksum and the two zero bytes after it.
const FOOTER: usize = 4;

/// Which of the four files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// `MYSETTING.DAT`: most of the player's settings.
    Player,
    /// `MYSETTING2.DAT`: the player settings that came later.
    Player2,
    /// `DJMMYSETTING.DAT`: the mixer's.
    Mixer,
    /// `DEVSETTING.DAT`: waveform and key display.
    General,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Player, Kind::Player2, Kind::Mixer, Kind::General];

    pub fn file_name(self) -> &'static str {
        match self {
            Kind::Player => "MYSETTING.DAT",
            Kind::Player2 => "MYSETTING2.DAT",
            Kind::Mixer => "DJMMYSETTING.DAT",
            Kind::General => "DEVSETTING.DAT",
        }
    }

    /// Where it goes on a drive, as the player sees it.
    pub fn on_drive(self) -> String {
        format!("/PIONEER/{}", self.file_name())
    }

    pub fn from_file_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.file_name().eq_ignore_ascii_case(name))
    }

    fn body_len(self) -> usize {
        self.rekordbox_body().len()
    }

    fn len(self) -> usize {
        HEADER + self.body_len() + FOOTER
    }

    /// Where the checksummed bytes start. The mixer's file counts its header
    /// too; the others count only the body.
    fn checksum_from(self) -> usize {
        match self {
            Kind::Mixer => 0,
            _ => HEADER,
        }
    }

    /// The brand and version rekordbox 6.6.1 writes in the header. The
    /// version is not rekordbox's own but the file's: 0.001 for both player
    /// files and 1.000 for the mixer's, measured, where rekordcrate's
    /// defaults write 6.6.1 in all four.
    fn rekordbox_header(self) -> (&'static str, &'static str) {
        match self {
            Kind::Player | Kind::Player2 => ("PIONEER", "0.001"),
            Kind::Mixer => ("PioneerDJ", "1.000"),
            Kind::General => ("PIONEER DJ", "6.6.1"),
        }
    }

    /// The body of the file rekordbox 6.6.1 puts on a drive that nobody has
    /// changed a setting for.
    fn rekordbox_body(self) -> &'static [u8] {
        match self {
            Kind::Player => &[
                0x78, 0x56, 0x34, 0x12, 0x02, 0x00, 0x00, 0x00, 0x81, 0x83, 0x81, 0x88, 0x81, 0x01,
                0x82, 0x81, 0x81, 0x01, 0x01, 0x01, 0x82, 0x80, 0x80, 0x81, 0x80, 0x81, 0x80, 0x00,
                0x00, 0x81, 0x00, 0x00, 0x81, 0x81, 0x81, 0x80, 0x81, 0x80, 0x00, 0x00,
            ],
            Kind::Player2 => &[
                0x81, 0x80, 0x83, 0x83, 0x81, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x81, 0x85, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
            Kind::Mixer => &[
                0x78, 0x56, 0x34, 0x12, 0x01, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x81, 0x82,
                0x80, 0x80, 0x81, 0x81, 0x80, 0x81, 0x80, 0x80, 0x85, 0x82, 0x80, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
            Kind::General => &[
                0x78, 0x56, 0x34, 0x12, 0x01, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00,
            ],
        }
    }
}

/// Where a setting is found in rekordbox's preferences, which is how the
/// editor groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    PlayerDj,
    PlayerLcd,
    PlayerIndicator,
    MixerDj,
    MixerBrightness,
    General,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::PlayerDj,
        Section::PlayerLcd,
        Section::PlayerIndicator,
        Section::MixerDj,
        Section::MixerBrightness,
        Section::General,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::PlayerDj => "Player · DJ setting",
            Section::PlayerLcd => "Player · Display (LCD)",
            Section::PlayerIndicator => "Player · Display (indicator)",
            Section::MixerDj => "Mixer · DJ setting",
            Section::MixerBrightness => "Mixer · Brightness",
            Section::General => "Waveform and key",
        }
    }
}

/// One setting: which byte of which file, and what its values mean.
#[derive(Debug, PartialEq, Eq)]
pub struct Field {
    pub kind: Kind,
    /// Where in the body.
    pub at: usize,
    /// rekordcrate's name for it, kept so the two can be checked against each
    /// other.
    pub id: &'static str,
    pub section: Section,
    /// As rekordbox's preferences name it.
    pub name: &'static str,
    /// Every value known, as rekordbox names it.
    pub values: &'static [(u8, &'static str)],
}

impl Field {
    /// What a byte means here, or `None` for a value nobody has named.
    pub fn label(&self, value: u8) -> Option<&'static str> {
        self.values.iter().find(|(known, _)| *known == value).map(|(_, label)| *label)
    }

    /// What rekordbox sets it to when nobody has changed it.
    pub fn rekordbox(&self) -> u8 {
        self.kind.rekordbox_body()[self.at]
    }
}

const OFF_ON: &[(u8, &str)] = &[(0x80, "Off"), (0x81, "On")];
const OFF_DARK_BRIGHT: &[(u8, &str)] = &[(0x80, "Off"), (0x81, "1 (Dark)"), (0x82, "2 (Bright)")];
const UNLOCK_LOCK: &[(u8, &str)] = &[(0x80, "Unlock"), (0x81, "Lock")];
const ONE_TO_FIVE: &[(u8, &str)] =
    &[(0x81, "1"), (0x82, "2"), (0x83, "3"), (0x84, "4"), (0x85, "5")];

macro_rules! field {
    ($kind:ident, $at:expr, $id:literal, $section:ident, $name:literal, $values:expr) => {
        Field {
            kind: Kind::$kind,
            at: $at,
            id: $id,
            section: Section::$section,
            name: $name,
            values: $values,
        }
    };
}

/// Every setting Booth knows, in the order the editor shows them within a
/// section.
pub const FIELDS: &[Field] = &[
    field!(
        Player,
        23,
        "play_mode",
        PlayerDj,
        "Play Mode / Auto Play Mode",
        &[(0x80, "Continue / On"), (0x81, "Single / Off"),]
    ),
    field!(Player, 21, "eject_lock", PlayerDj, "Eject/Load Lock", UNLOCK_LOCK),
    field!(Player, 29, "needle_lock", PlayerDj, "Needle Lock", UNLOCK_LOCK),
    field!(Player, 10, "quantize", PlayerDj, "Quantize", OFF_ON),
    field!(
        Player,
        24,
        "quantize_beat_value",
        PlayerDj,
        "Quantize Beat Value",
        &[(0x80, "1 Beat"), (0x81, "1/2 Beat"), (0x82, "1/4 Beat"), (0x83, "1/8 Beat"),]
    ),
    field!(Player, 22, "sync", PlayerDj, "Sync", OFF_ON),
    field!(
        Player,
        25,
        "hotcue_autoload",
        PlayerDj,
        "Hot Cue Auto Load",
        &[(0x80, "Off"), (0x81, "On"), (0x82, "rekordbox Setting"),]
    ),
    field!(Player, 26, "hotcue_color", PlayerDj, "Hot Cue Color", OFF_ON),
    field!(Player, 34, "auto_cue", PlayerDj, "Auto Cue", OFF_ON),
    field!(
        Player,
        11,
        "auto_cue_level",
        PlayerDj,
        "Auto Cue Level",
        &[
            (0x80, "-36dB"),
            (0x81, "-42dB"),
            (0x82, "-48dB"),
            (0x83, "-54dB"),
            (0x84, "-60dB"),
            (0x85, "-66dB"),
            (0x86, "-72dB"),
            (0x87, "-78dB"),
            (0x88, "Memory"),
        ]
    ),
    field!(Player, 32, "time_mode", PlayerDj, "Time Mode", &[(0x80, "Elapsed"), (0x81, "Remain")]),
    field!(Player, 33, "jog_mode", PlayerDj, "Jog Mode", &[(0x80, "CDJ"), (0x81, "Vinyl")]),
    field!(
        Player2,
        0,
        "vinyl_speed_adjust",
        PlayerDj,
        "Vinyl Speed Adjust",
        &[(0x80, "Touch & Release"), (0x81, "Touch"), (0x82, "Release"),]
    ),
    field!(
        Player,
        36,
        "tempo_range",
        PlayerDj,
        "Tempo Range",
        &[(0x80, "±6%"), (0x81, "±10%"), (0x82, "±16%"), (0x83, "Wide"),]
    ),
    field!(Player, 35, "master_tempo", PlayerDj, "Master Tempo", OFF_ON),
    field!(
        Player,
        37,
        "phase_meter",
        PlayerDj,
        "Phase Meter",
        &[(0x80, "Type 1"), (0x81, "Type 2"),]
    ),
    field!(
        Player2,
        10,
        "waveform",
        PlayerDj,
        "Waveform / Phase Meter",
        &[(0x80, "Waveform"), (0x81, "Phase Meter"),]
    ),
    field!(
        Player2,
        4,
        "waveform_divisions",
        PlayerDj,
        "Waveform Divisions",
        &[(0x80, "Time Scale"), (0x81, "Phrase"),]
    ),
    field!(
        Player2,
        12,
        "beat_jump_beat_value",
        PlayerDj,
        "Beat Jump Beat Value",
        &[
            (0x80, "1/2 Beat"),
            (0x81, "1 Beat"),
            (0x82, "2 Beat"),
            (0x83, "4 Beat"),
            (0x84, "8 Beat"),
            (0x85, "16 Beat"),
            (0x86, "32 Beat"),
            (0x87, "64 Beat"),
        ]
    ),
    field!(
        Player,
        12,
        "language",
        PlayerLcd,
        "Language",
        &[
            (0x81, "English"),
            (0x82, "Français"),
            (0x83, "Deutsch"),
            (0x84, "Italiano"),
            (0x85, "Nederlands"),
            (0x86, "Español"),
            (0x87, "Русский"),
            (0x88, "한국어"),
            (0x89, "简体中文"),
            (0x8a, "繁體中文"),
            (0x8b, "日本語"),
            (0x8c, "Português"),
            (0x8d, "Svenska"),
            (0x8e, "Čeština"),
            (0x8f, "Magyar"),
            (0x90, "Dansk"),
            (0x91, "Ελληνικά"),
            (0x92, "Türkçe"),
        ]
    ),
    field!(Player, 9, "lcd_brightness", PlayerLcd, "LCD Brightness", ONE_TO_FIVE),
    field!(Player2, 3, "jog_lcd_brightness", PlayerLcd, "Jog LCD Brightness", ONE_TO_FIVE),
    field!(
        Player2,
        1,
        "jog_display_mode",
        PlayerLcd,
        "Jog Display Mode",
        &[(0x80, "Auto"), (0x81, "Info"), (0x82, "Simple"), (0x83, "Artwork"),]
    ),
    field!(Player, 16, "slip_flashing", PlayerIndicator, "Slip Flashing", OFF_ON),
    field!(Player, 8, "on_air_display", PlayerIndicator, "On Air Display", OFF_ON),
    field!(
        Player,
        14,
        "jog_ring_brightness",
        PlayerIndicator,
        "Jog Ring Brightness",
        OFF_DARK_BRIGHT
    ),
    field!(Player, 15, "jog_ring_indicator", PlayerIndicator, "Jog Ring Indicator", OFF_ON),
    field!(
        Player,
        20,
        "disc_slot_illumination",
        PlayerIndicator,
        "Disc Slot Illumination",
        OFF_DARK_BRIGHT
    ),
    field!(
        Player2,
        2,
        "pad_button_brightness",
        PlayerIndicator,
        "Pad/Button Brightness",
        &[(0x81, "1"), (0x82, "2"), (0x83, "3"), (0x84, "4"),]
    ),
    field!(
        Mixer,
        12,
        "channel_fader_curve",
        MixerDj,
        "Channel Fader Curve",
        &[(0x80, "Steep Top"), (0x81, "Linear"), (0x82, "Steep Bottom"),]
    ),
    field!(
        Mixer,
        24,
        "channel_fader_curve_long_fader",
        MixerDj,
        "Channel Fader Curve (Long Fader)",
        &[(0x80, "Exponential"), (0x81, "Smooth"), (0x82, "Linear")]
    ),
    field!(
        Mixer,
        13,
        "crossfader_curve",
        MixerDj,
        "Crossfader Curve",
        &[(0x80, "Constant Power"), (0x81, "Slow Cut"), (0x82, "Fast Cut"),]
    ),
    field!(
        Mixer,
        14,
        "headphones_pre_eq",
        MixerDj,
        "Headphones Pre EQ",
        &[(0x80, "Post EQ"), (0x81, "Pre EQ"),]
    ),
    field!(
        Mixer,
        15,
        "headphones_mono_split",
        MixerDj,
        "Headphones Mono Split",
        &[(0x80, "Stereo"), (0x81, "Mono Split"),]
    ),
    field!(Mixer, 16, "beat_fx_quantize", MixerDj, "Beat FX Quantize", OFF_ON),
    field!(
        Mixer,
        17,
        "mic_low_cut",
        MixerDj,
        "Mic Low Cut",
        &[(0x80, "Off"), (0x81, "On (for MC)")]
    ),
    field!(
        Mixer,
        18,
        "talk_over_mode",
        MixerDj,
        "Talk Over Mode",
        &[(0x80, "Advanced"), (0x81, "Normal"),]
    ),
    field!(
        Mixer,
        19,
        "talk_over_level",
        MixerDj,
        "Talk Over Level",
        &[(0x80, "-24dB"), (0x81, "-18dB"), (0x82, "-12dB"), (0x83, "-6dB"),]
    ),
    field!(
        Mixer,
        20,
        "midi_channel",
        MixerDj,
        "MIDI Channel",
        &[
            (0x80, "1"),
            (0x81, "2"),
            (0x82, "3"),
            (0x83, "4"),
            (0x84, "5"),
            (0x85, "6"),
            (0x86, "7"),
            (0x87, "8"),
            (0x88, "9"),
            (0x89, "10"),
            (0x8a, "11"),
            (0x8b, "12"),
            (0x8c, "13"),
            (0x8d, "14"),
            (0x8e, "15"),
            (0x8f, "16"),
        ]
    ),
    field!(
        Mixer,
        21,
        "midi_button_type",
        MixerDj,
        "MIDI Button Type",
        &[(0x80, "Toggle"), (0x81, "Trigger"),]
    ),
    field!(
        Mixer,
        22,
        "display_brightness",
        MixerBrightness,
        "Display",
        &[(0x80, "White"), (0x81, "1"), (0x82, "2"), (0x83, "3"), (0x84, "4"), (0x85, "5"),]
    ),
    field!(
        Mixer,
        23,
        "indicator_brightness",
        MixerBrightness,
        "Indicator",
        &[(0x80, "1"), (0x81, "2"), (0x82, "3"),]
    ),
    // The one file whose values count from 1 rather than 0x80.
    field!(
        General,
        10,
        "waveform_color",
        General,
        "Waveform Color",
        &[(0x01, "Blue"), (0x03, "RGB"), (0x04, "3Band"),]
    ),
    field!(
        General,
        13,
        "waveform_current_position",
        General,
        "Waveform Current Position",
        &[(0x01, "Center"), (0x02, "Left"),]
    ),
    field!(
        General,
        9,
        "overview_waveform_type",
        General,
        "Overview Waveform",
        &[(0x01, "Half Waveform"), (0x02, "Full Waveform"),]
    ),
    field!(
        General,
        12,
        "key_display_format",
        General,
        "Key Display Format",
        &[(0x01, "Classic"), (0x02, "Alphanumeric"),]
    ),
];

/// The checksum every one of these files ends with.
fn crc16_xmodem(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0, |crc, &byte| {
        (0..8).fold(crc ^ (u16::from(byte) << 8), |crc, _| match crc & 0x8000 {
            0 => crc << 1,
            _ => (crc << 1) ^ 0x1021,
        })
    })
}

/// One settings file, every byte of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    kind: Kind,
    bytes: Vec<u8>,
}

impl File {
    /// The file rekordbox 6.6.1 writes when nothing has been changed.
    pub fn rekordbox(kind: Kind) -> File {
        let (brand, version) = kind.rekordbox_header();
        let mut bytes = Vec::with_capacity(kind.len());
        bytes.extend_from_slice(&0x60u32.to_le_bytes());
        for name in [brand, "rekordbox", version] {
            let mut field = [0u8; 32];
            field[..name.len()].copy_from_slice(name.as_bytes());
            bytes.extend_from_slice(&field);
        }
        bytes.extend_from_slice(&(kind.body_len() as u32).to_le_bytes());
        bytes.extend_from_slice(kind.rekordbox_body());
        bytes.extend_from_slice(&[0; FOOTER]);
        let mut file = File { kind, bytes };
        file.seal();
        file
    }

    /// Take a file as it was found, if it is one: the right length, its
    /// lengths where rekordbox puts them, and its checksum right. Nothing else
    /// about it is asked, so a file a player wrote with a name or a byte
    /// rekordbox would not have is taken as it is.
    pub fn parse(kind: Kind, bytes: Vec<u8>) -> Result<File> {
        let name = kind.file_name();
        if bytes.len() != kind.len() {
            bail!("{name} is {} bytes, where rekordbox writes {}", bytes.len(), kind.len());
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        if word(0) != 0x60 || word(HEADER - 4) as usize != kind.body_len() {
            bail!("{name} does not have its lengths where rekordbox puts them");
        }
        let file = File { kind, bytes };
        if file.checksum() != file.stored_checksum() {
            bail!(
                "{name}'s checksum does not match what it holds, so it was damaged or written by \
                 something that got the format wrong"
            );
        }
        Ok(file)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The setting's byte.
    pub fn get(&self, at: usize) -> u8 {
        self.bytes[HEADER + at]
    }

    /// Change one setting's byte, and the checksum with it.
    pub fn set(&mut self, at: usize, value: u8) {
        self.bytes[HEADER + at] = value;
        self.seal();
    }

    /// The brand, the software and the version, as the header names them.
    pub fn header(&self) -> [String; 3] {
        [4, 36, 68].map(|from| {
            let name = &self.bytes[from..from + 32];
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            String::from_utf8_lossy(&name[..end]).into_owned()
        })
    }

    /// Bytes in the body that are not a setting named here and are not what
    /// rekordbox writes there, as (where, rekordbox's, this file's).
    ///
    /// Empty for every file rekordbox 6.6.1 writes. Anything here is a setting
    /// somebody has yet to name, and is the reason a file is kept as bytes.
    pub fn unnamed(&self) -> Vec<(usize, u8, u8)> {
        let theirs = self.kind.rekordbox_body();
        (0..self.kind.body_len())
            .filter(|&at| !FIELDS.iter().any(|f| f.kind == self.kind && f.at == at))
            .filter(|&at| self.get(at) != theirs[at])
            .map(|at| (at, theirs[at], self.get(at)))
            .collect()
    }

    /// The SHA-256 of the whole file, which is how a drive's record says what
    /// it last put there.
    pub fn digest(&self) -> String {
        digest(&self.bytes)
    }

    fn checksum(&self) -> u16 {
        crc16_xmodem(&self.bytes[self.kind.checksum_from()..self.bytes.len() - FOOTER])
    }

    fn stored_checksum(&self) -> u16 {
        let at = self.bytes.len() - FOOTER;
        u16::from_le_bytes([self.bytes[at], self.bytes[at + 1]])
    }

    fn seal(&mut self) {
        let at = self.bytes.len() - FOOTER;
        let checksum = self.checksum().to_le_bytes();
        self.bytes[at..at + 2].copy_from_slice(&checksum);
    }
}

/// The SHA-256 of some bytes, as hex.
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A whole set: one of each file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    files: [File; 4],
}

impl Default for Settings {
    fn default() -> Self {
        Settings::rekordbox()
    }
}

impl Settings {
    /// What rekordbox puts on a drive when nobody has changed anything.
    pub fn rekordbox() -> Settings {
        Settings { files: Kind::ALL.map(File::rekordbox) }
    }

    pub fn file(&self, kind: Kind) -> &File {
        &self.files[kind as usize]
    }

    pub fn files(&self) -> &[File] {
        &self.files
    }

    pub fn get(&self, field: &Field) -> u8 {
        self.file(field.kind).get(field.at)
    }

    pub fn set(&mut self, field: &Field, value: u8) {
        self.files[field.kind as usize].set(field.at, value);
    }

    /// Put a file in place of the one of its kind: an import.
    pub fn take(&mut self, file: File) {
        let at = file.kind as usize;
        self.files[at] = file;
    }

    /// The settings that are not what rekordbox would write.
    pub fn changed(&self) -> Vec<&'static Field> {
        FIELDS.iter().filter(|field| self.get(field) != field.rekordbox()).collect()
    }
}

/// Kept in a collection as the files' names and their bytes in hex: a
/// setting nobody has named is a byte, and a byte is what has to survive.
impl Serialize for Settings {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        self.files
            .iter()
            .map(|file| (file.kind.file_name(), hex(&file.bytes)))
            .collect::<BTreeMap<_, _>>()
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let kept = BTreeMap::<String, String>::deserialize(deserializer)?;
        let mut settings = Settings::rekordbox();
        for (name, hex) in kept {
            let kind = Kind::from_file_name(&name)
                .ok_or_else(|| D::Error::custom(format!("{name} is not a settings file")))?;
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|at| hex.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(|| D::Error::custom(format!("{name} is not kept as hex")))?;
            settings.take(File::parse(kind, bytes).map_err(D::Error::custom)?);
        }
        Ok(settings)
    }
}

/// The folder a drive keeps its settings in, given the drive or the folder.
pub fn folder(path: &Path) -> PathBuf {
    let pioneer = path.join("PIONEER");
    match pioneer.is_dir() {
        true => pioneer,
        false => path.to_path_buf(),
    }
}

/// What a folder held.
#[derive(Debug, Default)]
pub struct Found {
    /// The files that read as settings files.
    pub files: Vec<File>,
    /// The ones that are there and do not, and why.
    pub refused: Vec<(Kind, String)>,
}

/// The settings files in a drive's `PIONEER` folder, or in the folder named.
pub fn read(path: &Path) -> Found {
    let folder = folder(path);
    let mut found = Found::default();
    for kind in Kind::ALL {
        let at = folder.join(kind.file_name());
        let bytes = match std::fs::read(&at) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                found.refused.push((kind, format!("{} could not be read: {e}", at.display())));
                continue;
            }
        };
        match File::parse(kind, bytes) {
            Ok(file) => found.files.push(file),
            Err(e) => found.refused.push((kind, e.to_string())),
        }
    }
    found
}

/// What a write does about one settings file, given what the drive has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The drive has none, so it goes on.
    Write,
    /// The drive has this one already.
    Same,
    /// The drive has the one Booth put there last time, and the set has
    /// changed since.
    Replace,
    /// The drive has one of its own, a player's or rekordbox's, and keeps it.
    Keep,
    /// The drive has one of its own, and was told to take this one instead.
    Over,
}

impl Step {
    /// Whether the file is written.
    pub fn writes(self) -> bool {
        matches!(self, Step::Write | Step::Replace | Step::Over)
    }

    /// What it does, in a few words.
    pub fn describe(self) -> &'static str {
        match self {
            Step::Write => "goes on",
            Step::Same => "already on it",
            Step::Replace => "replaces the one Booth put there",
            Step::Keep => "the drive's own is kept",
            Step::Over => "goes on over the drive's own, as asked",
        }
    }
}

/// What to do with `ours` on a drive holding `there`, when what Booth last
/// put there had the digest `last`.
///
/// A file on the drive is the drive's own unless it is byte for byte what
/// Booth last put there. A player saving its settings, or rekordbox exporting,
/// changes it, and from then on it is somebody's choice that a write does not
/// get to undo: rekordbox does not write over one either. `over` is somebody
/// saying it may.
pub fn step(ours: &File, there: Option<&[u8]>, last: Option<&str>, over: bool) -> Step {
    match there {
        None => Step::Write,
        Some(there) if there == ours.bytes() => Step::Same,
        Some(there) if last == Some(digest(there).as_str()) => Step::Replace,
        Some(_) if over => Step::Over,
        Some(_) => Step::Keep,
    }
}

/// Read the settings files at `path` and say what is in them, setting by
/// setting, for `booth-cli settings`.
pub fn describe(path: &Path) -> Result<Vec<(String, Vec<String>)>> {
    let files: Vec<(String, Result<File>)> = match path.is_file() {
        true => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let kind = Kind::from_file_name(name).with_context(|| {
                format!(
                    "{} is not one of {}",
                    path.display(),
                    Kind::ALL.map(Kind::file_name).join(", ")
                )
            })?;
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            vec![(path.display().to_string(), File::parse(kind, bytes))]
        }
        false => {
            let found = read(path);
            if found.files.is_empty() && found.refused.is_empty() {
                bail!(
                    "{} holds no settings files: point at a drive, its PIONEER folder, or one of \
                     the files",
                    path.display()
                );
            }
            let folder = folder(path);
            let shown = |kind: Kind| folder.join(kind.file_name()).display().to_string();
            found
                .files
                .into_iter()
                .map(|file| (shown(file.kind), Ok(file)))
                .chain(
                    found
                        .refused
                        .into_iter()
                        .map(|(kind, why)| (shown(kind), Err(anyhow::anyhow!(why)))),
                )
                .collect()
        }
    };
    Ok(files
        .into_iter()
        .map(|(shown, file)| {
            let lines = match file {
                Err(e) => vec![format!("not read: {e}")],
                Ok(file) => {
                    let [brand, software, version] = file.header();
                    let mut lines = vec![format!("written as {brand} {software} {version}")];
                    for field in FIELDS.iter().filter(|f| f.kind == file.kind) {
                        let value = file.get(field.at);
                        let named = field
                            .label(value)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("{value:#04x}, a value nobody has named"));
                        lines.push(match value == field.rekordbox() {
                            true => format!("{}: {named}", field.name),
                            false => format!(
                                "{}: {named}  (rekordbox's: {})",
                                field.name,
                                field.label(field.rekordbox()).unwrap_or("?")
                            ),
                        });
                    }
                    for (at, theirs, ours) in file.unnamed() {
                        lines.push(format!(
                            "byte {at} of the body is {ours:#04x}, where rekordbox writes \
                             {theirs:#04x}: a setting nobody has named"
                        ));
                    }
                    lines
                }
            };
            (shown, lines)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    /// The bodies and checksums of the four files rekordbox 6.6.1 put on an
    /// empty drive, copied from rekordcrate's `data/complete_export/empty`.
    const REKORDBOX: [(Kind, &str); 4] = [
        (Kind::Player, "78563412020000008183818881018281810101018280808180818000008100008181818081800000487a0000"),
        (Kind::Player2, "8180838381000000000080818500000000000000000000000000000000000000000000000000000040bb0000"),
        (Kind::Mixer, "78563412010000002000000081828080818180818080858280000000000000000000000000000000000000000000000000000000386c0000"),
        (Kind::General, "785634120100000001010101010100000000000000000000000000000000000016d00000"),
    ];

    #[test]
    fn the_files_booth_starts_from_are_byte_for_byte_the_ones_rekordbox_writes() {
        for (kind, body) in REKORDBOX {
            let file = File::rekordbox(kind);
            assert_eq!(&file.bytes()[HEADER..], hex(body), "{}", kind.file_name());
            assert_eq!(file.header()[1], "rekordbox");
            assert!(file.unnamed().is_empty());
        }
        assert_eq!(File::rekordbox(Kind::Player).header()[2], "0.001");
        assert_eq!(File::rekordbox(Kind::Mixer).header()[0], "PioneerDJ");
    }

    #[test]
    fn every_setting_has_a_byte_of_its_own_and_rekordboxs_value_is_one_it_names() {
        for field in FIELDS {
            assert!(field.at < field.kind.body_len(), "{}", field.id);
            assert!(field.label(field.rekordbox()).is_some(), "{}", field.id);
            let same = FIELDS.iter().filter(|f| f.kind == field.kind && f.at == field.at);
            assert_eq!(same.count(), 1, "{}", field.id);
        }
        assert_eq!(FIELDS.len(), 46);
    }

    #[test]
    fn a_setting_changes_its_own_byte_and_the_checksum_and_nothing_else() {
        let quantize = FIELDS.iter().find(|f| f.id == "quantize").unwrap();
        let mut settings = Settings::rekordbox();
        settings.set(quantize, 0x80);
        let before = File::rekordbox(Kind::Player);
        let after = settings.file(Kind::Player);
        let differ: Vec<usize> =
            (0..before.bytes().len()).filter(|&i| before.bytes()[i] != after.bytes()[i]).collect();
        assert_eq!(differ[0], HEADER + 10);
        assert!(differ[1..].iter().all(|&i| i >= before.bytes().len() - FOOTER));
        assert_eq!(settings.changed(), [quantize]);
        assert!(File::parse(Kind::Player, after.bytes().to_vec()).is_ok(), "its checksum holds");
    }

    #[test]
    fn a_file_with_a_byte_nobody_has_named_keeps_it_through_an_edit() {
        let mut bytes = File::rekordbox(Kind::Player2).bytes().to_vec();
        bytes[HEADER + 20] = 0x83;
        let mut file = File::rekordbox(Kind::Player2);
        file.bytes = bytes;
        file.seal();
        let mut settings = Settings::rekordbox();
        settings.take(File::parse(Kind::Player2, file.bytes().to_vec()).unwrap());
        let beat_jump = FIELDS.iter().find(|f| f.id == "beat_jump_beat_value").unwrap();
        settings.set(beat_jump, 0x87);
        let kept = settings.file(Kind::Player2);
        assert_eq!(kept.unnamed(), [(20, 0x00, 0x83)]);
        assert_eq!(kept.get(12), 0x87);
    }

    #[test]
    fn a_file_that_is_damaged_or_the_wrong_size_is_refused_and_says_why() {
        let mut bytes = File::rekordbox(Kind::Mixer).bytes().to_vec();
        bytes[HEADER + 13] = 0x80;
        let damaged = File::parse(Kind::Mixer, bytes.clone()).unwrap_err().to_string();
        assert!(damaged.contains("checksum"), "{damaged}");
        bytes.pop();
        let short = File::parse(Kind::Mixer, bytes).unwrap_err().to_string();
        assert!(short.contains("159 bytes, where rekordbox writes 160"), "{short}");
    }

    #[test]
    fn a_file_on_the_drive_is_written_over_only_when_booth_put_it_there_or_is_told_to() {
        let mut settings = Settings::rekordbox();
        let ours = settings.file(Kind::Player).clone();
        let last = ours.digest();
        let sync = FIELDS.iter().find(|f| f.id == "sync").unwrap();
        settings.set(sync, 0x81);
        let changed = settings.file(Kind::Player);
        let theirs = {
            let mut file = File::rekordbox(Kind::Player);
            file.set(sync.at, 0x81);
            file.set(10, 0x80);
            file
        };

        assert_eq!(step(changed, None, None, false), Step::Write);
        assert_eq!(step(changed, Some(changed.bytes()), None, false), Step::Same);
        assert_eq!(step(changed, Some(ours.bytes()), Some(&last), false), Step::Replace);
        assert_eq!(step(changed, Some(ours.bytes()), None, false), Step::Keep, "rekordbox's own");
        assert_eq!(step(changed, Some(theirs.bytes()), Some(&last), false), Step::Keep);
        assert_eq!(step(changed, Some(theirs.bytes()), Some(&last), true), Step::Over);
        assert!(!Step::Keep.writes() && Step::Over.writes());
    }

    #[test]
    fn a_set_is_kept_in_a_collection_as_its_bytes_and_read_back_the_same() {
        let mut settings = Settings::rekordbox();
        settings.set(FIELDS.iter().find(|f| f.id == "language").unwrap(), 0x8b);
        let kept = serde_json::to_string(&settings).unwrap();
        assert!(kept.contains("\"MYSETTING.DAT\":\"60000000"), "{kept}");
        assert_eq!(serde_json::from_str::<Settings>(&kept).unwrap(), settings);
        let damaged = kept.replacen("60000000", "61000000", 1);
        assert!(serde_json::from_str::<Settings>(&damaged).is_err());
    }

    #[test]
    fn a_drives_settings_are_read_from_its_pioneer_folder_and_a_bad_one_is_named() {
        let scratch = std::env::temp_dir().join(format!("booth-settings-{}", std::process::id()));
        let pioneer = scratch.join("PIONEER");
        std::fs::create_dir_all(&pioneer).unwrap();
        let mut player = File::rekordbox(Kind::Player);
        player.set(36, 0x83);
        std::fs::write(pioneer.join("MYSETTING.DAT"), player.bytes()).unwrap();
        std::fs::write(pioneer.join("DJMMYSETTING.DAT"), b"not one").unwrap();

        let found = read(&scratch);
        assert_eq!(found.files, [player]);
        assert_eq!(found.refused.len(), 1);
        assert_eq!(found.refused[0].0, Kind::Mixer);
        let described = describe(&scratch).unwrap();
        assert!(described[0].1.contains(&"Tempo Range: Wide  (rekordbox's: ±10%)".to_string()));
        assert!(described[1].1[0].starts_with("not read: DJMMYSETTING.DAT is 7 bytes"));
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
