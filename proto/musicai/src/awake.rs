//! Keeping the machine from going to sleep while a drive is being written.
//!
//! A laptop that sleeps halfway through a long write leaves a stick with half a
//! drive on it, and the research log has seen that more than once. Each
//! platform has its own way to say "not now", and none of them is a library
//! call this crate already makes, so this asks the one the platform ships: a
//! process that holds the machine awake for as long as it lives, tied to this
//! one so that a crash cannot leave the machine held awake for good.

/// The machine held awake until this is dropped.
pub struct Awake {
    holder: Holder,
}

enum Holder {
    /// A process that holds the machine awake while it runs.
    Process(std::process::Child),
    /// Windows holds it per thread, and lets it go when told.
    #[cfg(windows)]
    Thread,
    /// Nothing could be found to hold it.
    Nothing,
}

impl Awake {
    /// Hold the machine awake for `why`, and say how, or that it could not.
    pub fn hold(why: &str) -> (Self, String) {
        match hold(why) {
            Ok((holder, how)) => {
                (Self { holder }, format!("kept the machine awake for {why} ({how})"))
            }
            Err(reason) => (
                Self { holder: Holder::Nothing },
                format!(
                    "could not keep the machine awake for {why} ({reason}); a machine that sleeps \
                     partway leaves the drive half written"
                ),
            ),
        }
    }

    /// Whether anything is holding the machine awake.
    pub fn holding(&self) -> bool {
        !matches!(self.holder, Holder::Nothing)
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        match &mut self.holder {
            Holder::Process(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(windows)]
            Holder::Thread => unsafe {
                windows::SetThreadExecutionState(windows::ES_CONTINUOUS);
            },
            Holder::Nothing => {}
        }
    }
}

/// Start a process that holds the machine awake, and check it is still running
/// a moment later: a tool that is installed and refuses — no session bus, say —
/// exits at once, and that is not holding anything.
fn spawn(program: &str, args: &[String]) -> Result<Holder, String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    match child.try_wait() {
        Ok(None) => Ok(Holder::Process(child)),
        Ok(Some(status)) => Err(format!("{program} exited at once ({status})")),
        Err(e) => Err(format!("{program}: {e}")),
    }
}

#[cfg(target_os = "macos")]
fn hold(_why: &str) -> Result<(Holder, &'static str), String> {
    // Idle sleep and disk sleep, and only while this process lives.
    let pid = std::process::id().to_string();
    let args = ["-i", "-m", "-w", &pid].map(str::to_string);
    spawn("caffeinate", &args).map(|holder| (holder, "caffeinate"))
}

#[cfg(target_os = "linux")]
fn hold(why: &str) -> Result<(Holder, &'static str), String> {
    // systemd's inhibitor lock, held by a command that ends when this process
    // does rather than one that would outlive a crash.
    let pid = std::process::id().to_string();
    let args = [
        "--what=sleep:idle".to_string(),
        "--who=Booth".to_string(),
        format!("--why={why}"),
        "--mode=block".to_string(),
        "tail".to_string(),
        format!("--pid={pid}"),
        "-f".to_string(),
        "/dev/null".to_string(),
    ];
    spawn("systemd-inhibit", &args).map(|holder| (holder, "systemd-inhibit"))
}

#[cfg(windows)]
fn hold(_why: &str) -> Result<(Holder, &'static str), String> {
    // SAFETY: a plain call with flags; it returns zero when it fails.
    let previous = unsafe {
        windows::SetThreadExecutionState(windows::ES_CONTINUOUS | windows::ES_SYSTEM_REQUIRED)
    };
    match previous {
        0 => Err("SetThreadExecutionState refused".to_string()),
        _ => Ok((Holder::Thread, "SetThreadExecutionState")),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn hold(_why: &str) -> Result<(Holder, &'static str), String> {
    Err("no way to ask on this platform".to_string())
}

#[cfg(windows)]
mod windows {
    pub const ES_CONTINUOUS: u32 = 0x8000_0000;
    pub const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn SetThreadExecutionState(flags: u32) -> u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holding_the_machine_awake_says_how_or_why_not_and_lets_go_when_dropped() {
        let (awake, said) = Awake::hold("a test");
        match awake.holding() {
            true => assert!(said.starts_with("kept the machine awake for a test"), "{said}"),
            false => assert!(said.starts_with("could not keep the machine awake"), "{said}"),
        }
        drop(awake);
    }

    #[cfg(unix)]
    #[test]
    fn a_holding_process_is_ended_with_the_hold_and_not_left_running() {
        let Holder::Process(child) = spawn("sleep", &["30".to_string()]).unwrap() else {
            panic!("sleep did not start");
        };
        let pid = child.id();
        drop(Awake { holder: Holder::Process(child) });
        // Reaped, not merely signalled: no process by that id is left.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        assert!(!alive, "the holder is still running");
    }

    #[test]
    fn a_holder_that_exits_at_once_is_not_taken_for_one_that_holds() {
        let refused = spawn("false", &[]);
        assert!(refused.is_err());
        assert!(spawn("there-is-no-such-program", &[]).is_err());
    }
}
