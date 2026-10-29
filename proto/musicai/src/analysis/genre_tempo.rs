//! The tempos club genres are played at, and finding those genres in what the
//! model heard and in what a file's tag says.
//!
//! The table is `genre_tempos.json` beside this file: 75 genres in twelve
//! families, each with the range of tempos its scene plays at, where its
//! records cluster and the other names it goes by, put together for Booth from
//! the DJ and production guides its `sources` list. It is kept as it came, so
//! that a newer one can replace it.
//!
//! Only the range is read. It is where the grid goes, which for a genre felt
//! at half is the tempo it is written at: dubstep at 140 rather than the 70 it
//! is danced to, as a DJ mixing it with grime counts it. `alt_octave_bpm`, the
//! octave analysers mistake it for, is the range halved or doubled, which is
//! what the octave rule tries anyway; see [`super::tempo::Genre`].
//!
//! A genre is found by its name, its id or one of its aliases, in a tag or a
//! Discogs style alike, so that the model and a tag that say the same thing
//! say it to the same effect.

use std::sync::LazyLock;

use serde::Deserialize;

use super::style::Style;
use super::tempo::Genre;

#[derive(Deserialize)]
struct Table {
    genres: Vec<Played>,
}

/// One genre in the table, as much of it as is read.
#[derive(Deserialize)]
struct Played {
    id: String,
    name: String,
    aliases: Vec<String>,
    bpm_min: f64,
    bpm_max: f64,
}

static TABLE: LazyLock<Vec<Played>> = LazyLock::new(|| {
    serde_json::from_str::<Table>(include_str!("genre_tempos.json"))
        .expect("the genre tempo table is the one checked in, and the tests read it")
        .genres
});

/// Names records go by that the table does not give, for genres it has.
///
/// Trip hop is not in it, and is counted as hip hop is, in hip hop's octave
/// and sometimes under its range: Jenova 7's Introspect at 82. Boom bap is
/// hip hop by another name. Halftime is drum and bass written at its tempo and
/// danced to at half, as dubstep is. And Discogs writes psytrance with a
/// hyphen, which would otherwise make it plain trance.
const ALSO: [(&str, &str); 4] = [
    ("trip hop", "hip-hop"),
    ("boom bap", "hip-hop"),
    ("halftime", "drum-and-bass"),
    ("psy trance", "psytrance"),
];

/// Every name a genre is found by, as [`words`] writes it, and which genre.
static WORDS: LazyLock<Vec<(String, usize)>> = LazyLock::new(|| {
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut add = |name: &str, at: usize| {
        let name = words(name);
        if !found.iter().any(|(word, _)| *word == name) {
            found.push((name, at));
        }
    };
    for (at, played) in TABLE.iter().enumerate() {
        add(&played.id, at);
        add(&played.name, at);
        for alias in &played.aliases {
            add(alias, at);
        }
    }
    for (name, id) in ALSO {
        add(name, index_of(id).expect("every name Booth adds is for a genre in the table"));
    }
    found
});

/// The Discogs genres whose styles are looked for in the table. Its genres are
/// club music's, and a Rock style called Hardcore is punk, not gabber.
const HEARD_IN: [&str; 5] = ["Electronic", "Hip Hop", "Funk / Soul", "Latin", "Reggae"];

/// Discogs styles that name a genre in the table but are not it, or that are
/// filed under one whose tempo they are not counted at.
///
/// Hip hop's bounce is New Orleans's, at hip hop's tempo, not the UK's at 145.
/// Miami bass and the hip hop filed as bass music are counted from about 130,
/// and cloud rap and crunk either way, so a record of theirs keeps the octave
/// the correlation chose.
const ELSEWHERE: [((&str, &str), Option<&str>); 5] = [
    (("Hip Hop", "Bounce"), Some("hip-hop")),
    (("Hip Hop", "Miami Bass"), None),
    (("Hip Hop", "Bass Music"), None),
    (("Hip Hop", "Cloud Rap"), None),
    (("Hip Hop", "Crunk"), None),
];

/// How many of the styles the model is surest of are asked. Past five the
/// activations are small and the styles are the model's guesses.
const HEARD_TOP: usize = 5;

