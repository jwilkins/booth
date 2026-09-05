//! Finding demucs, and offering to install it when it is missing.
//!
//! Installing software on someone's machine is not something to do quietly, so
//! the rules here are deliberately strict:
//!
//! - nothing is installed without the user typing `y` at a prompt that first
//!   lists the exact commands that will run;
//! - the prompt only appears on a terminal, so a script or CI run gets a clear
//!   error and instructions instead of hanging on a read that never returns;
//! - `--install-demucs never` disables the offer entirely.
//!
//! Only macOS is wired up. Other platforms report what to run and leave it to
//! the user, which is what happened everywhere before this existed.

use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// What to do when demucs is not installed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum InstallPolicy {
    /// Offer to install it, when running on a terminal.
    Ask,
    /// Never offer; report how to install it and stop.
    Never,
    /// Install without asking. For scripts that have already decided.
    Yes,
}

/// The platforms this can install on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Other,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }
}

/// What the planner is allowed to know about the machine.
///
/// Passing this in rather than probing inside [`plan`] is what makes the
/// decision testable: every branch can be exercised from any platform, which
/// matters for logic that only ever executes on macOS.
#[derive(Clone, Debug)]
pub struct Environment {
    pub os: Os,
    pub has_uv: bool,
    pub has_pipx: bool,
    pub has_brew: bool,
    pub interactive: bool,
    pub policy: InstallPolicy,
}

impl Environment {
    pub fn detect(policy: InstallPolicy) -> Self {
        Self {
            os: Os::current(),
            has_uv: find_executable("uv").is_some(),
            has_pipx: find_executable("pipx").is_some(),
            has_brew: find_executable("brew").is_some(),
            interactive: std::io::stdin().is_terminal(),
            policy,
        }
    }
}

/// One command that would be run, with a plain description of why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    pub why: &'static str,
}

impl Step {
    fn new(program: &str, args: &[&str], why: &'static str) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            why,
        }
    }

    /// The command as the user would type it, for the consent prompt.
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// What to do about a missing demucs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Ask first, then run these steps.
    Offer(Vec<Step>),
    /// Run these steps without asking, because the user already said so.
    Proceed(Vec<Step>),
    /// Cannot help here. The string explains what the user should do.
    Manual(String),
}

/// The advice given when we cannot install it ourselves.
fn manual_instructions(reason: &str) -> Plan {
    Plan::Manual(format!(
        "{reason}. Install it with `pipx install demucs` (or `pip install demucs`), then run \
         this again — or use `--backend dsp` for the built-in separator, which needs nothing \
         installed"
    ))
}

/// Decide what to do about a missing demucs, without doing any of it.
pub fn plan(env: &Environment) -> Plan {
    if env.policy == InstallPolicy::Never {
        return manual_instructions("demucs is not installed and --install-demucs never was given");
    }
    if env.os != Os::MacOs {
        return manual_instructions(
            "demucs is not installed, and automatic installation is only wired up for macOS",
        );
    }

    // Every path installs numpy alongside demucs. Demucs imports it but does
    // not declare it as a dependency, and torch stopped pulling it in
    // transitively, so a plain `install demucs` produces something that fails
    // on first run with `No module named 'numpy'`.
    let steps = if env.has_uv {
        vec![Step::new(
            "uv",
            &["tool", "install", "--force", "demucs", "--with", "numpy"],
            "installs demucs, with the numpy it forgets to depend on",
        )]
    } else if env.has_pipx {
        vec![
            Step::new(
                "pipx",
                &["install", "demucs"],
                "installs demucs into its own isolated environment",
            ),
            Step::new(
                "pipx",
                &["inject", "demucs", "numpy"],
                "adds the numpy demucs imports but does not depend on",
            ),
        ]
    } else if env.has_brew {
        vec![
            Step::new(
                "brew",
                &["install", "pipx"],
                "installs pipx, which demucs is installed with",
            ),
            Step::new(
                "pipx",
                &["install", "demucs"],
                "installs demucs into its own isolated environment",
            ),
            Step::new(
                "pipx",
                &["inject", "demucs", "numpy"],
                "adds the numpy demucs imports but does not depend on",
            ),
        ]
    } else {
        return manual_instructions(
            "demucs is not installed, and none of uv, pipx or Homebrew is available to install \
             it with",
        );
    };

    // Only prompt where someone can actually answer. A read from a redirected
    // stdin would either hang or return nothing, and treating nothing as
    // consent is exactly the wrong default.
    match env.policy {
        InstallPolicy::Yes => Plan::Proceed(steps),
        InstallPolicy::Ask if env.interactive => Plan::Offer(steps),
        InstallPolicy::Ask => manual_instructions(
            "demucs is not installed, and there is no terminal to ask on (pass \
             --install-demucs yes to install without a prompt)",
        ),
        InstallPolicy::Never => unreachable!("handled above"),
    }
}

