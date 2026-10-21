//! Deciding where the cue points go.
//!
//! A cue is worth setting where something changes and where the voice comes in.
//! The first of those falls out of the phrase analysis; the second needs a
//! measurement of its own, and gets an honest one: energy that is both centred
//! in the stereo image and in the range a voice occupies. That finds a sung
//! line entering over a backing. It will also fire on a centred lead synth,
//! which is the limit of what can be told without separating the stems — and
//! separating them is minutes a track, not milliseconds.
//!
//! Everything lands on a beat, and on a downbeat where there is one close by,
//! because a cue half a beat early is worse than no cue at all.

use crate::export::{Cue, Rgb};

use super::features::Features;
use super::structure::{Kind, Structure};

/// A player has eight hot cues, A through H.
const HOT_CUES: usize = 8;

/// What a button is for, in the order the eight are filled.
///
/// A fixed order rather than a ranking, because what a DJ remixing a record
/// live reaches for is a set: the drop, the hook, the break, the drop back in,
/// a way in, a way out, the build before the drop, and a second line the room
/// sings. Ranking the arrangement against the words produced sets that were
/// all of one or all of the other, and sharing the buttons half and half
/// between them fixed that by count and not by kind — a busy arrangement's
/// four were its four loudest sections, and the words' four the hook and three
/// verse lines. A role nothing on the record fills is skipped, not held open.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// The first drop: where the floor moves.
    FirstDrop,
    /// The line the room sings.
    Hook,
    /// The first breakdown: where a second record comes in.
    Break,
    /// The second drop: the record coming back after the break. Where the
    /// hook comes back on it, the drop is that button and reads "drop".
    DropBackIn,
    /// A way in: the intro, or the build before the first drop.
    MixIn,
    /// A way out: the outro.
    MixOut,
    /// A build before a drop, to ride into it live.
    PreDrop,
    /// Eight bars or more with nothing sung: room for another record's vocal.
    Instrumental,
    /// A second line the room sings.
    Refrain,
}

/// The roles, in the order they are filled. See [`Role`]. One more than there
/// are buttons: a record with every one of them to offer fills the eight
/// before the last is reached, and most records are missing several.
const ORDER: [Role; HOT_CUES + 1] = [
    Role::FirstDrop,
    Role::Hook,
    Role::Break,
    Role::DropBackIn,
    Role::MixIn,
    Role::MixOut,
    Role::PreDrop,
    Role::Instrumental,
    Role::Refrain,
];
/// How long the voice has to keep going before its arrival counts, in seconds.
/// Shorter than this is a vocal stab, not an entry.
const MIN_VOCAL_SECONDS: f64 = 1.0;
/// And how long since the last one, so a phrase with gaps in it does not
/// produce a cue per breath.
const VOCAL_SPACING_SECONDS: f64 = 8.0;
/// How far a cue will be moved to land on a downbeat rather than a plain beat.
const SNAP_BEATS: usize = 2;

/// What a cue was set for, which decides its colour and what it is called.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Section(Kind),
    /// Where a voice arrives, measured off the mix.
    Vocal,
    /// The first landing of the line the track repeats most, read out of the
    /// words. See [`crate::transcribe`].
    Hook,
    /// Where another line the track keeps coming back to first lands.
    Refrain,
    /// The hook coming back after a verse's worth of absence. See
    /// [`crate::transcribe::MomentKind::Comeback`].
    ///
    /// A marker, like every other return: a line gets one button, so that
    /// eight buttons cover eight things. Where it lands on a drop it folds into
    /// the drop, which is a button of its own, and that is the one way a line
    /// is under two buttons.
    Comeback,
    /// A line sung once and never again.
    ///
    /// Only ever fills a button nothing better wanted — see [`to_the_buttons`]
    /// — which on a track with no chorus is most of them.
    Line,
    /// The longest stretch of the record with nothing sung over it, at its
    /// bar line: eight bars or more of room to lay another record's vocal
    /// over, which is the one moment a remix reaches for that neither the
    /// arrangement nor the words name. See [`instrumental_stretch`].
    Instrumental,
}

impl Reason {
    /// What to call it, when nothing better is known. A hook knows better: it
    /// carries the line itself.
    pub fn label(self) -> &'static str {
        match self {
            Reason::Section(kind) => kind.label(),
            Reason::Vocal => "vocal",
            Reason::Hook => "hook",
            Reason::Refrain => "refrain",
            // The hook, back: what a DJ calls it is the same thing.
            Reason::Comeback => "hook",
            Reason::Line => "line",
            Reason::Instrumental => "instrumental",
        }
    }

    /// Colours a DJ can read at a glance in a dark booth: the drop is red, the
    /// breakdown is blue, the voice is purple and the hook is the brightest
    /// thing on the waveform.
    pub fn color(self) -> Rgb {
        let (r, g, b) = match self {
            Reason::Section(Kind::Intro) => (0x30, 0x5a, 0xff),
            Reason::Section(Kind::Up) => (0xe2, 0xa0, 0x3f),
            Reason::Section(Kind::Chorus) => (0xc0, 0x3a, 0x22),
            Reason::Section(Kind::Down) => (0x2f, 0x6f, 0xd0),
            Reason::Section(Kind::Outro) => (0x2f, 0x7d, 0x52),
            Reason::Vocal => (0x9a, 0x6b, 0xd4),
            Reason::Hook | Reason::Comeback => (0xe8, 0x3c, 0x9e),
            Reason::Refrain => (0xb4, 0x5c, 0xc8),
            Reason::Line => (0x74, 0x55, 0xa4),
            // Teal: nothing sung, so none of the purples, and not a section.
            Reason::Instrumental => (0x2a, 0x9d, 0x8f),
        };
        Rgb { r, g, b }
    }

    /// Which cues survive when there are more than eight candidates.
    ///
    /// The hook outranks everything, including the drop. A drop can be found by
    /// looking at the waveform — it is the loud part — and the line the crowd
    /// sings cannot be found by looking at anything.
    pub fn priority(self, first_of_its_kind: bool) -> u32 {
        match self {
            Reason::Hook => 110,
            Reason::Section(Kind::Chorus) => 100,
            Reason::Vocal if first_of_its_kind => 95,
            Reason::Section(Kind::Intro) => 90,
            Reason::Section(Kind::Down) => 70,
            Reason::Refrain => 65,
            Reason::Vocal => 60,
            Reason::Section(Kind::Outro) => 55,
            // Below every line the record comes back to, above any it does
            // not: the hook coming back is worth more than a verse line and
            // less than a second line the room sings.
            Reason::Comeback => 55,
            Reason::Section(Kind::Up) => 50,
            // Reached by its role or not at all, in practice: below a build,
            // because where the two coincide the build's name says more.
            Reason::Instrumental => 48,
            Reason::Line => 45,
        }
    }

    fn is_section(self) -> bool {
        matches!(self, Reason::Section(_))
    }
}

/// One moment that might be worth a cue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub time_ms: u32,
    pub reason: Reason,
    /// What to write on the cue, when there is something better to say than the
    /// reason's own name — the words of a hook, say.
    pub label: Option<String>,
    /// How many bars the section runs for, where this is one.
    ///
    /// On the marker rather than only in the window, because a player's own
    /// phrase strip cannot say it: the label there is chosen from ten fixed
    /// strings by the phrase kind and its flag bytes, and there is no text
    /// field in the format to put a number in. A memory cue's comment is text,
    /// and a player shows it.
    pub bars: Option<u16>,
    /// Whether this may only ever be a memory cue.
    ///
    /// A player has eight buttons and as many memory cues as a track needs, so
    /// a moment that is worth marking but not worth a button — a line coming
    /// round for the fourth time — says so here rather than being ranked
    /// against the drops and losing every time by a different margin.
    pub memory_only: bool,
}

impl Candidate {
    pub fn new(time_ms: u32, reason: Reason) -> Self {
        Self { time_ms, reason, label: None, bars: None, memory_only: false }
    }

    pub fn named(time_ms: u32, reason: Reason, label: &str) -> Self {
        let label = label.trim();
        Self {
            time_ms,
            reason,
            label: (!label.is_empty()).then(|| label.to_string()),
            bars: None,
            memory_only: false,
        }
    }

    /// The same moment, marked as one that never takes a button.
    pub fn memory_only(mut self) -> Self {
        self.memory_only = true;
        self
    }

    /// Whether this moment's words go onto whatever swallows it in [`fold`].
    ///
    /// A marker that could never have been a button has nothing to lend. The
    /// hook coming back is one: its words are already on the hook's own
    /// button, and a drop that lands where the hook returns should read
    /// "drop", which is what a DJ reaching past the words is reaching for.
    fn lends_words(&self) -> bool {
        !self.memory_only
    }

    fn comment(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.reason.label().to_string())
    }
}

/// The cue a moment in the words has earned.
pub fn from_moment(moment: &crate::transcribe::Moment) -> Candidate {
    use crate::transcribe::MomentKind;
    let reason = match moment.kind {
        MomentKind::VocalIn => Reason::Vocal,
        MomentKind::Hook | MomentKind::Return => Reason::Hook,
        MomentKind::Comeback => Reason::Comeback,
        MomentKind::Refrain => Reason::Refrain,
        MomentKind::Line => Reason::Line,
    };
    let candidate = Candidate::named(moment.time_ms, reason, &moment.text);
    match moment.kind {
        MomentKind::Return | MomentKind::Comeback => candidate.memory_only(),
        _ => candidate,
    }
}

/// How close two moments have to be before they are one moment.
///
/// Half a second. A cue that near another one is not a second place to drop
/// in — it is the same place, found twice by two different measurements.
const TOGETHER_MS: u32 = 500;

/// How near two buttons can be, in bars, and still both be worth one.
///
/// Two. Pressed, the earlier is at the later inside two bars, which is less
/// time than it takes to reach for a second button, and a set of eight with
/// two that close covers seven places. It is the earlier that keeps its
/// button, because it plays into the later and the later does not play into
/// it.
const APART_BARS: usize = 2;

