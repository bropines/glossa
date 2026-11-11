//! A module's process together with everything it started.
//!
//! A process module may start helpers of its own (a model server, a
//! language runtime's workers). Killing only the program Glossa started
//! leaves those running with nobody to stop them, so a module's process is
//! put where the whole family can be ended at once: its own process group on
//! Unix (`unix.rs`), a job object on Windows (`win.rs`).
//!
//! The calls sit here rather than in `glossa-platform` for the reason the
//! OS OCR engines and the system voice do: the engine, and so this crate,
//! does not depend on it. The layout is the same, one file per system
//! behind functions that compile everywhere.
//!
//! [`prepare`] goes before the spawn and [`Tree::adopt`] right after it;
//! [`stop`] ends the family, politely first.

use std::process::{Child, ChildStdin, Command};
use std::time::{Duration, Instant};

#[cfg(unix)]
#[path = "unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "win.rs"]
mod imp;
#[cfg(not(any(unix, windows)))]
mod imp {
    //! No way to reach the descendants: only the child itself is ended.
    use std::process::{Child, Command};

    pub struct Tree;

    pub fn prepare(_: &mut Command) {}

    impl Tree {
        pub fn adopt(_: &Child) -> Tree {
            Tree
        }
        pub fn has_exited(&self, child: &mut Child) -> bool {
            !matches!(child.try_wait(), Ok(None))
        }
        pub fn kill(&self) {}
    }
}

/// How long a module gets to leave by itself after its stdin closes, which
/// is how the protocol asks it to (docs/modules/process-modules.md).
pub const EXIT_GRACE: Duration = Duration::from_secs(1);

/// Set `cmd` up so that the process it starts, and whatever that starts,
/// can be ended together.
pub fn prepare(cmd: &mut Command) {
    imp::prepare(cmd);
}

/// The family of one spawned child.
pub struct Tree(imp::Tree);

impl Tree {
    /// Take charge of `child`, which was spawned from a [`prepare`]d command.
    pub fn adopt(child: &Child) -> Tree {
        Tree(imp::Tree::adopt(child))
    }

    /// Whether the child itself has ended. Unlike `Child::try_wait` this
    /// leaves the exit status uncollected, so the process id stays taken
    /// and [`Tree::kill`] cannot hit a stranger that was given the same id.
    pub fn has_exited(&self, child: &mut Child) -> bool {
        self.0.has_exited(child)
    }

    /// End every process of the family now. Call it before the child's exit
    /// status is collected (`Child::wait`).
    pub fn kill(&self) {
        self.0.kill();
    }
}

/// End `child` and its family at once and collect its exit status.
pub fn kill_now(child: &mut Child, tree: &Tree) -> Option<std::process::ExitStatus> {
    tree.kill();
    let _ = child.kill();
    child.wait().ok()
}

/// Stop a module's process the way the protocol promises: close its stdin
/// (a module leaves on end of input), give it `grace` to go, then end
/// whatever is left of it and of the processes it started. Blocks for up to
/// `grace`.
pub fn stop(mut child: Child, stdin: Option<ChildStdin>, tree: Tree, grace: Duration) {
    drop(stdin);
    let until = Instant::now() + grace;
    while !tree.has_exited(&mut child) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    // Also after a polite exit: helpers the module left behind go with it.
    kill_now(&mut child, &tree);
}

/// [`stop`] without holding the caller up: stdin closes now, the wait and
/// the kill happen on a thread. Modules are stopped from the UI thread
/// (settings applied, a module switched off), which must not wait a second
/// per module.
pub fn stop_in_background(child: Child, stdin: Option<ChildStdin>, tree: Tree, name: &str) {
    drop(stdin);
    // The thread owns the three; if it cannot start they come back and the
    // family is ended on the spot.
    let parts = std::sync::Arc::new(std::sync::Mutex::new(Some((child, tree))));
    let theirs = parts.clone();
    let spawned = std::thread::Builder::new().name(format!("mod-stop-{name}")).spawn(move || {
        if let Some((child, tree)) = theirs.lock().ok().and_then(|mut p| p.take()) {
            stop(child, None, tree, EXIT_GRACE);
        }
    });
    if spawned.is_err() {
        if let Some((mut child, tree)) = parts.lock().ok().and_then(|mut p| p.take()) {
            kill_now(&mut child, &tree);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn alive(pid: u32) -> bool {
        Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    fn wait_gone(pid: u32) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while alive(pid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        !alive(pid)
    }

    /// A shell that starts a helper, tells us the helper's pid and then
    /// does `then`.
    fn family(then: &str) -> (Child, Tree, u32) {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &format!("sleep 300 & echo $!; {then}")]).stdin(Stdio::piped()).stdout(Stdio::piped());
        prepare(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let tree = Tree::adopt(&child);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line).unwrap();
        let helper: u32 = line.trim().parse().unwrap();
        assert!(alive(helper));
        (child, tree, helper)
    }

    #[test]
    fn a_module_that_leaves_on_end_of_input_is_not_killed_but_its_helper_is() {
        // `cat` ends when stdin closes; the shell then exits with 7.
        let (mut child, tree, helper) = family("cat >/dev/null; exit 7");
        let stdin = child.stdin.take();
        let pid = child.id();
        let started = Instant::now();
        stop(child, stdin, tree, Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(3), "it did not sit out the grace: {:?}", started.elapsed());
        assert!(wait_gone(helper), "the helper went with its module");
        assert!(!alive(pid));
    }

    #[test]
    fn a_module_that_ignores_end_of_input_is_killed_with_its_helper_after_the_grace() {
        let (mut child, tree, helper) = family("exec sleep 300");
        let stdin = child.stdin.take();
        let pid = child.id();
        let started = Instant::now();
        stop(child, stdin, tree, Duration::from_millis(150));
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(150) && took < Duration::from_secs(3), "{took:?}");
        assert!(wait_gone(helper) && !alive(pid));
    }

    #[test]
    fn stopping_in_the_background_returns_at_once_and_still_ends_the_family() {
        let (mut child, tree, helper) = family("exec sleep 300");
        let stdin = child.stdin.take();
        let started = Instant::now();
        stop_in_background(child, stdin, tree, "test");
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(wait_gone(helper));
    }

    #[test]
    fn has_exited_does_not_collect_the_status() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 3"]);
        prepare(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let tree = Tree::adopt(&child);
        let until = Instant::now() + Duration::from_secs(5);
        while !tree.has_exited(&mut child) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(tree.has_exited(&mut child) && tree.has_exited(&mut child), "asking twice still works");
        assert_eq!(
            kill_now(&mut child, &tree).and_then(|s| s.code()),
            Some(3),
            "the status was still there to collect"
        );
    }
}
