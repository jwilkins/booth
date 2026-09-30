//! Handing a drive image to a CDJ-3000 emulator.
//!
//! [cdj3k-emu](https://github.com/nsaintot/cdj3k-emu) boots real CDJ-3000
//! firmware under QEMU and gives it a virtual USB slot. Attaching an image to
//! that slot is a QEMU machine-protocol call — `blockdev-change-medium` on the
//! block device the emulator calls `usb0` — and QEMU listens for those on a
//! TCP port, so a program that has just written an image can put it in the slot
//! itself instead of asking somebody to go and find the file.
//!
//! **What this cannot do.** The emulator's own attach does two things: the
//! medium change here, and a nudge to the guest over its `cdj3k.cfg`
//! virtio-serial port that runs the in-guest mount scripts. Only the first is
//! reachable from outside. A real player notices a stick going in by itself, so
//! the firmware may well notice a medium change the same way — but that is a
//! reasonable expectation, not a measured fact, and nothing here has been run
//! against the emulator: it is Apple Silicon macOS only and needs Pioneer
//! firmware this project does not have. Everything below is tested against a
//! QEMU-speaking fake, which proves the conversation is right and proves
//! nothing about what the firmware does with it.
//!
//! The caller is expected to have somewhere to fall back to. [`attach`] says
//! plainly whether it got as far as the emulator, so "the image is written,
//! go and attach it" stays available when nothing is listening.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

/// The block device the emulator gives its virtual USB slot.
const USB_DRIVE_ID: &str = "usb0";

/// Where the emulator's first instance listens for machine-protocol calls.
///
/// Its config numbers instances from this, one port each, so a second
/// emulator is 4446 — which is why this is a starting point and not the
/// answer.
pub const DEFAULT_PORT: u16 = 4445;

/// Long enough to cross a loopback socket, short enough that a wrong port is
/// an answer rather than a wait. Nothing here talks to another machine.
const PATIENCE: Duration = Duration::from_secs(5);

/// Why an image did not reach the emulator.
#[derive(Debug)]
pub enum Refused {
    /// Nothing was listening. Almost always the emulator not running, or
    /// running as a different instance on a different port.
    NotListening(std::io::Error),
    /// It was listening and would not do it. The string is what it said,
    /// which is the only part of this worth showing somebody.
    Said(String),
    /// The conversation itself went wrong: a socket that closed, or a reply
    /// that was not the protocol.
    Broke(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotListening(e) => write!(f, "nothing is listening ({e})"),
            Refused::Said(what) => write!(f, "the emulator refused: {what}"),
            Refused::Broke(what) => write!(f, "the emulator stopped making sense: {what}"),
        }
    }
}

impl std::error::Error for Refused {}

/// Put an image in the emulator's USB slot.
///
/// `at` is where QEMU is listening, usually `127.0.0.1:4445`. The path is sent
/// as given and is resolved by the emulator, not here, so it has to be one that
/// process can see — an absolute path, since nothing says what its working
/// directory is.
pub fn attach(at: impl ToSocketAddrs, image: &Path) -> Result<(), Refused> {
    let path = image.to_string_lossy().into_owned();
    let mut talking = Qemu::connect(at)?;
    // The greeting and the handshake come first; QEMU answers nothing else
    // until capabilities have been negotiated, so a command sent before this
    // is an error rather than a fast path.
    talking.greeting()?;
    talking.call("qmp_capabilities", serde_json::json!({}))?;
    talking.call(
        "blockdev-change-medium",
        serde_json::json!({ "id": USB_DRIVE_ID, "filename": path, "format": "raw" }),
    )?;
    Ok(())
}

/// What became of an image that was sent.
#[derive(Debug)]
pub enum Sent {
    /// It went into the slot. Whether the firmware then mounted it is the one
    /// thing this cannot say — see the note at the top of this file.
    Attached,
    /// Nothing was listening, so the file was pointed at in a file manager
    /// instead and the attaching is somebody's to do by hand.
    Revealed,
    /// Nothing was listening and the file could not be pointed at either. The
    /// image is still written; only the handing over failed.
    Written(Refused),
}

/// Hand an image to the emulator, or failing that, point at it.
///
/// The fallback is the point. The emulator is not always running, is not
/// always the instance on this port, and may refuse; none of that should leave
/// somebody with a written image and no idea where it went. What this returns
/// says which of the three happened, so the caller can say so too.
pub fn send(at: impl ToSocketAddrs, image: &Path) -> Sent {
    match attach(at, image) {
        Ok(()) => Sent::Attached,
        // A refusal is worth reporting rather than working around: the
        // emulator is there and said no, and revealing the file would bury
        // that under a file manager opening.
        Err(refused @ Refused::Said(_)) => Sent::Written(refused),
        Err(refused) => match reveal(image) {
            true => Sent::Revealed,
            false => Sent::Written(refused),
        },
    }
}