/// Text of the consent prompt.
///
/// Split out so the wording is covered by tests: it is the only thing standing
/// between the user and commands running on their machine, so it has to name
/// every one of them.
pub fn consent_prompt(steps: &[Step]) -> String {
    let mut text = String::from("demucs is not installed. Install it now?\n\n");
    for step in steps {
        text.push_str(&format!("    {}    # {}\n", step.command_line(), step.why));
    }
    text.push_str(
        "\nThis installs software on your machine, and demucs downloads about 300 MB of model \
         weights the first time it runs.\n\nProceed? [y/N] ",
    );
    text
}

/// Ask, and return whether the answer was yes.
///
/// Anything other than `y` or `yes` is a no, including an empty line, so
/// pressing return declines.
fn ask(steps: &[Step]) -> Result<bool> {
    print!("{}", consent_prompt(steps));
    std::io::stdout().flush().ok();

    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).context("reading your answer")?;
    Ok(is_yes(&answer))
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Run the steps, showing each one as it goes.
fn run(steps: &[Step]) -> Result<()> {
    for step in steps {
        println!("  running {}", step.command_line());
        let status = Command::new(&step.program)
            .args(&step.args)
            .status()
            .with_context(|| format!("could not run {}", step.command_line()))?;
        if !status.success() {
            bail!("{} failed with {status}", step.command_line());
        }
    }
    Ok(())
}

/// Find an executable on `PATH`.
pub fn find_executable(name: impl AsRef<OsStr>) -> Option<PathBuf> {
    let name = name.as_ref();

    // An explicit path is used as given rather than searched for.
    let as_path = Path::new(name);
    if as_path.components().count() > 1 {
        return as_path.is_file().then(|| as_path.to_path_buf());
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join(name)).find(|p| p.is_file())
    })
}

/// Where pipx puts the binaries it installs.
///
/// A freshly installed demucs will not be on the `PATH` this process inherited
/// — `pipx ensurepath` only affects shells started afterwards — so the binary
/// has to be looked for where pipx actually put it.
fn pipx_bin_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PIPX_BIN_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("bin"))
}

/// Look for demucs, including where an install would have just put it.
pub fn locate(program: &OsStr) -> Option<PathBuf> {
    find_executable(program).or_else(|| {
        // Only worth checking pipx's directory for the default name; an
        // explicit --demucs-bin means the user knows where it is.
        if program != OsStr::new("demucs") {
            return None;
        }
        let candidate = pipx_bin_dir()?.join("demucs");
        candidate.is_file().then_some(candidate)
    })
}