/// A name written the one way, so that "Drum & Bass", "drum-and-bass" and
/// "Drum and Bass" are the same: lower case, `&` as "and", anything else that
/// is not a letter or a digit a space between words.
fn words(name: &str) -> String {
    name.to_lowercase()
        .replace('&', " and ")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn index_of(id: &str) -> Option<usize> {
    TABLE.iter().position(|played| played.id == id)
}

fn genre(at: usize, sure: f32) -> Genre {
    let played: &'static Played = &TABLE[at];
    Genre {
        id: &played.id,
        name: &played.name,
        from_bpm: played.bpm_min,
        to_bpm: played.bpm_max,
        sure,
    }
}

/// The genres a piece of text names, one for each part of it: a tag lists
/// several as "IDM, Jungle, Dubstep", "Trip-Hop/Lo-Fi/Swing".
///
/// Where a part names more than one, it is the one it ends with, and then the
/// longest: "Hardcore Hip-Hop" is hip hop, and "Tech House" tech house rather
/// than house, as English puts the kind of thing last.
fn named_in(text: &str) -> Vec<usize> {
    text.split([',', ';', '/', '|'])
        .filter_map(|part| {
            let part = format!(" {} ", words(part));
            WORDS
                .iter()
                .filter_map(|(word, at)| {
                    let word = format!(" {word} ");
                    part.rfind(&word).map(|from| ((from + word.len(), word.len()), *at))
                })
                .max_by_key(|&(place, _)| place)
                .map(|(_, at)| at)
        })
        .collect()
}

/// The genre a track's tempo was put in once, by the id it was kept by, for
/// measuring it again without the model.
pub fn called(id: &str) -> Option<Genre> {
    index_of(id).map(|at| genre(at, 1.0))
}

/// The genres a file's own genre tag names, each as sure as can be: it is
/// somebody's word on it.
pub fn tagged(tag: &str) -> Vec<Genre> {
    named_in(tag).into_iter().map(|at| genre(at, 1.0)).collect()
}

/// The genres among the five styles the model is surest of, each as
/// sure as the model was of the style. `styles` surest first, as
/// [`super::style::Classifier::classify`] gives them.
///
/// A style is found by its own name, and failing that by its Discogs genre's,
/// which is how Boom Bap and Turntablism are hip hop.
pub fn heard(styles: &[Style]) -> Vec<Genre> {
    styles
        .iter()
        .take(HEARD_TOP)
        .filter_map(|style| {
            let filed = ELSEWHERE.iter().find(|(filed, _)| *filed == (style.genre, style.style));
            let at = match filed {
                Some((_, id)) => id.and_then(index_of),
                None if HEARD_IN.contains(&style.genre) => {
                    let named = named_in(style.style);
                    named.first().copied().or_else(|| named_in(style.genre).first().copied())
                }
                None => None,
            };
            at.map(|at| genre(at, style.confidence))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(genres: &[Genre]) -> Vec<&'static str> {
        genres.iter().map(|genre| genre.id).collect()
    }

    fn heard_as(styles: &[(&'static str, &'static str, f32)]) -> Vec<Genre> {
        let styles: Vec<Style> = styles
            .iter()
            .map(|&(genre, style, confidence)| Style { genre, style, confidence })
            .collect();
        heard(&styles)
    }

    #[test]
    fn the_table_reads_and_no_name_in_it_is_two_genres() {
        assert_eq!(TABLE.len(), 75);
        for played in TABLE.iter() {
            assert!(played.bpm_min < played.bpm_max, "{}", played.id);
        }
        let mut seen = std::collections::HashMap::new();
        for (at, played) in TABLE.iter().enumerate() {
            for name in [&played.id, &played.name].into_iter().chain(&played.aliases) {
                if let Some(other) = seen.insert(words(name), at) {
                    assert_eq!(other, at, "{name} is {} and {}", TABLE[other].id, played.id);
                }
            }
        }
    }

    #[test]
    fn a_tag_names_a_genre_however_it_is_written() {
        assert_eq!(ids(&tagged("Drum & Bass")), ["drum-and-bass"]);
        assert_eq!(ids(&tagged("drum'n'bass")), ["drum-and-bass"]);
        assert_eq!(ids(&tagged("D&B")), ["drum-and-bass"]);
        assert_eq!(ids(&tagged("R&B")), ["rnb"]);
        assert_eq!(ids(&tagged("Instrumental Hip-Hop")), ["hip-hop"]);
        assert_eq!(ids(&tagged("Hip-Hop/Trap")), ["hip-hop", "hip-hop-trap"]);
        assert_eq!(ids(&tagged("Trip-Hop/Lo-Fi/Swing")), ["hip-hop"]);
        assert_eq!(ids(&tagged("IDM, Jungle, Dubstep, Experimental")), ["jungle", "dubstep"]);
        assert!(tagged("Electronic").is_empty());
        assert!(tagged("Trailer").is_empty());
    }

    #[test]
    fn a_tag_naming_two_is_the_kind_it_ends_with() {
        assert_eq!(ids(&tagged("Tech House")), ["tech-house"]);
        assert_eq!(ids(&tagged("minimal techno")), ["minimal-techno"]);
        assert_eq!(ids(&tagged("Hardcore Hip-Hop")), ["hip-hop"]);
        assert_eq!(ids(&tagged("Melodic House & Techno")), ["melodic-house"]);
    }

    #[test]
    fn a_style_is_found_by_its_name_and_then_by_its_discogs_genre() {
        let found = heard_as(&[
            ("Electronic", "Drum n Bass", 0.5),
            ("Hip Hop", "Boom Bap", 0.4),
            ("Electronic", "Psy-Trance", 0.3),
            ("Electronic", "Ambient", 0.2),
            ("Hip Hop", "Trap", 0.1),
        ]);
        assert_eq!(ids(&found), ["drum-and-bass", "hip-hop", "psytrance", "hip-hop-trap"]);
        assert_eq!(found[0].sure, 0.5, "as sure as the model was");
    }

    #[test]
    fn a_style_that_is_not_the_genre_it_names_says_nothing_or_its_own() {
        let found = heard_as(&[
            ("Rock", "Hardcore", 0.5),
            ("Hip Hop", "Bounce", 0.4),
            ("Hip Hop", "Miami Bass", 0.3),
            ("Hip Hop", "Cloud Rap", 0.2),
        ]);
        assert_eq!(ids(&found), ["hip-hop"]);
    }

    #[test]
    fn only_the_five_styles_the_model_is_surest_of_are_asked() {
        let mut styles = vec![("Electronic", "Ambient", 0.5); 5];
        styles.push(("Electronic", "Drum n Bass", 0.4));
        assert!(heard_as(&styles).is_empty());
    }

    #[test]
    fn a_genre_kept_by_its_id_is_found_again_and_an_unknown_one_is_not() {
        let found = called("drum-and-bass").unwrap();
        assert_eq!((found.name, found.from_bpm, found.to_bpm), ("Drum & Bass", 170.0, 180.0));
        assert_eq!(called("polka"), None);
        assert_eq!(called("drum and bass"), None, "by its id, not its name");
    }
}