/// Show a file where the person running this can see it.
///
/// Best effort, and deliberately not an error when it fails: this is the
/// consolation prize for not reaching the emulator, and a file manager that
/// will not open is not worth a second failure on top of the first.
fn reveal(image: &Path) -> bool {
    let (program, arguments): (&str, Vec<std::ffi::OsString>) = if cfg!(target_os = "macos") {
        ("open", vec!["-R".into(), image.as_os_str().to_owned()])
    } else if cfg!(target_os = "windows") {
        ("explorer", vec![format!("/select,{}", image.display()).into()])
    } else {
        // No `-R` anywhere else, so the folder is the nearest thing to it.
        let folder = image.parent().unwrap_or(image);
        ("xdg-open", vec![folder.as_os_str().to_owned()])
    };
    std::process::Command::new(program)
        .args(arguments)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|done| done.success())
        .unwrap_or(false)
}

/// One conversation with QEMU's machine protocol.
///
/// Line-delimited JSON both ways, and a reply per command, which is the whole
/// protocol as far as this needs it.
struct Qemu {
    reading: BufReader<TcpStream>,
    writing: TcpStream,
}

impl Qemu {
    fn connect(at: impl ToSocketAddrs) -> Result<Self, Refused> {
        // Every address the name resolves to, because `127.0.0.1` and
        // `localhost` are not always the same socket and the second one can
        // offer a v6 address first.
        let mut last = None;
        let addresses = at.to_socket_addrs().map_err(Refused::NotListening)?;
        for address in addresses {
            match TcpStream::connect_timeout(&address, PATIENCE) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(PATIENCE)).ok();
                    stream.set_write_timeout(Some(PATIENCE)).ok();
                    let reading =
                        BufReader::new(stream.try_clone().map_err(Refused::NotListening)?);
                    return Ok(Self { reading, writing: stream });
                }
                Err(e) => last = Some(e),
            }
        }
        Err(Refused::NotListening(last.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no address to try")
        })))
    }

    /// The `QMP` banner QEMU sends before it will take anything.
    fn greeting(&mut self) -> Result<(), Refused> {
        let line = self.line()?;
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(value) if value.get("QMP").is_some() => Ok(()),
            _ => Err(Refused::Broke(format!("said {} instead of hello", first_words(&line)))),
        }
    }

    /// Send one command and wait for the reply that belongs to it.
    fn call(&mut self, execute: &str, arguments: serde_json::Value) -> Result<(), Refused> {
        let command = serde_json::json!({ "execute": execute, "arguments": arguments });
        writeln!(self.writing, "{command}").map_err(|e| Refused::Broke(e.to_string()))?;
        self.writing.flush().map_err(|e| Refused::Broke(e.to_string()))?;

        // Events arrive unasked and in the middle of things, so the reply is
        // the next line that is one — anything carrying `event` is QEMU
        // narrating and is not what was asked for.
        loop {
            let line = self.line()?;
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                return Err(Refused::Broke(format!("{execute} answered {}", first_words(&line))));
            };
            if value.get("event").is_some() {
                continue;
            }
            if let Some(trouble) = value.get("error") {
                let said = trouble
                    .get("desc")
                    .and_then(|d| d.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| trouble.to_string());
                return Err(Refused::Said(said));
            }
            if value.get("return").is_some() {
                return Ok(());
            }
            return Err(Refused::Broke(format!("{execute} answered {}", first_words(&line))));
        }
    }

    fn line(&mut self) -> Result<String, Refused> {
        let mut line = String::new();
        match self.reading.read_line(&mut line) {
            Ok(0) => Err(Refused::Broke("it hung up".to_string())),
            Ok(_) => Ok(line),
            Err(e) => Err(Refused::Broke(e.to_string())),
        }
    }
}