/// Suggest cue points for a track.
///
/// Returns a memory cue at the first downbeat — which is where a player parks
/// when the track loads — followed by up to eight hot cues in time order.
pub fn suggest(
    features: &Features,
    beat_times: &[u32],
    structure: &Structure,
    bar_phase: usize,
) -> Vec<Cue> {
    if beat_times.is_empty() {
        return Vec::new();
    }

    let mut candidates = sections(structure, beat_times);
    for at in vocal_entries(features, beat_times, structure) {
        candidates.push(Candidate::new(at, Reason::Vocal));
    }

    // The memory cue is where the track begins as far as a player is
    // concerned, and a player counts in bars from it.
    let start = first_downbeat(beat_times, bar_phase);
    // The last beat the grid names, which is the last bar a player will let you
    // park on rather than wherever the file happens to stop.
    let end = beat_times.last().copied();
    assemble(start, end, beat_times, candidates)
}

/// A cue at the start of every section the phrase analysis found.
pub fn sections(structure: &Structure, beat_times: &[u32]) -> Vec<Candidate> {
    structure
        .sections
        .iter()
        .filter_map(|section| {
            beat_times.get(section.start_beat as usize - 1).map(|&at| Candidate {
                // Whole bars, rounded down: a boundary dragged into the middle
                // of a bar loses that bar rather than claiming it, which is
                // what the window's own strip counts and shows.
                bars: Some(section.end_beat.saturating_sub(section.start_beat) / 4)
                    .filter(|bars| *bars > 0),
                ..Candidate::new(at, Reason::Section(section.kind))
            })
        })
        .collect()
}

/// How many bars with nothing sung make an instrumental stretch worth a
/// button. Eight: a phrase, which is the least another record's vocal can be
/// laid over and sound meant.
pub const INSTRUMENTAL_BARS: usize = 8;

/// The longest stretch of the record with nothing sung over it, as a cue at
/// its bar line, where it is [`INSTRUMENTAL_BARS`] long or longer.
///
/// `grid` is every beat from the first downbeat and `sung` is where the stem
/// is singing, as `transcribe::align::sounds` measures it. Only the gaps
/// between sung stretches are looked at: before the first is the intro and
/// after the last the outro, which have cues of their own. The cue goes on the
/// first bar line after the singing stops — or on the first four-bar boundary
/// after it, where that still leaves the eight, because a vocal laid over from
/// a phrase boundary lands in phrase and one laid over from bar three does
/// not. `None` with no grid, no words read, or no gap long enough.
pub fn instrumental_stretch(grid: &[u32], sung: &[(u32, u32)]) -> Option<Candidate> {
    use super::place::BEATS_PER_BAR;
    let bar = BEATS_PER_BAR;
    // Whole bars from the bar line at beat `from` before the singing resumes.
    let bars_clear = |from: usize, until: u32| {
        (1..).take_while(|&n| grid.get(from + n * bar).is_some_and(|&end| end <= until)).count()
    };
    let mut best: Option<(usize, u32)> = None;
    for pair in sung.windows(2) {
        let (quiet_from, quiet_until) = (pair[0].1, pair[1].0);
        let first_beat = grid.partition_point(|at| *at < quiet_from);
        let bar_line = first_beat.div_ceil(bar) * bar;
        let phrase_line = bar_line.div_ceil(bar * 4) * bar * 4;
        let start = [phrase_line, bar_line].into_iter().find(|&from| {
            grid.get(from).is_some() && bars_clear(from, quiet_until) >= INSTRUMENTAL_BARS
        });
        let Some(from) = start else { continue };
        let bars = bars_clear(from, quiet_until);
        if best.is_none_or(|(longest, _)| bars > longest) {
            best = Some((bars, grid[from]));
        }
    }
    best.map(|(bars, at)| Candidate {
        bars: u16::try_from(bars).ok(),
        ..Candidate::new(at, Reason::Instrumental)
    })
}

/// Turn everything worth cueing into the cues a player will actually hold.
///
/// One memory cue at `start_ms`, then the best eight of `candidates` in time
/// order. Moments that land on top of each other are folded into one, and the
/// survivors are ranked by [`Reason::priority`] — because a player has eight
/// hot cues and a busy track has more than eight moments, and which eight it
/// keeps is the whole difference between a useful set of cues and a wall of
/// markers. No two of the eight are within [`APART_BARS`] of each other on
/// `grid`, the track's beats; with no grid, nothing is.
///
/// The hook is always one of them, and always A; the rest are lettered from B
/// in time order, or from A on a record with no hook.
pub fn assemble(
    start_ms: u32,
    end_ms: Option<u32>,
    grid: &[u32],
    candidates: Vec<Candidate>,
) -> Vec<Cue> {
    assemble_with_reasons(start_ms, end_ms, grid, candidates).0
}

/// [`assemble`], and what each button is for: one [`Reason`] per hot cue, in
/// the order the hot cues come in the list, which is time order.
///
/// For an account of the cues to say. A cue's comment cannot: a hook's
/// comment is its line and so is a refrain's, and a drop that a line lands on
/// carries the line.
pub fn assemble_with_reasons(
    start_ms: u32,
    end_ms: Option<u32>,
    grid: &[u32],
    candidates: Vec<Candidate>,
) -> (Vec<Cue>, Vec<Reason>) {
    use super::place::{beats_between, BEATS_PER_BAR};

    use crate::transcribe::inside;

    // The hook's line, to know its button by where a drop has taken it in and
    // carries its words, or where a longer line with the hook inside it has
    // taken the hook's button — see below.
    let hook_line = candidates
        .iter()
        .find(|candidate| candidate.reason == Reason::Hook && !candidate.memory_only)
        .and_then(|candidate| candidate.label.clone());
    let is_hook = |candidate: &Candidate| {
        candidate.reason == Reason::Hook
            || hook_line.as_deref().is_some_and(|hook| {
                candidate.label.as_deref().is_some_and(|said| said == hook || inside(hook, said))
            })
    };
    // A button the words put there, which says a line, as against a section
    // that a line was folded into.
    let says_a_line = |candidate: &Candidate| {
        !candidate.reason.is_section()
            && candidate.label.as_deref().is_some_and(|said| !said.trim().is_empty())
    };

    let mut kept = fold(rank(candidates));

    // Every moment worth cueing becomes a memory cue, whether or not it also
    // gets one of the eight buttons. A player holds as many of these as the
    // track needs, and what they cost is a line in the list rather than a hot
    // cue somebody wanted for something else.
    let mut cues = memory_cues(start_ms, end_ms, &kept);

    // A hot cue where the memory cue already is, is a button that does what
    // loading the track already did — and eight is not many. The name is worth
    // keeping: "intro" on the cue a player parks at says something, and it
    // costs no slot to say it there.
    //
    // This is nearly always the first section: an arrangement starts at the
    // top of the record, and so does the memory cue. It was quietly taking hot
    // cue A on every track, where it sat invisibly underneath the memory marker
    // and looked like a cue that had not been set.
    //
    // Taken out before the eight are chosen, not after, or the track would
    // come out with seven. Not the hook, on a record that opens on it: the
    // hook always has a button, and it is always A.
    if let Some(at) = kept.iter().position(|(candidate, _)| {
        candidate.time_ms.abs_diff(start_ms) < TOGETHER_MS && !is_hook(candidate)
    }) {
        kept.remove(at);
    }
    kept.retain(|(candidate, _)| !candidate.memory_only);

    // No two buttons within two bars: the later of the two is left a marker,
    // and its button goes to whatever is next in line — unless the later is
    // the hook, which always has a button, and then it is the earlier that
    // goes. Looked for among the eight rather than among every candidate,
    // because a line that was never going to be a button has no business
    // keeping anything off one; and the eight are chosen again each time one
    // is left out, because the button it frees is filled by the order, and
    // what fills it can be crowded in its turn.
    let mut barred = vec![false; kept.len()];
    let chosen = loop {
        let chosen = to_the_buttons(&kept, &barred);
        // A line inside another line on the eight: "praise you" and "I have to
        // praise you like I should". Pressed, the longer plays the shorter, so
        // the shorter's button goes to something else, as asked — the hook's
        // too, and then the longer line is the hook's button.
        let shorter = chosen.iter().copied().find(|&i| {
            says_a_line(&kept[i].0)
                && chosen.iter().any(|&j| {
                    says_a_line(&kept[j].0)
                        && inside(
                            kept[i].0.label.as_deref().unwrap_or_default(),
                            kept[j].0.label.as_deref().unwrap_or_default(),
                        )
                })
        });
        if let Some(i) = shorter {
            barred[i] = true;
            continue;
        }
        let crowded = chosen.windows(2).find(|pair| {
            beats_between(grid, kept[pair[0]].0.time_ms, kept[pair[1]].0.time_ms)
                .is_some_and(|beats| beats < (APART_BARS * BEATS_PER_BAR) as f32)
        });
        match crowded {
            Some(pair) if is_hook(&kept[pair[1]].0) => barred[pair[0]] = true,
            Some(pair) => barred[pair[1]] = true,
            None => break chosen,
        }
    };

    // The hook on A, on every record that has one: the same pad for the line
    // the room sings whichever record is on the deck, so it is never looked
    // for. The rest from B, in time order.
    let hook = chosen.iter().position(|&i| is_hook(&kept[i].0));
    let letter = |n: usize| match hook {
        Some(hook) if n == hook => 1,
        Some(hook) if n < hook => n + 2,
        _ => n + 1,
    };

    let mut reasons = Vec::new();
    for (n, candidate) in chosen.into_iter().map(|i| &kept[i].0).enumerate() {
        let color = candidate.reason.color();
        cues.push(
            Cue::hot(letter(n) as u8, candidate.time_ms)
                .with_comment(&candidate.comment())
                .with_color(color.r, color.g, color.b),
        );
        reasons.push(candidate.reason);
    }
    (cues, reasons)
}

