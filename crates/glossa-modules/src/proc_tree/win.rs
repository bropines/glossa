//! Windows: the module's process is put into a job object that kills its
//! processes when the job is terminated or its last handle closes, so the
//! family also goes when Glossa itself dies.

use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct Tree {
    /// `None` when the job could not be made or the process not put into
    /// it; then only the child itself is ended (by `Child::kill`).
    job: Option<HANDLE>,
}

// SAFETY: a job handle is a kernel object reference; the calls made on it
// (terminate, close) may come from any thread.
unsafe impl Send for Tree {}
unsafe impl Sync for Tree {}

/// Nothing to set before the spawn: the job is attached right after it.
pub fn prepare(_: &mut Command) {}

impl Tree {
    /// Processes the child starts after this belong to the job too. One it
    /// managed to start in the instant before is missed; the standard
    /// library offers no way to spawn suspended.
    pub fn adopt(child: &Child) -> Tree {
        Tree { job: make_job(child) }
    }

    pub fn has_exited(&self, child: &mut Child) -> bool {
        // The process is held by handle here, so collecting its status
        // early frees nothing that `kill` relies on.
        !matches!(child.try_wait(), Ok(None))
    }

    pub fn kill(&self) {
        if let Some(job) = self.job {
            // SAFETY: `job` is a job handle we created and have not closed.
            unsafe {
                let _ = TerminateJobObject(job, 1);
            }
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            // SAFETY: closing our own handle, once. With
            // KILL_ON_JOB_CLOSE this also ends whatever is still in the job.
            unsafe {
                let _ = CloseHandle(job);
            }
        }
    }
}

fn make_job(child: &Child) -> Option<HANDLE> {
    // SAFETY: plain Win32 calls. The job handle is ours until closed; the
    // process handle is borrowed from `child`, which outlives the call; the
    // limits structure is zero-initialised plain data of the size we pass.
    unsafe {
        let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        let assigned = set.and_then(|()| AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())));
        if let Err(e) = assigned {
            tracing::debug!("the module's process could not be put into a job: {e}");
            let _ = CloseHandle(job);
            return None;
        }
        Some(job)
    }
}
