//! Filling in what a file does not say about itself.
//!
//! A fingerprint identifies the *audio*, which is a different and much better
//! question than what the file name says. But it is not certain, and the two
//! can disagree — so the interesting part of this module is not the lookup, it
//! is [`decide`]: when to write a match in without asking, when to ask, and
//! when to leave a track alone.
//!
//! The rule it encodes: never overwrite something a person put there, and never
//! silently take a guess over a name that came out of the file's own tags.

use crate::library::Track;

/// One possible identification, as AcoustID and MusicBrainz report it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    /// Confidence, 0 to 1.
    pub score: f64,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub year: Option<u32>,
    pub recording_mbid: String,
    pub acoustid: String,
}

impl Match {
    pub fn describe(&self) -> String {
        let mut text = format!("{} — {}", self.artist, self.title);
        if !self.album.is_empty() {
            text.push_str(&format!(" ({})", self.album));
        }
        text
    }

    /// Whether it says anything worth having.
    pub fn is_useful(&self) -> bool {
        !self.artist.trim().is_empty() && !self.title.trim().is_empty()
    }
}

/// What to do with a match.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Write it in.
    Apply,
    /// Put it to the user: either the score is short of the bar, or it
    /// disagrees with something already there.
    Ask,
    /// Not worth showing anyone.
    Reject,
}

/// The score below which a match is not even worth asking about.
///
/// AcoustID returns long tails of near-zero matches for anything with a common
/// intro; a list of those to answer is worse than no list.
pub const FLOOR: f64 = 0.5;

/// Whether a match should be applied, asked about, or dropped.
///
/// Three things decide it: how confident the match is, whether the track
/// already has names, and — when it does — where those names came from. A name
/// out of the file's own tags is somebody's answer already, so a fingerprint
/// that disagrees with it is a question rather than a correction. A name off
/// the file name is a guess, so a confident fingerprint beats it.
pub fn decide(track: &Track, candidate: &Match, threshold: f64) -> Decision {
    if !candidate.is_useful() || candidate.score < FLOOR {
        return Decision::Reject;
    }

    // Nothing to disagree with: the track has no names of its own.
    if !has_names(track) {
        return match candidate.score >= threshold {
            true => Decision::Apply,
            false => Decision::Ask,
        };
    }

    // It agrees with what is already there, so there is nothing to do and
    // nothing to ask. Filling in an album or a year around an agreed name is
    // still worth doing.
    if agrees(track, candidate) {
        return match candidate.score >= threshold && adds_something(track, candidate) {
            true => Decision::Apply,
            false => Decision::Reject,
        };
    }

    // It disagrees. Whose answer is being contradicted decides whether that is
    // a correction or a question.
    match track.from_tags {
        true => Decision::Ask,
        false => match candidate.score >= threshold {
            true => Decision::Apply,
            false => Decision::Ask,
        },
    }
}

/// Whether the track already claims an artist and a title.
fn has_names(track: &Track) -> bool {
    !track.artist.trim().is_empty() && !track.title.trim().is_empty()
}

/// Whether the match says the same thing the track already does.
///
/// Loosely: case, spacing and punctuation differ constantly between a tag and a
/// database, and treating "Roll With the Punches" and "Roll With The Punches"
/// as a conflict would put half a library in front of the user for nothing.
fn agrees(track: &Track, candidate: &Match) -> bool {
    simplify(&track.artist) == simplify(&candidate.artist)
        && simplify(&track.title) == simplify(&candidate.title)
}

/// Whether applying it would fill in anything the track does not have.
fn adds_something(track: &Track, candidate: &Match) -> bool {
    (track.album.trim().is_empty() && !candidate.album.trim().is_empty())
        || (track.year.is_none() && candidate.year.is_some())
}

/// Lower case, no punctuation, single spaces.
fn simplify(text: &str) -> String {
    let mut out = String::new();
    let mut spaced = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            spaced = false;
        } else if !spaced {
            out.push(' ');
            spaced = true;
        }
    }
    out.trim_end().to_string()
}

/// A track and the match to put to the user.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub id: u32,
    /// What the track says now.
    pub current: String,
    /// Where that came from, in words.
    pub source: &'static str,
    pub candidate: Match,
    /// What the file's own path says, when that is worth having and disagrees
    /// with the fingerprint. Two answers that both look right is exactly the
    /// case a person should settle.
    pub from_path: Option<crate::guess::Guess>,
}

/// Which answer to a question was taken.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Leave the track as it is.
    Mine,
    /// Take what the fingerprint said.
    Fingerprint,
    /// Take what the path said.
    Path,
}