/// Which of the candidates get the eight buttons, in time order.
///
/// Each [`Role`] in [`ORDER`] takes the moment that fills it, where the record
/// has one. Whatever buttons the roles leave go to the rest by worth — a third
/// drop before a second break — with the lines sung once last, spread over the
/// record rather than taken from its front: taken from the front, three
/// buttons went to the first twenty seconds of a verse while the rest of the
/// record had none, which is a map of a verse.
///
/// Nothing is held open. A track with nine sections and no words gets eight
/// sections; an instrumental gets its eight and nothing is left blank that
/// could have been filled. What *is* left blank is what nothing was found for,
/// which is the honest answer — an empty button says "nothing here" and a
/// made-up one says the wrong thing in a booth.
///
/// Takes the list in time order, as [`fold`] leaves it, so that the earliest
/// moment of a kind is the first match, and hands back the indices of the
/// eight, in the same order. A candidate `barred` is passed over.
fn to_the_buttons(kept: &[(Candidate, u32)], barred: &[bool]) -> Vec<usize> {
    let mut taken = barred.to_vec();
    let mut chosen: Vec<usize> = Vec::new();
    let earliest = |taken: &[bool], wanted: &dyn Fn(&Candidate) -> bool| {
        kept.iter().enumerate().position(|(i, (candidate, _))| !taken[i] && wanted(candidate))
    };
    let latest = |taken: &[bool], wanted: &dyn Fn(&Candidate) -> bool| {
        kept.iter().enumerate().rposition(|(i, (candidate, _))| !taken[i] && wanted(candidate))
    };
    let is = |reason: Reason| move |candidate: &Candidate| candidate.reason == reason;
    let first_drop = kept
        .iter()
        .find(|(candidate, _)| candidate.reason == Reason::Section(Kind::Chorus))
        .map(|(candidate, _)| candidate.time_ms);
    for role in ORDER {
        let pick = match role {
            Role::FirstDrop => earliest(&taken, &is(Reason::Section(Kind::Chorus))),
            Role::Hook => earliest(&taken, &is(Reason::Hook)),
            Role::Break => earliest(&taken, &is(Reason::Section(Kind::Down))),
            // The first drop is taken by now, so the earliest drop left is
            // the second. Not the hook coming back: that is the hook's line
            // again, and a line gets one button unless the other is a drop.
            Role::DropBackIn => earliest(&taken, &is(Reason::Section(Kind::Chorus))),
            Role::MixIn => earliest(&taken, &|candidate: &Candidate| {
                matches!(candidate.reason, Reason::Section(Kind::Intro | Kind::Up))
                    && first_drop.is_none_or(|drop| candidate.time_ms < drop)
            }),
            Role::MixOut => latest(&taken, &is(Reason::Section(Kind::Outro))),
            Role::PreDrop => earliest(&taken, &is(Reason::Section(Kind::Up))),
            Role::Instrumental => earliest(&taken, &is(Reason::Instrumental)),
            Role::Refrain => earliest(&taken, &is(Reason::Refrain)),
        };
        if let Some(i) = pick {
            taken[i] = true;
            chosen.push(i);
        }
        if chosen.len() == HOT_CUES {
            break;
        }
    }

    // Then whatever is left, best first and ties to the earlier moment, with
    // the lines sung once kept back: they are all worth the same, and which of
    // them get a button is a question of where in the record they are.
    let mut rest: Vec<usize> =
        (0..kept.len()).filter(|&i| !taken[i] && kept[i].0.reason != Reason::Line).collect();
    rest.sort_by(|&a, &b| {
        kept[b].1.cmp(&kept[a].1).then(kept[a].0.time_ms.cmp(&kept[b].0.time_ms))
    });
    for i in rest.into_iter().take(HOT_CUES.saturating_sub(chosen.len())) {
        taken[i] = true;
        chosen.push(i);
    }
    let room = HOT_CUES.saturating_sub(chosen.len());
    let lines: Vec<(u32, usize)> = (0..kept.len())
        .filter(|&i| !taken[i] && kept[i].0.reason == Reason::Line)
        .map(|i| (kept[i].0.time_ms, i))
        .collect();
    chosen.extend(crate::transcribe::spread_in_time(lines, room).into_iter().map(|(_, i)| i));

    // Indices in order are moments in order, `kept` being in time order.
    chosen.sort_unstable();
    chosen
}

/// What a section is called on a marker, in the words a DJ uses about a record.
fn section_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Intro => "Intro",
        Kind::Up => "Build",
        Kind::Down => "Break",
        Kind::Chorus => "Drop",
        Kind::Outro => "Outro",
    }
}

