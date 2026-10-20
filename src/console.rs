//! Janela de console com o log, opcional.
//!
//! O executável de release é um app de janela (sem console). Quem quiser ver o log liga
//! "Mostrar console de log" em Ajustes: abrimos um console só para isso e o fechamos ao desligar.
//! Os utilitários de linha de comando (`--lab`, `--mcp-config`...) usam o console do terminal que
//! os chamou, via [`attach_parent`].

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, ENABLE_VIRTUAL_TERMINAL_PROCESSING, FreeConsole, GetConsoleMode,
    GetConsoleWindow, GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleMode, SetConsoleOutputCP, SetConsoleTitleW,
};
use windows::Win32::UI::WindowsAndMessaging::{DeleteMenu, GetSystemMenu, MF_BYCOMMAND, SC_CLOSE};
use windows::core::w;

/// O console atual foi aberto por nós (e só esse podemos fechar).
static OWNED: AtomicBool = AtomicBool::new(false);

/// Usa o console do terminal que iniciou o processo, se houver (não faz nada ao abrir pelo Explorer).
pub fn attach_parent() {
    // SAFETY: chamada simples da API; falha (sem terminal pai) é esperada e ignorada.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Mostra (`true`) ou esconde (`false`) o console de log. Idempotente.
pub fn set_visible(show: bool) {
    // SAFETY: só chamadas de API do console; todas as falhas são toleradas (o log segue sem console).
    unsafe {
        let has_console = !GetConsoleWindow().is_invalid();
        if show && !has_console {
            if AllocConsole().is_err() {
                return;
            }
            OWNED.store(true, Ordering::SeqCst);
            let _ = SetConsoleOutputCP(65001); // acentos do log em UTF-8
            let _ = SetConsoleTitleW(w!("IsNotAEngine - log"));
            if let Ok(out) = GetStdHandle(STD_OUTPUT_HANDLE) {
                let mut mode = Default::default();
                if GetConsoleMode(out, &mut mode).is_ok() {
                    let _ = SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING); // cores do log
                }
            }
            // Fechar o console pelo X encerraria o motor: tira o botão. Para sumir, desligue em Ajustes.
            let menu = GetSystemMenu(GetConsoleWindow(), false);
            if !menu.is_invalid() {
                let _ = DeleteMenu(menu, SC_CLOSE, MF_BYCOMMAND);
            }
        } else if !show && has_console && OWNED.swap(false, Ordering::SeqCst) {
            let _ = FreeConsole();
        }
    }
}