/// Enough of a line to say what arrived, for a message somebody has to read.
fn first_words(line: &str) -> String {
    let trimmed = line.trim();
    match trimmed.char_indices().nth(80) {
        Some((at, _)) => format!("{}…", &trimmed[..at]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    /// A stand-in for QEMU: greets, then answers each command from a script.
    ///
    /// It records what it was asked, because the point of most of these tests
    /// is the shape of the command rather than what comes back — a call that
    /// succeeds against a fake but names the wrong device is worth nothing.
    fn fake(replies: Vec<&'static str>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reading = BufReader::new(stream.try_clone().unwrap());
            let mut writing = stream;
            writeln!(writing, r#"{{"QMP":{{"version":{{}},"capabilities":[]}}}}"#).unwrap();
            writing.flush().unwrap();

            let mut heard = Vec::new();
            for reply in replies {
                let mut line = String::new();
                if reading.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                heard.push(line.trim().to_string());
                writeln!(writing, "{reply}").unwrap();
                writing.flush().unwrap();
            }
            heard
        });
        (port, handle)
    }

    #[test]
    fn an_image_goes_into_the_slot_the_emulator_calls_usb0() {
        let (port, server) = fake(vec![r#"{"return":{}}"#, r#"{"return":{}}"#]);
        let sent = attach(("127.0.0.1", port), Path::new("/tmp/rekordbox.img"));
        let heard = server.join().unwrap();
        assert!(sent.is_ok(), "{sent:?}");

        // Capabilities first, or QEMU answers nothing.
        assert!(heard[0].contains("qmp_capabilities"), "{heard:?}");

        // And the command itself, field by field: the wrong device id or the
        // wrong format would pass a test that only checked it succeeded.
        let asked: serde_json::Value = serde_json::from_str(&heard[1]).unwrap();
        assert_eq!(asked["execute"], "blockdev-change-medium");
        assert_eq!(asked["arguments"]["id"], "usb0");
        assert_eq!(asked["arguments"]["filename"], "/tmp/rekordbox.img");
        assert_eq!(asked["arguments"]["format"], "raw");
    }

    #[test]
    fn a_refusal_is_reported_rather_than_worked_around() {
        // The emulator is there and said no. Opening a file manager on top of
        // that would bury the one thing worth reading.
        let (port, server) = fake(vec![
            r#"{"return":{}}"#,
            r#"{"error":{"class":"GenericError","desc":"tray is locked"}}"#,
        ]);
        let what = send(("127.0.0.1", port), Path::new("/tmp/rekordbox.img"));
        server.join().unwrap();
        match what {
            Sent::Written(Refused::Said(said)) => assert!(said.contains("tray"), "{said}"),
            other => panic!("a refusal did not survive send: {other:?}"),
        }
    }

    #[test]
    fn an_emulator_that_is_not_running_still_leaves_the_image_accounted_for() {
        // Whether a file manager opens is the machine's business — in a test
        // container there is none — so what matters is that it is one of the
        // two answers that say where the image is, and never a silent success.
        let spare = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = spare.local_addr().unwrap().port();
        drop(spare);

        let what = send(("127.0.0.1", port), Path::new("/tmp/rekordbox.img"));
        assert!(
            matches!(what, Sent::Revealed | Sent::Written(Refused::NotListening(_))),
            "an unreachable emulator did not say so: {what:?}"
        );
    }

    #[test]
    fn nothing_listening_is_told_apart_from_a_refusal() {
        // The one the caller has to act on differently: the emulator not being
        // up is a thing to fall back from, and a refusal is a thing to report.
        let spare = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = spare.local_addr().unwrap().port();
        drop(spare);

        let sent = attach(("127.0.0.1", port), Path::new("/tmp/rekordbox.img"));
        assert!(matches!(sent, Err(Refused::NotListening(_))), "{sent:?}");
    }

    #[test]
    fn what_the_emulator_said_no_with_is_what_comes_back() {
        let (port, server) = fake(vec![
            r#"{"return":{}}"#,
            r#"{"error":{"class":"DeviceNotFound","desc":"Block device 'usb0' not found"}}"#,
        ]);
        let sent = attach(("127.0.0.1", port), Path::new("/tmp/rekordbox.img"));
        server.join().unwrap();

        match sent {
            Err(Refused::Said(what)) => assert!(what.contains("usb0"), "{what}"),
            other => panic!("a refusal did not come back as one: {other:?}"),
        }
    }

    #[test]
    fn qemu_talking_over_the_reply_does_not_lose_it() {
        // Events arrive unasked and land between a command and its answer, so
        // a client that reads exactly one line per command reads an event and
        // calls it the reply.
        let (port, server) = fake(vec![
            r#"{"return":{}}"#,
            r#"{"event":"DEVICE_TRAY_MOVED","timestamp":{"seconds":1,"microseconds":0}}"#,
        ]);
        // The event is answered by the fake in place of the reply, so the real
        // reply is the next thing it sends: another round of the script.
        let joined = std::thread::spawn(move || attach(("127.0.0.1", port), Path::new("/x.img")));
        let heard = server.join().unwrap();
        // The fake ran out of script after the event, so the socket closes and
        // this ends as a broken conversation rather than a wrong success —
        // which is the point: the event was not mistaken for the reply.
        let sent = joined.join().unwrap();
        assert!(matches!(sent, Err(Refused::Broke(_))), "an event was taken for a reply: {sent:?}");
        assert_eq!(heard.len(), 2, "{heard:?}");
    }
}