/// Make sure demucs is available, installing it with consent if it is not.
///
/// Returns the path to use, which may differ from `program` when a fresh pipx
/// install has landed somewhere not yet on `PATH`.
pub fn ensure_available(program: &OsStr, policy: InstallPolicy) -> Result<OsString> {
    if let Some(found) = locate(program) {
        return Ok(found.into_os_string());
    }

    let env = Environment::detect(policy);
    let steps = match plan(&env) {
        Plan::Manual(message) => bail!("{message}"),
        Plan::Offer(steps) => {
            if !ask(&steps)? {
                bail!(
                    "not installing demucs. Use `--backend dsp` for the built-in separator, or \
                     install demucs yourself with `pipx install demucs`"
                );
            }
            steps
        }
        Plan::Proceed(steps) => steps,
    };

    run(&steps)?;

    locate(program).map(PathBuf::into_os_string).ok_or_else(|| {
        anyhow::anyhow!(
            "the install commands succeeded but demucs still cannot be found. It may be \
             somewhere not on your PATH — try `pipx ensurepath` in a new shell, or pass \
             --demucs-bin with its location"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: Os, has_pipx: bool, has_brew: bool, interactive: bool) -> Environment {
        Environment {
            os,
            has_uv: false,
            has_pipx,
            has_brew,
            interactive,
            policy: InstallPolicy::Ask,
        }
    }

    /// Every install route must bring numpy with it. Demucs imports numpy but
    /// does not declare it, so installing demucs alone yields a build that
    /// dies on first run — which is exactly what happened to a real user.
    fn assert_installs_numpy(steps: &[Step]) {
        let all: String = steps.iter().map(|s| s.command_line()).collect::<Vec<_>>().join(" ; ");
        assert!(all.contains("numpy"), "no step installs numpy: {all}");
    }

    #[test]
    fn prefers_uv_when_it_is_available() {
        let mut e = env(Os::MacOs, true, true, true);
        e.has_uv = true;
        let Plan::Offer(steps) = plan(&e) else { panic!("expected an offer") };
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].command_line(), "uv tool install --force demucs --with numpy");
        assert_installs_numpy(&steps);
    }

    #[test]
    fn uses_pipx_when_it_is_available() {
        let plan = plan(&env(Os::MacOs, true, true, true));
        let Plan::Offer(steps) = plan else { panic!("expected an offer, got {plan:?}") };
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].command_line(), "pipx install demucs");
        assert_eq!(steps[1].command_line(), "pipx inject demucs numpy");
        assert_installs_numpy(&steps);
    }

    #[test]
    fn installs_pipx_first_when_only_homebrew_is_present() {
        let plan = plan(&env(Os::MacOs, false, true, true));
        let Plan::Offer(steps) = plan else { panic!("expected an offer, got {plan:?}") };
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].command_line(), "brew install pipx");
        assert_eq!(steps[1].command_line(), "pipx install demucs");
        assert_installs_numpy(&steps);
    }

    #[test]
    fn every_install_route_brings_numpy() {
        for (uv, pipx, brew) in [(true, true, true), (false, true, true), (false, false, true)] {
            let mut e = env(Os::MacOs, pipx, brew, true);
            e.has_uv = uv;
            let Plan::Offer(steps) = plan(&e) else { panic!("expected an offer for {e:?}") };
            assert_installs_numpy(&steps);
        }
    }

    #[test]
    fn gives_up_gracefully_with_neither_installer() {
        let plan = plan(&env(Os::MacOs, false, false, true));
        let Plan::Manual(message) = plan else { panic!("expected manual advice, got {plan:?}") };
        assert!(message.contains("none of uv, pipx or Homebrew"), "{message}");
        // And still points at the option that always works.
        assert!(message.contains("--backend dsp"), "{message}");
    }

    #[test]
    fn only_offers_on_macos() {
        let plan = plan(&env(Os::Other, true, true, true));
        let Plan::Manual(message) = plan else { panic!("expected manual advice, got {plan:?}") };
        assert!(message.contains("only wired up for macOS"), "{message}");
    }

    #[test]
    fn never_policy_refuses_even_when_it_could() {
        let mut e = env(Os::MacOs, true, true, true);
        e.policy = InstallPolicy::Never;
        let Plan::Manual(message) = plan(&e) else { panic!("expected manual advice") };
        assert!(message.contains("--install-demucs never"), "{message}");
    }

    #[test]
    fn does_not_prompt_when_there_is_no_terminal() {
        // A script or CI run must get an error rather than a hung read.
        let plan = plan(&env(Os::MacOs, true, true, false));
        let Plan::Manual(message) = plan else { panic!("expected manual advice, got {plan:?}") };
        assert!(message.contains("no terminal"), "{message}");
        assert!(message.contains("--install-demucs yes"), "{message}");
    }

    #[test]
    fn yes_policy_skips_the_prompt_without_a_terminal() {
        let mut e = env(Os::MacOs, true, true, false);
        e.policy = InstallPolicy::Yes;
        let Plan::Proceed(steps) = plan(&e) else { panic!("expected to proceed") };
        assert_eq!(steps[0].command_line(), "pipx install demucs");
    }

    #[test]
    fn only_an_explicit_yes_counts_as_consent() {
        for yes in ["y", "Y", "yes", "YES", " yes \n"] {
            assert!(is_yes(yes), "{yes:?} should have been accepted");
        }
        // Pressing return declines, and so does anything ambiguous.
        for no in ["", "\n", "n", "no", "sure", "ok", "yep", "1"] {
            assert!(!is_yes(no), "{no:?} should not have been treated as consent");
        }
    }

    #[test]
    fn the_prompt_names_every_command_and_the_real_cost() {
        let steps = vec![
            Step::new("brew", &["install", "pipx"], "installs pipx"),
            Step::new("pipx", &["install", "demucs"], "installs demucs"),
        ];
        let prompt = consent_prompt(&steps);

        assert!(prompt.contains("brew install pipx"), "{prompt}");
        assert!(prompt.contains("pipx install demucs"), "{prompt}");
        // The model download is the part people are not expecting.
        assert!(prompt.contains("300 MB"), "{prompt}");
        // Defaults to no.
        assert!(prompt.trim_end().ends_with("[y/N]"), "{prompt}");
    }

    #[test]
    fn finds_an_executable_on_the_path() {
        // `sh` exists on every platform this runs on.
        assert!(find_executable("sh").is_some());
        assert!(find_executable("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn an_explicit_path_is_taken_as_given() {
        assert!(find_executable("/bin/sh").is_some());
        assert!(find_executable("/bin/definitely-not-here").is_none());
    }
}