/// Whether a match and a path disagree about what a track is.
///
/// Compared the way [`agrees`] compares, and on the artist and title only: a
/// path never carries a release, so a difference in album is not a
/// disagreement about the recording.
pub fn conflicts(candidate: &Match, guess: &crate::guess::Guess) -> bool {
    if !guess.is_strong() || !candidate.is_useful() {
        return false;
    }
    simplify(&candidate.artist) != simplify(&guess.artist)
        || simplify(&candidate.title) != simplify(&guess.title)
}

/// What to do with a path's guess when no fingerprint came back.
///
/// A fingerprint that found nothing is not evidence of anything — the recording
/// may simply not be in the database, which is the normal state of affairs for
/// white labels, promos, edits and most of what a DJ owns. The path is then the
/// only evidence there is, and a strongly structured one is better than
/// "unknown artist".
///
/// It still never overwrites a name out of the file's own tags: somebody put
/// that there.
pub fn decide_from_path(track: &Track, guess: &crate::guess::Guess) -> Decision {
    if !guess.is_strong() {
        return Decision::Reject;
    }
    if track.from_tags {
        return Decision::Reject;
    }
    match has_names(track)
        && simplify(&track.artist) == simplify(&guess.artist)
        && simplify(&track.title) == simplify(&guess.title)
    {
        // Already says what the path says.
        true => Decision::Reject,
        false => Decision::Apply,
    }
}

