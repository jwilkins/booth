//! Every control in the window says what it does.
//!
//! Checked by reading the source rather than by driving the window, because
//! what is being promised is a property of all of them at once and a UI test
//! can only ever visit the ones somebody remembered to write a test for. A
//! button added next year with no hover text fails this without anybody having
//! to think of it.
//!
//! The one thing it cannot tell is whether the words are any good.

use std::path::Path;

/// The calls that put something clickable, typeable or draggable on screen.
///
/// Labels are not here. A label is text, and text that happens to be clickable
/// is handled where it is written — the two in the sidebar have hover text of
/// their own, and there is no way to tell those apart from the hundreds that
/// are only text.
const CONTROLS: [&str; 12] = [
    "ui.button(",
    "ui.small_button(",
    "ui.checkbox(",
    "ui.radio(",
    "ui.radio_value(",
    "ui.selectable_label(",
    "ui.selectable_value(",
    "ui.toggle_value(",
    "ui.menu_button(",
    "ui.link(",
    "egui::Slider::new(",
    "egui::TextEdit::singleline(",
];

/// How far past the call to look for the hover text.
///
/// Generous, because a control is often written over a dozen lines of builder
/// calls and the hover comes at the end of them. A menu is the exception —
/// its body is as long as the menu — so those carry their hover on the line
/// after the closing brace and are found by the same look-ahead widened once.
const LOOK_AHEAD: usize = 26;

#[test]
fn every_control_in_the_window_says_what_it_does() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bare = Vec::new();

    for entry in std::fs::read_dir(&src).expect("the source folder should be readable") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a readable file");
        let lines: Vec<&str> = text.lines().collect();
        // Tests build their own controls to press, and a test's button is not
        // part of the window.
        let tests_from = lines.iter().position(|line| line.trim() == "mod tests {");
        for (at, line) in lines.iter().enumerate() {
            if tests_from.is_some_and(|start| at > start) {
                break;
            }
            if !CONTROLS.iter().any(|call| line.contains(call)) {
                continue;
            }
            let ahead = lines[at..(at + LOOK_AHEAD).min(lines.len())].join("\n");
            if ahead.contains("on_hover_text") || ahead.contains("on_disabled_hover_text") {
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            bare.push(format!("{name}:{}: {}", at + 1, line.trim()));
        }
    }

    assert!(
        bare.is_empty(),
        "{} control(s) with nothing to say when the pointer rests on them:\n{}",
        bare.len(),
        bare.join("\n")
    );
}
