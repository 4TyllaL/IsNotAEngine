//! Chaves de API no Gerenciador de Credenciais do Windows (protegidas pela conta do usuário).
//! Nunca vão para o `config.toml`, para o log nem para a página do Studio.

use windows::Win32::Security::Credentials::{
    CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn target(name: &str) -> Vec<u16> {
    wide(&format!("IsNotAEngine/ai/{name}"))
}

pub fn save(name: &str, key: &str) -> Result<(), String> {
    let target = target(name);
    let mut user = wide("api-key");
    let blob = key.as_bytes();
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_ptr() as *mut u16),
        UserName: PWSTR(user.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("could not store the key in Windows Credential Manager: {e}"))
}

pub fn load(name: &str) -> Option<String> {
    let target = target(name);
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut found).ok()?;
        let cred = &*found;
        let bytes = std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize).to_vec();
        CredFree(found as *const _);
        String::from_utf8(bytes).ok().filter(|k| !k.trim().is_empty())
    }
}

pub fn delete(name: &str) {
    let target = target(name);
    let _ = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip_through_credential_manager_and_can_be_deleted() {
        let name = format!("test-{}", std::process::id());
        assert_eq!(load(&name), None);
        save(&name, "sk-ant-ÚNICA-123").expect("grava");
        assert_eq!(load(&name).as_deref(), Some("sk-ant-ÚNICA-123"), "UTF-8 preservado");
        save(&name, "outra").expect("sobrescreve");
        assert_eq!(load(&name).as_deref(), Some("outra"));
        delete(&name);
        assert_eq!(load(&name), None);
        delete(&name); // apagar o que não existe não é erro
    }
}