/// How a track's current names should be described when asking about them.
pub fn source_of(track: &Track) -> &'static str {
    match (has_names(track), track.from_tags) {
        (false, _) => "nothing",
        (true, true) => "the file's tags",
        (true, false) => "the file name",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str, from_tags: bool) -> Track {
        let mut track = Track::placeholder(1);
        track.artist = artist.into();
        track.title = title.into();
        track.from_tags = from_tags;
        track
    }

    fn candidate(score: f64, artist: &str, title: &str) -> Match {
        Match {
            score,
            artist: artist.into(),
            title: title.into(),
            album: "Livity Sound".into(),
            year: Some(2019),
            ..Match::default()
        }
    }

    #[test]
    fn a_track_with_no_names_takes_a_confident_match() {
        let track = track("", "", false);
        let found = candidate(0.95, "Peverelist", "Roll With The Punches");
        assert_eq!(decide(&track, &found, 0.9), Decision::Apply);
    }

    #[test]
    fn a_match_below_the_threshold_is_a_question_rather_than_an_answer() {
        let track = track("", "", false);
        let found = candidate(0.7, "Peverelist", "Roll With The Punches");
        assert_eq!(decide(&track, &found, 0.9), Decision::Ask);
        // And the same match is applied once the bar is lowered to meet it.
        assert_eq!(decide(&track, &found, 0.6), Decision::Apply);
    }

    #[test]
    fn a_barely_scoring_match_is_not_worth_anyone_looking_at() {
        let track = track("", "", false);
        let found = candidate(0.2, "Someone", "Something");
        assert_eq!(decide(&track, &found, 0.9), Decision::Reject);
        assert_eq!(decide(&track, &found, 0.1), Decision::Reject, "the floor is not the threshold");
    }

    #[test]
    fn a_match_that_names_nothing_is_rejected() {
        let track = track("", "", false);
        let mut found = candidate(0.99, "", "Something");
        assert_eq!(decide(&track, &found, 0.5), Decision::Reject);
        found.artist = "Someone".into();
        found.title = "  ".into();
        assert_eq!(decide(&track, &found, 0.5), Decision::Reject);
    }

    #[test]
    fn a_confident_match_beats_a_name_taken_off_the_file_name() {
        // The file name is a guess; a fingerprint identifies the audio.
        let track = track("01 Track", "01 Track", false);
        let found = candidate(0.95, "Peverelist", "Roll With The Punches");
        assert_eq!(decide(&track, &found, 0.9), Decision::Apply);
    }

    #[test]
    fn a_name_out_of_the_files_own_tags_is_never_overwritten_without_asking() {
        // Somebody already answered this question — possibly the user. However
        // confident the fingerprint is, replacing it silently is not ours to do.
        let track = track("Peverelist", "Roll With The Punches", true);
        let found = candidate(0.99, "Pev", "Roll With The Punches (Original Mix)");
        assert_eq!(decide(&track, &found, 0.9), Decision::Ask);
        assert_eq!(decide(&track, &found, 0.0), Decision::Ask, "no threshold makes it automatic");
    }

    #[test]
    fn a_match_that_agrees_is_not_a_question() {
        let mut track = track("Peverelist", "Roll With The Punches", true);
        track.album = "Livity Sound".into();
        track.year = Some(2019);
        let found = candidate(0.99, "Peverelist", "Roll With The Punches");
        assert_eq!(decide(&track, &found, 0.9), Decision::Reject, "nothing left to add");
    }

    #[test]
    fn an_agreeing_match_still_fills_in_what_is_missing() {
        // The names match, but the album and year are blank — worth taking
        // without troubling anyone.
        let track = track("Peverelist", "Roll With The Punches", true);
        let found = candidate(0.99, "Peverelist", "Roll With The Punches");
        assert_eq!(decide(&track, &found, 0.9), Decision::Apply);
    }

    #[test]
    fn punctuation_and_case_are_not_a_disagreement() {
        let track = track("Peverelist", "Roll With the Punches", true);
        let found = candidate(0.99, "peverelist", "Roll With The Punches!");
        // Same record, spelled differently. Putting this in front of a user for
        // every second track would make the whole feature not worth having.
        assert_ne!(decide(&track, &found, 0.9), Decision::Ask);
    }

    #[test]
    fn simplifying_collapses_the_differences_that_do_not_matter() {
        assert_eq!(simplify("Roll With The Punches!"), "roll with the punches");
        assert_eq!(simplify("  A/B — C  "), "a b c");
        assert_eq!(simplify("Sébastien"), "sébastien");
        assert_eq!(simplify(""), "");
    }

    #[test]
    fn where_a_name_came_from_is_said_plainly() {
        assert_eq!(source_of(&track("", "", false)), "nothing");
        assert_eq!(source_of(&track("A", "B", false)), "the file name");
        assert_eq!(source_of(&track("A", "B", true)), "the file's tags");
    }

    fn from_path(artist: &str, title: &str) -> crate::guess::Guess {
        crate::guess::Guess {
            artist: artist.into(),
            title: title.into(),
            ..crate::guess::Guess::default()
        }
    }

    #[test]
    fn a_path_that_says_the_same_thing_as_the_fingerprint_is_not_a_conflict() {
        let found = candidate(0.9, "Peverelist", "Roll With The Punches");
        assert!(!conflicts(&found, &from_path("peverelist", "roll with the punches!")));
        assert!(!conflicts(&found, &from_path("Peverelist", "Roll With The Punches")));
    }

    #[test]
    fn a_path_that_names_another_record_is_a_conflict() {
        let found = candidate(0.99, "Peverelist", "Roll With The Punches");
        assert!(conflicts(&found, &from_path("Batu", "Marius")));
        // Even at a score that would otherwise be applied without asking: a
        // fingerprint is about the audio and a path is about what somebody
        // filed it as, and both being confident is the case worth a person.
        assert!(conflicts(&found, &from_path("Peverelist", "Sun Dance")));
    }

    #[test]
    fn a_path_with_no_artist_in_it_is_not_evidence_of_anything() {
        let found = candidate(0.99, "Peverelist", "Roll With The Punches");
        assert!(!conflicts(&found, &from_path("", "track04")), "nothing to disagree with");
    }

    #[test]
    fn a_strong_path_names_a_track_no_fingerprint_could() {
        // The normal state of affairs for a white label, a promo or an edit:
        // AcoustID has never heard of it, and the folder it is filed in has.
        let unnamed = track("", "", false);
        assert_eq!(decide_from_path(&unnamed, &from_path("Batu", "Marius")), Decision::Apply);
    }

    #[test]
    fn a_path_never_overwrites_what_the_files_own_tags_said() {
        let tagged = track("Peverelist", "Roll With The Punches", true);
        assert_eq!(decide_from_path(&tagged, &from_path("Batu", "Marius")), Decision::Reject);
    }

    #[test]
    fn a_path_that_says_what_the_track_already_says_is_no_news() {
        let named = track("Batu", "Marius", false);
        assert_eq!(decide_from_path(&named, &from_path("Batu", "Marius")), Decision::Reject);
        // And one that says something else about a name off a file name is
        // worth taking: the path is the better read of the two.
        assert_eq!(decide_from_path(&named, &from_path("Batu", "Gehenna")), Decision::Apply);
    }

    #[test]
    fn a_weak_path_is_left_alone() {
        let unnamed = track("", "", false);
        assert_eq!(decide_from_path(&unnamed, &from_path("", "track04")), Decision::Reject);
    }

    #[test]
    fn a_match_describes_itself_for_a_person_to_read() {
        let found = candidate(0.9, "Batu", "Marius");
        assert_eq!(found.describe(), "Batu — Marius (Livity Sound)");
        let bare = Match { artist: "Batu".into(), title: "Marius".into(), ..Match::default() };
        assert_eq!(bare.describe(), "Batu — Marius");
    }
}
