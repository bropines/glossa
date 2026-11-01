//! Windows Credential Manager: one generic credential per secret, named
//! `Glossa/<account>`, kept for this user on this machine
//! (`CRED_PERSIST_LOCAL_MACHINE`, so it does not roam).

use std::sync::Arc;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

use super::{SecretStore, SERVICE};

struct CredentialManager;

pub fn store() -> Option<Arc<dyn SecretStore>> {
    Some(Arc::new(CredentialManager))
}

fn target(account: &str) -> Vec<u16> {
    format!("{SERVICE}/{account}").encode_utf16().chain(std::iter::once(0)).collect()
}

fn not_found(e: &windows::core::Error) -> bool {
    e.code() == ERROR_NOT_FOUND.to_hresult()
}

impl SecretStore for CredentialManager {
    fn name(&self) -> &'static str {
        "Windows Credential Manager"
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let name = target(account);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name and a valid out pointer; the
        // credential is freed below with CredFree.
        match unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred) } {
            Ok(()) => {}
            Err(e) if not_found(&e) => return Ok(None),
            Err(e) => return Err(format!("CredReadW: {e}")),
        }
        // SAFETY: CredReadW succeeded, so `cred` points at a credential
        // whose blob is `CredentialBlobSize` bytes long.
        let secret = unsafe {
            let c = &*cred;
            let blob = if c.CredentialBlob.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
            };
            let text = String::from_utf8_lossy(blob).into_owned();
            CredFree(cred as *const _);
            text
        };
        Ok(Some(secret))
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let mut name = target(account);
        let mut user: Vec<u16> = SERVICE.encode_utf16().chain(std::iter::once(0)).collect();
        let mut blob = secret.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: every pointer in `cred` points into a buffer alive until
        // the call returns; CredWriteW copies what it keeps.
        unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("CredWriteW: {e}"))
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let name = target(account);
        // SAFETY: a NUL-terminated name.
        match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(format!("CredDeleteW: {e}")),
        }
    }
}