/// A named memory cue for every moment, in time order.
///
/// The names are what a DJ would write on the markers themselves: Start, then
/// each section numbered within its own kind — Build 1, Drop 1, Break 1, Drop 2
/// — and End on the last bar. Numbering within the kind rather than across all
/// of them is the whole point: six markers all called "drop" say nothing that
/// looking at the waveform does not.
///
/// A sung line gets a verse number and its words, every time it lands: "V1 Get
/// Down" wherever that line comes round. The number says which line of the
/// record this is without having to read it, and the words say which line it
/// is at a glance — a CDJ-3000X shows the whole comment, so there is no reason
/// to make somebody remember what V1 was.
///
/// `kept` is the folded, time-ordered list [`assemble`] works from.
fn memory_cues(start_ms: u32, end_ms: Option<u32>, kept: &[(Candidate, u32)]) -> Vec<Cue> {
    let mut out = vec![Cue::memory(start_ms).with_comment("Start")];
    let mut sections: Vec<(&'static str, usize)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut plain_vocals = 0usize;

    for (candidate, _) in kept {
        // The section that sits on the start is what Start is: naming it twice
        // would put two markers on the same bar, which is one to step over
        // every time the track is loaded. It is still counted, so a second
        // intro reads "Intro 2" rather than starting again at one.
        let at_start = candidate.time_ms.abs_diff(start_ms) < TOGETHER_MS;
        let label = match candidate.reason {
            Reason::Section(kind) => {
                let word = section_word(kind);
                let count = match sections.iter_mut().find(|(seen, _)| *seen == word) {
                    Some((_, count)) => {
                        *count += 1;
                        *count
                    }
                    None => {
                        sections.push((word, 1));
                        1
                    }
                };
                // "Drop 1 · 40 bars". The ordinal says which drop this is
                // and the bars say what kind of drop it is — a 40-bar one is
                // the record's centre and a 16-bar one is a passing lift, and
                // on a deck you are reading the marker rather than counting
                // bars off the waveform.
                match candidate.bars {
                    Some(bars) => format!("{word} {count} · {bars} bars"),
                    None => format!("{word} {count}"),
                }
            }
            // One a record, and what it is worth saying is how long it is.
            Reason::Instrumental => match candidate.bars {
                Some(bars) => format!("Instrumental \u{b7} {bars} bars"),
                None => "Instrumental".to_string(),
            },
            _ => match candidate.label.as_deref() {
                Some(text) => {
                    // Numbered by the line it is, not by how many have been
                    // seen: a line that comes round four times is V1 all four
                    // times, and the one after it is V2 rather than V5.
                    let at = match lines
                        .iter()
                        .position(|seen| crate::transcribe::same_line(seen, text))
                    {
                        Some(at) => at,
                        None => {
                            lines.push(text.to_string());
                            lines.len() - 1
                        }
                    };
                    // The words every time, not only the first. A player with
                    // room for them is a player that should show them, and
                    // "V1" on its own asks somebody in a booth to remember
                    // what V1 was.
                    format!("V{} {}", at + 1, lines[at])
                }
                // A voice the mix found and the words did not. Numbered on its
                // own, because calling it V1 would claim it is a line that has
                // been read when nothing has read it.
                None => {
                    plain_vocals += 1;
                    format!("Vocal {plain_vocals}")
                }
            },
        };
        if !at_start {
            out.push(Cue::memory(candidate.time_ms).with_comment(&label));
        }
    }

    // The last bar, so that running out of record is a marker rather than a
    // surprise. Only when it is clear of everything else: a marker half a
    // second after the outro cue is two markers on one moment.
    if let Some(end) = end_ms {
        let clear = out.iter().all(|cue| cue.time_ms + TOGETHER_MS <= end);
        if clear {
            out.push(Cue::memory(end).with_comment("End"));
        }
    }
    out
}

/// Put the candidates in time order and price each one.
///
/// The first vocal is worth more than the ones after it, which is why this is a
/// pass over the whole list rather than a map: what a moment is worth depends
/// on what came before it.
fn rank(mut candidates: Vec<Candidate>) -> Vec<(Candidate, u32)> {
    candidates.sort_by_key(|candidate| candidate.time_ms);
    let mut seen_vocal = false;
    candidates
        .into_iter()
        .map(|candidate| {
            let first = candidate.reason == Reason::Vocal && !seen_vocal;
            if candidate.reason == Reason::Vocal {
                seen_vocal = true;
            }
            let priority = candidate.reason.priority(first);
            (candidate, priority)
        })
        .collect()
}

/// Fold moments that land together into one.
///
/// A section start wins any such argument, whatever it is worth: when the voice
/// arrives exactly where the drop does, that is the drop, and a second marker
/// half a second away from the first is a marker nobody can use. But the loser's
/// words are kept — a drop that is also where the hook lands should say so,
/// because "drop" is something a DJ can see on the waveform and the line the
/// crowd sings is not.
///
/// Takes the list already in time order, as [`rank`] leaves it.
fn fold(ranked: Vec<(Candidate, u32)>) -> Vec<(Candidate, u32)> {
    let mut kept: Vec<(Candidate, u32)> = Vec::with_capacity(ranked.len());
    for (candidate, priority) in ranked {
        let Some((last, last_priority)) = kept.last_mut() else {
            kept.push((candidate, priority));
            continue;
        };
        if last.time_ms.abs_diff(candidate.time_ms) >= TOGETHER_MS {
            kept.push((candidate, priority));
            continue;
        }
        // A moment that may only ever be a memory cue never takes the place of
        // one that could have been a button, whatever it is worth: the line
        // coming round for the fourth time is worth a marker and is not worth
        // the drop's slot.
        let beaten = match (last.memory_only, candidate.memory_only) {
            (true, false) => true,
            (false, true) => false,
            _ => match (last.reason.is_section(), candidate.reason.is_section()) {
                (true, false) => false,
                (false, true) => true,
                _ => priority > *last_priority,
            },
        };
        // Nor does it lend its words to whatever swallowed it, and neither
        // does the hook coming back — see `Candidate::lends_words`. A drop
        // that happens to land on a hook coming round should still read
        // "drop", because that is what a DJ is reaching for when they are
        // not reaching for the words.
        let lend = match beaten {
            true => last.lends_words() && !candidate.memory_only,
            false => candidate.lends_words() && !last.memory_only,
        };
        match beaten {
            true => {
                let words = last.label.take().filter(|_| lend);
                *last = candidate;
                *last_priority = priority;
                last.label = last.label.take().or(words);
            }
            false => {
                let words = candidate.label.filter(|_| lend);
                last.label = last.label.take().or(words);
            }
        }
    }
    kept
}

/// Where a player should park when the track loads: the first bar line.
///
/// A beat tracker starts wherever it first found a beat, which is as likely to
/// be the second or fourth beat of a bar as the first. Parking there puts the
/// track's own start a part-bar out of phase with everything that counts in
/// fours — the bar numbers in the transport, a four-bar loop taken on the
/// first press, the grid a second deck is being matched against — and the
/// error is inaudible to look at and obvious to mix with.
///
/// So the cue skips forward to the first beat the grid calls a bar line:
/// nothing, or one, two or three beats. Forward rather than back, because the
/// beats before it are the part bar at the head of the track and there is no
/// bar there to park at the start of.
fn first_downbeat(beat_times: &[u32], bar_phase: usize) -> u32 {
    // The first beat the grid calls a bar line, which is this many beats in.
    // Never more than three: the fourth beat of any bar is followed by one.
    beat_times.get(bar_phase % 4).copied().unwrap_or(beat_times[0])
}

/// Where a voice arrives, in milliseconds, snapped to the grid.
fn vocal_entries(features: &Features, beat_times: &[u32], structure: &Structure) -> Vec<u32> {
    let curve = smoothed_voice(features);
    if curve.is_empty() {
        return Vec::new();
    }

    let mut sorted = curve.clone();
    sorted.sort_by(f32::total_cmp);
    let quiet = sorted[sorted.len() / 5];
    let loud = sorted[sorted.len() * 9 / 10];
    // Nothing stands out from the backing, so nothing is singing over it.
    if loud - quiet < 0.02 {
        return Vec::new();
    }
    let threshold = quiet + (loud - quiet) * 0.5;

    let sustain = (MIN_VOCAL_SECONDS * features.frame_rate) as usize;
    let mut entries = Vec::new();
    let mut last_entry = f64::NEG_INFINITY;
    let mut frame = 1;
    while frame + sustain < curve.len() {
        let rising = curve[frame] >= threshold && curve[frame - 1] < threshold;
        if rising && curve[frame..frame + sustain].iter().all(|&v| v >= threshold * 0.7) {
            let seconds = features.seconds_at(frame);
            if seconds - last_entry >= VOCAL_SPACING_SECONDS {
                entries.push(snap(seconds, beat_times, structure));
                last_entry = seconds;
            }
            frame += sustain;
            continue;
        }
        frame += 1;
    }
    entries
}

/// The voice measurement, smoothed over half a second so that one syllable does
/// not read as an entry and one gap does not read as a departure.
fn smoothed_voice(features: &Features) -> Vec<f32> {
    let window = ((features.frame_rate / 2.0) as usize).max(1);
    if features.voice.len() < window * 2 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(features.voice.len());
    let mut sum = 0.0f64;
    for (i, &value) in features.voice.iter().enumerate() {
        sum += value as f64;
        if i >= window {
            sum -= features.voice[i - window] as f64;
        }
        out.push((sum / window.min(i + 1) as f64) as f32);
    }
    out
}

/// Move a moment onto the grid: the nearest downbeat if one is within a beat or
/// two, otherwise the nearest beat.
fn snap(seconds: f64, beat_times: &[u32], structure: &Structure) -> u32 {
    let target = (seconds * 1000.0).round() as u32;
    let nearest = beat_times
        .iter()
        .enumerate()
        .min_by_key(|(_, &at)| at.abs_diff(target))
        .map(|(i, _)| i)
        .unwrap_or(0);

    // Bar lines are wherever the phrase analysis says a section starts, four
    // beats apart from there.
    let bar_phase =
        structure.sections.first().map(|s| (s.start_beat as usize - 1) % 4).unwrap_or(0);
    let from = nearest.saturating_sub(SNAP_BEATS);
    let to = (nearest + SNAP_BEATS + 1).min(beat_times.len());
    let downbeat = (from..to).find(|i| i % 4 == bar_phase);
    beat_times[downbeat.unwrap_or(nearest)]
}

#[cfg(test)]
mod tests {
    use super::super::features;
    use super::super::structure::Section;
    use super::*;
    use crate::audio::Audio;
    use crate::export::CueKind;

    const RATE: u32 = 44_100;
    const BEAT_MS: u32 = 500;

    fn beats(count: usize) -> Vec<u32> {
        (0..count).map(|i| i as u32 * BEAT_MS).collect()
    }

    fn line(start_ms: u32, text: &str) -> crate::transcribe::Line {
        crate::transcribe::Line { start_ms, end_ms: start_ms + 2_000, text: text.to_string() }
    }

    /// Lines that share no words worth counting, so that a fixture of one-off
    /// lines stays one-off: `Transcript::refrains` groups lines by the words
    /// they share, and "line number 1" is the same line as "line number 2".
    const UNALIKE: [&str; 11] = [
        "copper kettles whistle at dawn",
        "the ferry leaves without us",
        "paper lanterns over the harbour",
        "nobody counted the stairs",
        "velvet curtains and a broken clock",
        "rain on the tin roof again",
        "somebody left the radio playing",
        "a map with no roads on it",
        "the last tram rattles home",
        "glass marbles rolling downhill",
        "cold coffee in a paper cup",
    ];

    fn section(start_bar: usize, end_bar: usize, kind: Kind, intensity: f32) -> Section {
        Section {
            start_beat: (start_bar * 4) as u16 + 1,
            end_beat: (end_bar * 4) as u16 + 1,
            kind,
            intensity,
        }
    }

    /// A track with a backing throughout and a centred "voice" over part of it.
    fn with_voice(total_secs: f32, voice_from: f32, voice_to: f32) -> Audio {
        let frames = (RATE as f32 * total_secs) as usize;
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        for i in 0..frames {
            let t = i as f32 / RATE as f32;
            let two_pi = 2.0 * std::f32::consts::PI;
            // A wide backing: the same note, out of phase, so it sits nowhere
            // near the middle.
            let backing = 0.3 * (two_pi * 300.0 * t).sin();
            left[i] = backing;
            right[i] = -backing;
            // Plus a kick down the middle, which is centred but too low to be
            // mistaken for a voice.
            let kick = 0.4 * (two_pi * 55.0 * t).sin();
            left[i] += kick;
            right[i] += kick;
            if t >= voice_from && t < voice_to {
                // A centred, wobbling tone in the vocal range.
                let vibrato = (two_pi * 5.0 * t).sin() * 20.0;
                let voice = 0.35 * (two_pi * (900.0 + vibrato) * t).sin();
                left[i] += voice;
                right[i] += voice;
            }
        }
        Audio::new(RATE, vec![left, right]).unwrap()
    }

    #[test]
    fn every_section_start_gets_a_cue() {
        let structure = Structure {
            sections: vec![
                section(0, 8, Kind::Intro, -20.0),
                section(8, 16, Kind::Up, -14.0),
                section(16, 24, Kind::Chorus, -8.0),
            ],
        };
        let audio = with_voice(4.0, 10.0, 10.0);
        let cues = suggest(&features::extract(&audio), &beats(128), &structure, 0);

        // The first section is the memory cue rather than a hot one: the
        // player already parks at the top of the record, so a hot cue there is
        // a button that does what loading the track did. It keeps the name.
        assert!(!cues[0].is_hot());
        assert_eq!(cues[0].time_ms, 0);
        assert_eq!(cues[0].comment.as_deref(), Some("Start"));

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 2);
        assert_eq!(hot[0].time_ms, 8 * 4 * BEAT_MS);
        assert_eq!(hot[1].time_ms, 16 * 4 * BEAT_MS);
    }

    #[test]
    fn cues_are_named_and_coloured_by_what_they_are() {
        let structure = Structure {
            sections: vec![
                section(0, 8, Kind::Intro, -20.0),
                section(8, 16, Kind::Down, -18.0),
                section(16, 24, Kind::Chorus, -8.0),
            ],
        };
        let cues =
            suggest(&features::extract(&with_voice(4.0, 10.0, 10.0)), &beats(128), &structure, 0);
        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();

        // The intro is on the memory cue, where the player parks anyway.
        assert_eq!(cues[0].comment.as_deref(), Some("Start"));
        assert_eq!(hot[0].comment.as_deref(), Some("break"));
        assert_eq!(hot[1].comment.as_deref(), Some("drop"));
        // The drop is red and the breakdown is blue, and they are not the same.
        assert_ne!(hot[0].color, hot[1].color);
        assert_eq!(hot[1].color, Some(Rgb { r: 0xc0, g: 0x3a, b: 0x22 }));
    }

    #[test]
    fn the_first_cue_a_player_parks_on_is_a_memory_cue() {
        let structure = Structure { sections: vec![section(0, 8, Kind::Intro, -20.0)] };
        let cues =
            suggest(&features::extract(&with_voice(4.0, 10.0, 10.0)), &beats(64), &structure, 0);
        assert!(!cues[0].is_hot());
        assert_eq!(cues[0].time_ms, 0);
        assert_eq!(cues[0].kind, CueKind::Point);
    }

    #[test]
    fn a_voice_coming_in_gets_its_own_cue() {
        // Twenty seconds of backing, with a voice from ten seconds in.
        let audio = with_voice(20.0, 10.0, 20.0);
        let f = features::extract(&audio);
        let structure = Structure { sections: vec![section(0, 10, Kind::Intro, -20.0)] };
        let cues = suggest(&f, &beats(80), &structure, 0);

        let vocal = cues.iter().find(|c| c.comment.as_deref() == Some("vocal"));
        let vocal = vocal.unwrap_or_else(|| panic!("no vocal cue in {cues:?}"));
        let seconds = vocal.time_ms as f64 / 1000.0;
        assert!((seconds - 10.0).abs() < 1.5, "the voice cue landed at {seconds:.2}s");
    }

    #[test]
    fn an_instrumental_gets_no_vocal_cue() {
        let audio = with_voice(20.0, 100.0, 100.0);
        let f = features::extract(&audio);
        let structure = Structure { sections: vec![section(0, 10, Kind::Intro, -20.0)] };
        let cues = suggest(&f, &beats(80), &structure, 0);
        assert!(cues.iter().all(|c| c.comment.as_deref() != Some("vocal")), "{cues:?}");
    }

    #[test]
    fn a_vocal_cue_lands_on_a_beat() {
        let audio = with_voice(20.0, 10.3, 20.0);
        let f = features::extract(&audio);
        let structure = Structure { sections: vec![section(0, 10, Kind::Intro, -20.0)] };
        let cues = suggest(&f, &beats(80), &structure, 0);
        for cue in &cues {
            assert_eq!(cue.time_ms % BEAT_MS, 0, "a cue at {} ms is off the grid", cue.time_ms);
        }
    }

    #[test]
    fn a_track_with_more_than_eight_moments_keeps_the_useful_ones() {
        let kinds = [
            Kind::Intro,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Up,
            Kind::Chorus,
            Kind::Outro,
        ];
        let sections = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| section(i * 8, (i + 1) * 8, kind, -10.0))
            .collect();
        let structure = Structure { sections };
        let cues =
            suggest(&features::extract(&with_voice(4.0, 10.0, 10.0)), &beats(400), &structure, 0);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 8, "a player has eight hot cues");
        // All three drops survived; the builds are what got dropped.
        assert_eq!(hot.iter().filter(|c| c.comment.as_deref() == Some("drop")).count(), 3);
        assert!(hot.iter().filter(|c| c.comment.as_deref() == Some("build")).count() < 3);
        // And they are lettered in time order.
        for pair in hot.windows(2) {
            assert!(pair[0].hot_cue < pair[1].hot_cue);
            assert!(pair[0].time_ms <= pair[1].time_ms);
        }
    }

    /// The memory cues a set came out with, in the order they were written.
    fn marks(cues: &[Cue]) -> Vec<String> {
        cues.iter()
            .filter(|cue| !cue.is_hot())
            .map(|cue| cue.comment.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn the_track_start_lands_on_a_bar_line_rather_than_wherever_tracking_began() {
        // A beat tracker starts at the first beat it heard, which is as likely
        // to be the second or fourth of a bar as the first. Parking the start
        // cue there puts the track a part-bar out of phase with everything
        // that counts in fours — the bar numbers in the transport, a four-bar
        // loop taken on the first press, a second deck being matched to it.
        let times = beats(64);
        let features = features::extract(&with_voice(4.0, 10.0, 10.0));

        // Half-second beats, so skipping one beat is 500 ms.
        for (phase, expected) in [(0usize, 0u32), (1, 500), (2, 1_000), (3, 1_500)] {
            let cues = suggest(&features, &times, &Structure::default(), phase);
            let start = cues.iter().find(|cue| !cue.is_hot()).expect("no memory cue at all");
            assert_eq!(
                start.time_ms, expected,
                "a grid whose bar line is {phase} beats in parked at {}",
                start.time_ms
            );
        }
    }

    #[test]
    fn the_start_never_skips_a_whole_bar_looking_for_one() {
        // Three beats is the most it can ever be: the fourth beat of a bar is
        // followed by the first of the next.
        let times = beats(64);
        let features = features::extract(&with_voice(4.0, 10.0, 10.0));
        let cues = suggest(&features, &times, &Structure::default(), 4);
        let start = cues.iter().find(|cue| !cue.is_hot()).unwrap();
        assert_eq!(start.time_ms, 0, "a phase of four is a phase of none");
    }

    #[test]
    fn a_section_marker_says_how_long_the_section_runs() {
        // What a player's own phrase strip cannot say. The label there comes
        // from ten fixed strings chosen by the phrase kind and its flag bytes,
        // and the format has no text field to put a number in — so a deck
        // draws "CHORUS 2" for a forty-bar drop and a sixteen-bar one alike.
        // A memory cue's comment is text, and a player shows it.
        let structure = Structure {
            sections: vec![
                section(0, 16, Kind::Intro, -20.0),
                section(16, 56, Kind::Chorus, -6.0),
                section(56, 72, Kind::Chorus, -8.0),
            ],
        };
        let marked = marks(&assemble(0, Some(200_000), &[], sections(&structure, &beats(400))));

        assert!(marked.contains(&"Drop 1 · 40 bars".to_string()), "{marked:?}");
        assert!(marked.contains(&"Drop 2 · 16 bars".to_string()), "{marked:?}");
    }

    #[test]
    fn a_marker_with_no_length_behind_it_is_left_as_it_was() {
        // A moment that is not a section — a hook, a vocal coming in — has no
        // length to report, and inventing one would be worse than the number
        // being absent.
        let candidates = vec![Candidate::new(30_000, Reason::Section(Kind::Chorus))];
        assert_eq!(marks(&assemble(0, Some(200_000), &[], candidates)), ["Start", "Drop 1", "End"]);
    }

    #[test]
    fn sections_are_numbered_within_their_own_kind() {
        // Six markers all reading "drop" say nothing the waveform does not.
        // Numbered within the kind, the list is the arrangement.
        let kinds = [Kind::Intro, Kind::Up, Kind::Chorus, Kind::Down, Kind::Up, Kind::Chorus];
        let candidates = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 30_000, Reason::Section(kind)))
            .collect();

        assert_eq!(
            marks(&assemble(0, Some(200_000), &[], candidates)),
            ["Start", "Build 1", "Drop 1", "Break 1", "Build 2", "Drop 2", "End"]
        );
    }

    #[test]
    fn the_first_section_is_start_and_is_still_counted() {
        // Start is the intro, so a second intro has to read "Intro 2" — one
        // that started again at one would say there were two first sections.
        let candidates = vec![
            Candidate::new(0, Reason::Section(Kind::Intro)),
            Candidate::new(60_000, Reason::Section(Kind::Intro)),
        ];
        assert_eq!(marks(&assemble(0, None, &[], candidates)), ["Start", "Intro 2"]);
    }

    #[test]
    fn a_sung_line_is_named_wherever_it_lands_and_keeps_its_number() {
        // What a player shows while a track is loaded is the comment on the
        // marker, and a CDJ-3000X shows the whole of it. So every landing says
        // which line it is and what the line was, and the number stays with
        // the line rather than counting the landings.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "get down"),
                line(60_000, "everybody in the room"),
                line(90_000, "get down"),
                line(120_000, "everybody in the room"),
                line(150_000, "get down"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let marks = marks(&assemble(0, None, &[], candidates));
        assert_eq!(marks[0], "Start");
        assert_eq!(
            marks.iter().filter(|mark| *mark == "V1 get down").count(),
            3,
            "every landing of the line should say which line it is: {marks:?}"
        );
        assert!(
            !marks.iter().any(|mark| *mark == "V1"),
            "a number with no words asks somebody in a booth to remember: {marks:?}"
        );
        assert!(
            marks.iter().any(|mark| mark.starts_with("V2 ")),
            "the other line is another verse: {marks:?}"
        );
    }

    #[test]
    fn a_verse_marker_carries_the_whole_line() {
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "everybody in the room put your hands up"),
                line(90_000, "everybody in the room put your hands up"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let marks = marks(&assemble(0, None, &[], candidates));
        assert!(
            marks.iter().all(|mark| !mark.starts_with('V')
                || mark == "V1 everybody in the room put your hands up"),
            "{marks:?}"
        );
    }

    #[test]
    fn a_line_the_recogniser_wrote_two_ways_is_labelled_one_way() {
        // Close enough to be the same line is close enough to carry the same
        // marker: two spellings of one hook read as two hooks in a booth.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "hold me closer now"),
                line(90_000, "hold me closer, now"),
                line(150_000, "hold me closer now"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let marks = marks(&assemble(0, None, &[], candidates));
        let verses: Vec<&String> = marks.iter().filter(|mark| mark.starts_with('V')).collect();
        assert!(verses.len() >= 2, "{marks:?}");
        assert!(
            verses.iter().all(|mark| *mark == verses[0]),
            "one line should read one way: {verses:?}"
        );
    }

    #[test]
    fn the_end_marker_is_only_written_where_nothing_else_is() {
        // A marker half a second after the outro cue is two markers on one
        // moment, which is one to step over every time.
        let candidates = vec![Candidate::new(200_000, Reason::Section(Kind::Outro))];
        assert_eq!(marks(&assemble(0, Some(200_100), &[], candidates)), ["Start", "Outro 1"]);
    }

    #[test]
    fn the_hook_is_cued_with_the_words_that_make_it_one() {
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "hold me closer now"),
                line(90_000, "hold me closer now"),
                line(150_000, "hold me closer now"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(0, None, &[], candidates);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        let hook = hot.iter().find(|c| c.time_ms == 30_000).expect("no cue on the hook");
        assert_eq!(hook.comment.as_deref(), Some("hold me closer now"));
        // Once, not once per airing, and not again where it comes back from
        // its longest absence: eight cues spent more than once on the same
        // words is eight cues covering fewer than eight things.
        let with_the_words: Vec<u32> = hot
            .iter()
            .filter(|c| c.comment.as_deref() == hook.comment.as_deref())
            .map(|c| c.time_ms)
            .collect();
        assert_eq!(with_the_words, vec![30_000], "{hot:?}");
    }

    #[test]
    fn the_slots_the_words_give_back_go_to_the_arrangement() {
        // The point of the whole change. A track whose hook comes round five
        // times, with an arrangement to match: before, the words took six of
        // the eight and the breaks and builds were pushed out.
        let words = crate::transcribe::Transcript {
            lines: (0..5).map(|i| line(30_000 + i * 60_000, "hold me closer now")).collect(),
            ..Default::default()
        };
        let kinds = [
            Kind::Intro,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Chorus,
            Kind::Down,
            Kind::Outro,
        ];
        let mut candidates: Vec<Candidate> = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 45_000, Reason::Section(kind)))
            .collect();
        candidates.extend(words.moments().iter().map(from_moment));

        let hot: Vec<Cue> =
            assemble(0, None, &[], candidates).into_iter().filter(Cue::is_hot).collect();
        let named = |what: &str| hot.iter().filter(|c| c.comment.as_deref() == Some(what)).count();

        assert_eq!(named("hold me closer now"), 1, "the hook took more than one slot: {hot:?}");
        // And the arrangement got the rest, saying what it is: a drop that
        // happened to land on a hook return still reads "drop", because that
        // is what a DJ is reaching for when they are not reaching for the
        // words.
        assert_eq!(named("drop"), 2, "{hot:?}");
        assert_eq!(named("break"), 2, "{hot:?}");
    }

    #[test]
    fn the_hook_survives_a_track_with_more_moments_than_a_player_has_cues() {
        // Ten sections, all of them loud, plus a hook halfway through. Eight
        // slots; the hook takes one of them whatever else is going on.
        let mut candidates: Vec<Candidate> =
            (0..10).map(|i| Candidate::new(i * 20_000, Reason::Section(Kind::Chorus))).collect();
        candidates.push(Candidate::named(95_000, Reason::Hook, "hold me closer now"));

        let cues = assemble(0, None, &[], candidates);
        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 8);
        assert!(
            hot.iter().any(|c| c.comment.as_deref() == Some("hold me closer now")),
            "the hook was dropped for a drop: {hot:?}"
        );
    }

    #[test]
    fn a_drop_that_is_also_the_hook_is_one_cue_that_says_the_words() {
        let candidates = vec![
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::named(60_200, Reason::Hook, "hold me closer now"),
        ];
        let cues = assemble(0, None, &[], candidates);

        let hot: Vec<&Cue> = cues.iter().filter(|c| c.is_hot()).collect();
        assert_eq!(hot.len(), 1, "two markers half a second apart: {hot:?}");
        // The section keeps the moment — it is on the grid — and the words
        // come with it.
        assert_eq!(hot[0].time_ms, 60_000);
        assert_eq!(hot[0].comment.as_deref(), Some("hold me closer now"));
        assert_eq!(hot[0].color, Some(Reason::Section(Kind::Chorus).color()));
    }

    /// Six lines with nothing in common, so the grouping keeps them apart and
    /// the track genuinely repeats nothing. Lines that differ by one word do
    /// not do: the whole point of the grouping is that those are one line.
    const VERSE: [&str; 6] = [
        "walking through the city at night",
        "nobody told me it would end",
        "a coat on the back of a chair",
        "every window on the eighteenth floor",
        "she said wait for the rain",
        "counting the stops to the river",
    ];

    /// The hot cues a set came out with, in time order.
    fn buttons(cues: &[Cue]) -> Vec<String> {
        cues.iter()
            .filter(|cue| cue.is_hot())
            .map(|cue| cue.comment.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_track_where_nothing_repeats_still_comes_back_with_lyric_cues() {
        // The fault: a rap, a live take, a record with one verse and no
        // chorus has no hook and no refrains, so the words came back with one
        // marker for the voice arriving and nothing else. Its lines are what
        // it has.
        let words = crate::transcribe::Transcript {
            lines: VERSE
                .iter()
                .enumerate()
                .map(|(i, text)| line(20_000 + i as u32 * 40_000, text))
                .collect(),
            ..Default::default()
        };
        let mut candidates = vec![
            Candidate::new(0, Reason::Section(Kind::Intro)),
            Candidate::new(100_000, Reason::Section(Kind::Chorus)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));

        let hot = buttons(&assemble(0, None, &[], candidates));
        let sung = hot.iter().filter(|name| VERSE.contains(&name.as_str())).count();
        assert!(sung >= 4, "the spare buttons should have gone to the words: {hot:?}");
        assert!(hot.len() <= HOT_CUES, "{hot:?}");
    }

    /// A record with nine things worth a button, the hook a bar after the
    /// first drop.
    fn the_hook_a_bar_after_the_drop() -> Vec<Candidate> {
        let words = crate::transcribe::Transcript {
            lines: vec![line(62_000, "hold me closer now"), line(140_000, "hold me closer now")],
            ..Default::default()
        };
        let mut candidates = vec![
            Candidate::new(0, Reason::Section(Kind::Intro)),
            Candidate::new(50_000, Reason::Section(Kind::Up)),
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::new(100_000, Reason::Section(Kind::Down)),
            Candidate::new(150_000, Reason::Section(Kind::Up)),
            Candidate::new(160_000, Reason::Section(Kind::Chorus)),
            Candidate::new(200_000, Reason::Section(Kind::Down)),
            Candidate::new(220_000, Reason::Section(Kind::Chorus)),
            Candidate::new(240_000, Reason::Section(Kind::Outro)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));
        candidates
    }

    #[test]
    fn of_two_buttons_closer_than_two_bars_the_first_keeps_its_and_the_second_goes_elsewhere() {
        // A break a bar after the first drop: pressed, the drop is at the
        // break in a bar, so a button on the break as well is a second button
        // for one place. The break stays a marker, the first break's button
        // goes to the next one, and the second break, which was left out,
        // gets the button the break gave up.
        let candidates = vec![
            Candidate::new(50_000, Reason::Section(Kind::Up)),
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::new(62_000, Reason::Section(Kind::Down)),
            Candidate::new(100_000, Reason::Section(Kind::Down)),
            Candidate::new(150_000, Reason::Section(Kind::Up)),
            Candidate::new(160_000, Reason::Section(Kind::Chorus)),
            Candidate::new(200_000, Reason::Section(Kind::Down)),
            Candidate::new(220_000, Reason::Section(Kind::Chorus)),
            Candidate::new(240_000, Reason::Section(Kind::Outro)),
        ];
        let cues = assemble(0, None, &beats(600), candidates);
        let hot: Vec<u32> = cues.iter().filter(|cue| cue.is_hot()).map(|cue| cue.time_ms).collect();
        assert_eq!(
            hot,
            vec![50_000, 60_000, 100_000, 150_000, 160_000, 200_000, 220_000, 240_000],
            "{cues:?}"
        );
        assert!(cues.iter().any(|cue| !cue.is_hot() && cue.time_ms == 62_000), "{cues:?}");
    }

    #[test]
    fn the_hook_keeps_its_button_against_one_a_bar_before_it_and_is_always_a() {
        // The one exception to the earlier keeping its button: the hook always
        // has one. The drop a bar before it is left a marker, the first drop's
        // button goes to the next drop, and the hook is A with the rest from B
        // in time order.
        let cues = assemble(0, None, &beats(600), the_hook_a_bar_after_the_drop());
        let hot: Vec<(u8, u32)> =
            cues.iter().filter(|cue| cue.is_hot()).map(|cue| (cue.hot_cue, cue.time_ms)).collect();
        assert_eq!(
            hot,
            vec![
                (2, 50_000),
                (1, 62_000),
                (3, 100_000),
                (4, 150_000),
                (5, 160_000),
                (6, 200_000),
                (7, 220_000),
                (8, 240_000),
            ],
            "{cues:?}"
        );
        assert!(cues.iter().any(|cue| !cue.is_hot() && cue.time_ms == 60_000), "{cues:?}");
    }

    #[test]
    fn of_two_lines_one_inside_the_other_only_the_longer_has_a_button() {
        // Praise You: the hook is "praise you", sung most, and a second line
        // has it inside. Pressed, the longer plays the shorter, so it keeps the
        // button, takes the hook's place on A, and the hook is markers only.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(30_000, "praise you"),
                line(40_000, "I have to praise you like I should"),
                line(60_000, "praise you"),
                line(90_000, "praise you"),
                line(100_000, "I have to praise you like I should"),
                line(120_000, "praise you"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![
            Candidate::new(10_000, Reason::Section(Kind::Chorus)),
            Candidate::new(70_000, Reason::Section(Kind::Down)),
            Candidate::new(150_000, Reason::Section(Kind::Chorus)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));

        let cues = assemble(0, None, &[], candidates);
        let hot: Vec<(u8, u32, String)> = cues
            .iter()
            .filter(|cue| cue.is_hot())
            .map(|cue| (cue.hot_cue, cue.time_ms, cue.comment.clone().unwrap_or_default()))
            .collect();
        assert!(!hot.iter().any(|(_, _, said)| said == "praise you"), "{hot:?}");
        assert!(hot.contains(&(1, 40_000, "I have to praise you like I should".into())), "{hot:?}");
        assert!(
            marks(&cues).iter().any(|mark| mark.ends_with(" praise you")),
            "the hook is still marked where it is sung: {:?}",
            marks(&cues)
        );
    }

    #[test]
    fn a_drop_that_carries_a_line_keeps_its_button_whatever_lines_it_is_inside() {
        // The rule is about two buttons for the same words. A drop is a button
        // for the drop, whatever line landed on it.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(10_000, "praise you"),
                line(40_000, "I have to praise you like I should"),
                line(100_000, "I have to praise you like I should"),
            ],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(10_000, Reason::Section(Kind::Chorus))];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(0, None, &[], candidates);
        assert!(cues.iter().any(|cue| cue.is_hot() && cue.time_ms == 10_000), "{cues:?}");
    }

    #[test]
    fn without_a_grid_no_two_buttons_are_too_close() {
        // Bars are the grid's to say. With none, the drop keeps its button a
        // second before the hook, as it did before the rule.
        let cues = assemble(0, None, &[], the_hook_a_bar_after_the_drop());
        assert!(cues.iter().any(|cue| cue.is_hot() && cue.time_ms == 60_000), "{cues:?}");
        assert!(cues.iter().any(|cue| cue.is_hot() && cue.time_ms == 62_000), "{cues:?}");
    }

    #[test]
    fn a_record_that_opens_on_its_hook_still_has_the_hook_on_a() {
        // A hot cue where the memory cue already is does what loading the track
        // does, and is left off for that — but not the hook's: the hook always
        // has a button, and A is where it is.
        let words = crate::transcribe::Transcript {
            lines: vec![line(0, "hold me closer now"), line(90_000, "hold me closer now")],
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(30_000, Reason::Section(Kind::Chorus))];
        candidates.extend(words.moments().iter().map(from_moment));
        let hot: Vec<(u8, u32)> = assemble(0, None, &beats(600), candidates)
            .iter()
            .filter(|cue| cue.is_hot())
            .map(|cue| (cue.hot_cue, cue.time_ms))
            .collect();
        assert_eq!(hot, vec![(1, 0), (2, 30_000)]);
    }

    #[test]
    fn the_first_of_two_keeps_its_button_even_where_the_second_was_chosen_before_it() {
        // The drop is the first button filled and the build a bar before it
        // the fifth, but the build is first in the record: pressed, it plays
        // into the drop, and the drop does not play into it.
        let candidates = vec![
            Candidate::new(58_000, Reason::Section(Kind::Up)),
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::new(100_000, Reason::Section(Kind::Down)),
            Candidate::new(160_000, Reason::Section(Kind::Chorus)),
        ];
        let cues = assemble(0, None, &beats(600), candidates);
        let hot: Vec<u32> = cues.iter().filter(|cue| cue.is_hot()).map(|cue| cue.time_ms).collect();
        assert_eq!(hot, vec![58_000, 100_000, 160_000], "{cues:?}");
        assert!(cues.iter().any(|cue| !cue.is_hot() && cue.time_ms == 60_000), "{cues:?}");
    }

    #[test]
    fn two_buttons_two_bars_apart_are_both_kept() {
        let candidates = vec![
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::new(64_000, Reason::Section(Kind::Down)),
        ];
        let hot: Vec<u32> = assemble(0, None, &beats(600), candidates)
            .iter()
            .filter(|cue| cue.is_hot())
            .map(|cue| cue.time_ms)
            .collect();
        assert_eq!(hot, vec![60_000, 64_000]);
    }

    #[test]
    fn the_eight_are_filled_in_the_order_a_live_remix_reaches_for_them() {
        // Everything on offer: six sections past the start, a hook that comes
        // back after a long break, a second, shorter line — between lines sung
        // as often, the longer is the hook — and a verse of one-off lines. The
        // eight are the build in, the drop, the hook, the refrain, the break,
        // the second build, the second drop and the outro — and not the hook
        // again where it comes back, nor any verse line.
        let mut lines = vec![
            line(64_000, "hold me closer now"),
            line(70_000, "take me home"),
            line(184_000, "hold me closer now"),
            line(190_000, "take me home"),
        ];
        for (i, text) in VERSE.iter().enumerate() {
            lines.push(line(10_000 + i as u32 * 7_000, text));
        }
        let words = crate::transcribe::Transcript { lines, ..Default::default() };
        let sections = [
            (0, Kind::Intro),
            (30_000, Kind::Up),
            (60_000, Kind::Chorus),
            (120_000, Kind::Down),
            (150_000, Kind::Up),
            (180_000, Kind::Chorus),
            (240_000, Kind::Outro),
        ];
        let mut candidates: Vec<Candidate> =
            sections.iter().map(|&(at, kind)| Candidate::new(at, Reason::Section(kind))).collect();
        candidates.extend(words.moments().iter().map(from_moment));

        let hot: Vec<(u32, String)> = assemble(0, None, &[], candidates)
            .into_iter()
            .filter(Cue::is_hot)
            .map(|cue| (cue.time_ms, cue.comment.unwrap_or_default()))
            .collect();
        assert_eq!(
            hot.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            vec![30_000, 60_000, 64_000, 70_000, 120_000, 150_000, 180_000, 240_000],
            "{hot:?}"
        );
    }

    #[test]
    fn an_instrumental_stretch_takes_its_button_before_a_second_line() {
        // The same record with eight bars of nothing sung at 200 s: the
        // stretch is the eighth role and the refrain the ninth, so the
        // refrain is what gives way.
        let mut lines = vec![
            line(64_000, "hold me closer now"),
            line(70_000, "take me home"),
            line(184_000, "hold me closer now"),
            line(190_000, "take me home"),
        ];
        for (i, text) in VERSE.iter().enumerate() {
            lines.push(line(10_000 + i as u32 * 7_000, text));
        }
        let words = crate::transcribe::Transcript { lines, ..Default::default() };
        let sections = [
            (0, Kind::Intro),
            (30_000, Kind::Up),
            (60_000, Kind::Chorus),
            (120_000, Kind::Down),
            (150_000, Kind::Up),
            (180_000, Kind::Chorus),
            (240_000, Kind::Outro),
        ];
        let mut candidates: Vec<Candidate> =
            sections.iter().map(|&(at, kind)| Candidate::new(at, Reason::Section(kind))).collect();
        candidates.extend(words.moments().iter().map(from_moment));
        candidates
            .push(Candidate { bars: Some(16), ..Candidate::new(200_000, Reason::Instrumental) });

        let cues = assemble(0, None, &[], candidates);
        let hot: Vec<u32> = cues.iter().filter(|cue| cue.is_hot()).map(|cue| cue.time_ms).collect();
        assert_eq!(hot, vec![30_000, 60_000, 64_000, 120_000, 150_000, 180_000, 200_000, 240_000]);
        assert!(
            marks(&cues).iter().any(|mark| mark == "Instrumental \u{b7} 16 bars"),
            "{:?}",
            marks(&cues)
        );
    }

    #[test]
    fn the_longest_stretch_with_nothing_sung_is_cued_at_its_phrase_line() {
        // 120 BPM: a bar is two seconds. Singing until 36.5 s and again from
        // 80 s leaves 43.5 s, twenty-one bars from the bar line at 38 s; the
        // four-bar boundary at 40 s still leaves twenty, so the cue goes
        // there. A gap of six bars elsewhere is not a stretch, and the intro
        // and the outro are not looked at however long they are.
        let grid: Vec<u32> = (0..400).map(|i| i * 500).collect();
        let sung = [(20_000, 22_000), (34_500, 36_500), (80_000, 82_000), (94_000, 96_000)];
        let stretch = instrumental_stretch(&grid, &sung).expect("a stretch");
        assert_eq!(
            (stretch.time_ms, stretch.bars, stretch.reason),
            (40_000, Some(20), Reason::Instrumental)
        );

        // Where taking the phrase line would leave too few bars, the bar
        // line itself: singing until 36.5 s and again from 55 s is eight whole bars
        // from 38 s and only seven from 40 s.
        let short =
            instrumental_stretch(&grid, &[(34_500, 36_500), (55_000, 57_000)]).expect("a stretch");
        assert_eq!((short.time_ms, short.bars), (38_000, Some(8)));

        assert!(instrumental_stretch(&grid, &[(34_500, 36_500), (50_000, 52_000)]).is_none());
        assert!(
            instrumental_stretch(&grid, &[(100_000, 102_000)]).is_none(),
            "the intro is not a stretch"
        );
        assert!(instrumental_stretch(&[], &sung).is_none());
    }

    #[test]
    fn a_busy_arrangement_cannot_push_the_words_out() {
        // Nine drops, and a record with a hook that comes back and a refrain:
        // the words' two are in the eight whatever the arrangement offers,
        // because the set is incomplete without them. Sharing the buttons
        // half and half used to see to that by count; the order does it by
        // kind, and gives the arrangement the other six. The hook coming back
        // is a marker: a line gets one button.
        let words = crate::transcribe::Transcript {
            lines: vec![
                line(31_000, "hold me closer now"),
                line(37_000, "take me home"),
                line(151_000, "hold me closer now"),
                line(157_000, "take me home"),
            ],
            ..Default::default()
        };
        let mut candidates: Vec<Candidate> = (0..9)
            .map(|i| Candidate::new(i * 20_000 + 10_000, Reason::Section(Kind::Chorus)))
            .collect();
        candidates.extend(words.moments().iter().map(from_moment));

        let hot: Vec<(u32, String)> = assemble(0, None, &[], candidates)
            .into_iter()
            .filter(Cue::is_hot)
            .map(|cue| (cue.time_ms, cue.comment.unwrap_or_default()))
            .collect();
        assert_eq!(hot.len(), HOT_CUES, "{hot:?}");
        for (at, what) in [(31_000, "the hook"), (37_000, "the refrain")] {
            assert!(hot.iter().any(|(when, _)| *when == at), "{what} has no button: {hot:?}");
        }
        assert!(!hot.iter().any(|(when, _)| *when == 151_000), "the hook twice: {hot:?}");
        assert_eq!(hot.iter().filter(|(_, name)| name == "drop").count(), 6, "{hot:?}");
    }

    #[test]
    fn each_button_comes_back_with_what_it_is_for_in_time_order_with_the_hook_on_a() {
        // The second drop reads "drop" and so does the first, and the hook's
        // button reads its line: the comments alone cannot say which button
        // is the hook and which a drop the hook happens to land on.
        let words = crate::transcribe::Transcript {
            lines: vec![line(31_000, "hold me closer now"), line(150_200, "hold me closer now")],
            ..Default::default()
        };
        let mut candidates = vec![
            Candidate::new(10_000, Reason::Section(Kind::Chorus)),
            Candidate::new(90_000, Reason::Section(Kind::Down)),
            Candidate::new(150_000, Reason::Section(Kind::Chorus)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));

        let (cues, reasons) = assemble_with_reasons(0, None, &[], candidates);
        let hot: Vec<(u8, u32)> =
            cues.iter().filter(|cue| cue.is_hot()).map(|cue| (cue.hot_cue, cue.time_ms)).collect();
        assert_eq!(hot.len(), reasons.len(), "{hot:?} {reasons:?}");
        let said: Vec<(u8, u32, Reason)> =
            hot.iter().zip(&reasons).map(|(&(letter, at), &why)| (letter, at, why)).collect();
        assert_eq!(
            said,
            vec![
                (2, 10_000, Reason::Section(Kind::Chorus)),
                (1, 31_000, Reason::Hook),
                (3, 90_000, Reason::Section(Kind::Down)),
                (4, 150_000, Reason::Section(Kind::Chorus)),
            ]
        );
    }

    #[test]
    fn the_hook_coming_back_on_a_drop_is_under_that_drops_button() {
        // The one way a line is under two buttons: where the hook comes back on
        // a drop, the drop is a button of its own, and reads "drop".
        let words = crate::transcribe::Transcript {
            lines: vec![line(31_000, "hold me closer now"), line(150_200, "hold me closer now")],
            ..Default::default()
        };
        let mut candidates = vec![
            Candidate::new(10_000, Reason::Section(Kind::Chorus)),
            Candidate::new(90_000, Reason::Section(Kind::Down)),
            Candidate::new(150_000, Reason::Section(Kind::Chorus)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));

        let hot: Vec<(u32, String)> = assemble(0, None, &[], candidates)
            .into_iter()
            .filter(Cue::is_hot)
            .map(|cue| (cue.time_ms, cue.comment.unwrap_or_default()))
            .collect();
        assert!(hot.contains(&(31_000, "hold me closer now".into())), "{hot:?}");
        assert!(hot.contains(&(150_000, "drop".into())), "{hot:?}");
    }

    #[test]
    fn an_instrumental_gets_every_button_for_its_arrangement() {
        // Nothing is held open. There are no words to fill the roles that
        // want them, so the arrangement has every button.
        let kinds = [
            Kind::Intro,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Up,
            Kind::Chorus,
            Kind::Down,
            Kind::Chorus,
            Kind::Outro,
        ];
        let candidates: Vec<Candidate> = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| Candidate::new(i as u32 * 30_000 + 1_000, Reason::Section(kind)))
            .collect();

        let hot = buttons(&assemble(0, None, &[], candidates));
        assert_eq!(hot.len(), HOT_CUES, "{hot:?}");
    }

    #[test]
    fn a_track_with_little_to_cue_leaves_the_rest_of_the_buttons_empty() {
        // Rather than filling them with something made up. An empty button
        // says "nothing here"; a made-up one says the wrong thing in a booth.
        let candidates = vec![
            Candidate::new(30_000, Reason::Section(Kind::Chorus)),
            Candidate::new(90_000, Reason::Section(Kind::Down)),
        ];
        let hot = buttons(&assemble(0, None, &[], candidates));
        assert_eq!(hot.len(), 2, "{hot:?}");
    }

    #[test]
    fn a_line_only_fills_a_button_nothing_better_wanted() {
        // A hook and a refrain take their buttons, and a line sung once is a
        // marker whatever else happens — and a button too where one is
        // spare, as here, which costs the marker nothing.
        let mut lines: Vec<crate::transcribe::Line> =
            (0..3).map(|i| line(20_000 + i * 90_000, "hold me closer now")).collect();
        lines.extend((0..3).map(|i| line(50_000 + i * 90_000, "and the night comes down")));
        lines.push(line(300_000, "something said once and never again"));
        let words = crate::transcribe::Transcript { lines, ..Default::default() };

        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));

        let hot = buttons(&assemble(0, None, &[], candidates));
        assert!(hot.iter().any(|name| name == "hold me closer now"), "{hot:?}");
        assert!(hot.iter().any(|name| name == "and the night comes down"), "{hot:?}");
        // And it is still a marker, which costs nothing.
        let marks = marks(&assemble(0, None, &[], {
            let mut again = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
            again.extend(words.moments().iter().map(from_moment));
            again
        }));
        assert!(marks.iter().any(|mark| mark.contains("said once")), "{marks:?}");
    }

    #[test]
    fn a_record_shaped_like_the_report_cues_its_hook_once_and_not_its_first_verse() {
        // The report: a verse of one-off lines in the first forty seconds, a
        // chorus from forty-five, a break, the chorus back at a hundred and
        // four, an outro. The arrangement found only an intro, the break and
        // the outro. What came back was three buttons on the first twenty
        // seconds of the verse. The hook is cued once; where it comes back
        // after the break is a marker, there being no drop there for it to be.
        let verse: Vec<u32> = vec![
            1_740, 5_520, 9_280, 12_590, 14_430, 19_220, 24_700, 27_070, 28_130, 33_610, 39_770,
        ];
        let mut lines: Vec<crate::transcribe::Line> =
            verse.iter().zip(UNALIKE).map(|(&at, text)| line(at, text)).collect();
        lines.push(line(41_510, "we go round again, we go round again"));
        for at in [45_790, 52_770, 60_090, 67_370, 104_040, 111_580, 118_630, 126_150] {
            lines.push(line(at, "oh we are moving"));
        }
        for at in [49_140, 56_460, 63_730, 108_000, 115_260, 122_310, 129_620] {
            lines.push(line(at, "take me higher now"));
        }
        let words = crate::transcribe::Transcript { lines, ..Default::default() };

        let mut candidates = vec![
            Candidate::new(40, Reason::Section(Kind::Intro)),
            Candidate::new(76_760, Reason::Section(Kind::Down)),
            Candidate::new(109_640, Reason::Section(Kind::Outro)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(40, Some(134_000), &[], candidates);
        let hot: Vec<(u32, String)> = cues
            .iter()
            .filter(|cue| cue.is_hot())
            .map(|cue| (cue.time_ms, cue.comment.clone().unwrap_or_default()))
            .collect();

        let hook: Vec<u32> =
            hot.iter().filter(|(_, name)| name == "oh we are moving").map(|(at, _)| *at).collect();
        assert_eq!(hook, vec![45_790], "the hook once: {hot:?}");
        assert!(
            marks(&cues).iter().filter(|mark| mark.ends_with("oh we are moving")).count() >= 8,
            "every airing is still a marker: {:?}",
            marks(&cues)
        );
        assert!(hot.iter().any(|(at, _)| *at == 76_760), "the break: {hot:?}");
        let early_lines = hot.iter().filter(|(at, _)| *at < 20_000 && verse.contains(at)).count();
        assert!(early_lines <= 1, "the first twenty seconds took {early_lines} buttons: {hot:?}");
        assert_eq!(hot.len(), HOT_CUES, "{hot:?}");
    }

    #[test]
    fn spare_buttons_go_to_lines_spread_over_the_record_not_the_earliest_ones() {
        // Eight one-off lines offered, four slots for them: the four are
        // spread over the record rather than the first four by the clock.
        let words = crate::transcribe::Transcript {
            lines: UNALIKE
                .iter()
                .take(8)
                .enumerate()
                .map(|(i, text)| line(10_000 + i as u32 * 5_000, text))
                .collect(),
            ..Default::default()
        };
        let mut candidates = vec![Candidate::new(0, Reason::Section(Kind::Intro))];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(0, None, &[], candidates);
        let times: Vec<u32> =
            cues.iter().filter(|cue| cue.is_hot()).map(|cue| cue.time_ms).collect();
        // Every one-off is a candidate and there is room for all of them, so
        // this case takes them all; the spread shows when there is not.
        assert_eq!(times.len(), 8, "{times:?}");

        let mut candidates = vec![
            Candidate::new(0, Reason::Section(Kind::Intro)),
            Candidate::new(60_000, Reason::Section(Kind::Chorus)),
            Candidate::new(90_000, Reason::Section(Kind::Down)),
            Candidate::new(120_000, Reason::Section(Kind::Chorus)),
            Candidate::new(150_000, Reason::Section(Kind::Outro)),
        ];
        candidates.extend(words.moments().iter().map(from_moment));
        let cues = assemble(0, None, &[], candidates);
        let lines: Vec<u32> = cues
            .iter()
            .filter(|cue| cue.is_hot())
            .filter(|cue| cue.comment.as_deref().is_some_and(|c| UNALIKE.contains(&c)))
            .map(|cue| cue.time_ms)
            .collect();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_ne!(lines, vec![10_000, 15_000, 20_000, 25_000], "the first four by the clock");
        assert!(lines.iter().any(|&at| at >= 40_000), "the back of the verse too: {lines:?}");
    }

    #[test]
    fn a_hook_is_worth_more_than_a_build() {
        assert!(Reason::Hook.priority(false) > Reason::Section(Kind::Chorus).priority(false));
        assert!(Reason::Refrain.priority(false) > Reason::Section(Kind::Up).priority(false));
        assert!(Reason::Hook.color() != Reason::Section(Kind::Chorus).color());
        // The hook coming back: worth more than a line sung once, less than a
        // second line the room sings, and the same colour as the hook.
        assert!(Reason::Comeback.priority(false) > Reason::Line.priority(false));
        assert!(Reason::Comeback.priority(false) < Reason::Refrain.priority(false));
        assert_eq!(Reason::Comeback.color(), Reason::Hook.color());
    }

    #[test]
    fn a_track_with_no_beats_gets_no_cues() {
        let cues =
            suggest(&features::extract(&with_voice(4.0, 1.0, 3.0)), &[], &Structure::default(), 0);
        assert!(cues.is_empty());
    }
}
