//! Gerenciamento de inicialização automática com o Windows.
//! Instalado normalmente: chave HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
//! Empacotado (MSIX / Microsoft Store): a chave Run seria virtualizada e não iniciaria nada, então vale a
//! tarefa de inicialização (`StartupTask`) declarada no manifesto do pacote (`packaging/msix`).
#![allow(unsafe_op_in_unsafe_fn)]

use tracing::info;
use windows::{
    core::*,
    Win32::System::Registry::*,
};

const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const APP_VALUE_NAME: PCWSTR = w!("IsNotAEngine");

/// Id da `desktop:StartupTask` em `packaging/msix/AppxManifest.xml`.
const STARTUP_TASK_ID: &str = "IsNotAEngineStartup";

/// O processo roda dentro de um pacote MSIX?
fn is_packaged() -> bool {
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    let mut len = 0u32;
    // APPMODEL_ERROR_NO_PACKAGE (15700) quando não há pacote; ERROR_INSUFFICIENT_BUFFER quando há.
    unsafe { GetCurrentPackageFullName(&mut len, None).0 != 15700 }
}

fn startup_task() -> Result<windows::ApplicationModel::StartupTask> {
    windows::ApplicationModel::StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK_ID))?.join()
}

pub fn is_autostart_enabled() -> bool {
    use windows::ApplicationModel::StartupTaskState as S;
    if is_packaged() {
        return startup_task().and_then(|t| t.State()).is_ok_and(|s| s == S::Enabled || s == S::EnabledByPolicy);
    }
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, Some(0), KEY_READ, &mut hkey).is_err() {
            return false;
        }

        let mut data_type = REG_VALUE_TYPE::default();
        let mut data_len = 0u32;
        let res = RegQueryValueExW(
            hkey,
            APP_VALUE_NAME,
            None,
            Some(&mut data_type),
            None,
            Some(&mut data_len),
        );
        let _ = RegCloseKey(hkey);
        res.is_ok() && data_len > 0
    }
}

pub fn set_autostart_enabled(enabled: bool) -> Result<()> {
    use windows::ApplicationModel::StartupTaskState as S;
    if is_packaged() {
        let task = startup_task()?;
        if enabled {
            // O usuário pode ter desligado nas Configurações do Windows: aí o pedido não tem efeito.
            let state = task.RequestEnableAsync()?.join()?;
            if state != S::Enabled && state != S::EnabledByPolicy {
                return Err(Error::new(windows::Win32::Foundation::E_ACCESSDENIED, "A inicialização está desativada nas Configurações do Windows (Aplicativos > Inicialização)"));
            }
        } else {
            task.Disable()?;
        }
        info!(enabled, "Inicialização automática (pacote) atualizada");
        return Ok(());
    }
    unsafe {
        let mut hkey = HKEY::default();
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            RUN_SUBKEY,
            Some(0),
            KEY_WRITE,
            &mut hkey,
        ).ok()?;

        if enabled {
            let exe = std::env::current_exe()
                .map_err(|e| Error::new(windows::Win32::Foundation::E_FAIL, format!("Falha ao obter caminho do exe: {}", e)))?;
            let cmd = format!("\"{}\"", exe.display());
            let cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(
                cmd_w.as_ptr() as *const u8,
                cmd_w.len() * std::mem::size_of::<u16>(),
            );
            RegSetValueExW(
                hkey,
                APP_VALUE_NAME,
                Some(0),
                REG_SZ,
                Some(bytes),
            ).ok()?;
            info!(path = %cmd, "Inicialização automática com o Windows ativada com sucesso");
        } else {
            let _ = RegDeleteValueW(hkey, APP_VALUE_NAME);
            info!("Inicialização automática com o Windows desativada com sucesso");
        }

        let _ = RegCloseKey(hkey);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autostart_query() {
        // Apenas verifica se a leitura do registro funciona sem pânico
        let _enabled = is_autostart_enabled();
    }
}
