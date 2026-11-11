//! Unix: the module's process leads a process group of its own, and the
//! group is what gets killed.

use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

pub struct Tree {
    /// The child's pid, which is also its group's id. Zero or negative
    /// would address our own group or every process, so [`Tree::kill`]
    /// refuses anything but a real pid.
    pgid: libc::pid_t,
}

/// Start the child as the leader of a new process group. Its descendants
/// stay in the group unless they leave it on purpose (a daemon that calls
/// `setsid`), so one signal reaches them all, and a Ctrl+C in the terminal
/// `glossa-cli` runs in no longer reaches the module before Glossa has
/// closed its stdin.
pub fn prepare(cmd: &mut Command) {
    cmd.process_group(0);
}

impl Tree {
    pub fn adopt(child: &Child) -> Tree {
        Tree { pgid: libc::pid_t::try_from(child.id()).unwrap_or(0) }
    }

    pub fn has_exited(&self, _child: &mut Child) -> bool {
        if self.pgid <= 1 {
            return true;
        }
        // SAFETY: `waitid` writes into the zeroed `siginfo_t` we hand it and
        // touches nothing else. WNOWAIT leaves the child a zombie, so the
        // pid (and the group id) cannot be given to another process until
        // `Child::wait` collects it.
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let r = libc::waitid(
                libc::P_PID,
                self.pgid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            // An error means there is no such child any more (it was
            // collected already). Otherwise `si_pid` stays zero while the
            // child runs.
            r != 0 || info.si_pid() != 0
        }
    }

    pub fn kill(&self) {
        if self.pgid <= 1 {
            return;
        }
        // SAFETY: a plain system call with integer arguments. The group id
        // is the pid of a child whose status has not been collected yet
        // (see `has_exited`), so it still names our module's group. A group
        // that is already empty answers ESRCH, which is fine.
        unsafe {
            libc::killpg(self.pgid, libc::SIGKILL);
        }
    }
}
