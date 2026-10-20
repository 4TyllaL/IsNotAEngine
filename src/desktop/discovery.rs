//! Descoberta da camada de wallpaper do Explorer.
//!
//! Isola o comportamento NÃO documentado de criação e hierarquia de janelas do Explorer.

use tracing::{debug, info, warn};
use windows::{
    Win32::Foundation::*, Win32::Graphics::Gdi::*, Win32::System::StationsAndDesktops::*,
    Win32::UI::WindowsAndMessaging::*, core::*,
};

const WM_SPAWN_WORKERW: u32 = 0x052C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopLayout {
    /// Win11 24H2+: WorkerW é filho do Progman, que possui WS_EX_NOREDIRECTIONBITMAP.
    Modern,
    /// Win10 / Win11 ≤ 23H2: WorkerW top-level adjacente à janela de ícones.
    Legacy,
    /// Fallback: usa o próprio Progman.
    Fallback,
}

#[derive(Debug, Clone)]
pub struct WallpaperLayer {
    pub layout: DesktopLayout,
    pub parent: HWND,
}

pub fn ensure_interactive_desktop() {
    unsafe {
        let ws = OpenWindowStationW(w!("WinSta0"), false, 0x10000000)
            .or_else(|_| OpenWindowStationW(w!("WinSta0"), false, 0x00020000 | 0x037F));
        if let Ok(ws) = ws {
            let _ = SetProcessWindowStation(ws);
        }
        let d = OpenDesktopW(w!("Default"), DESKTOP_CONTROL_FLAGS(0), false, 0x10000000)
            .or_else(|_| OpenDesktopW(w!("Default"), DESKTOP_CONTROL_FLAGS(0), false, 0x00020000 | 0x01FF));
        if let Ok(d) = d {
            let res = SetThreadDesktop(d);
            let err = GetLastError();
            info!(success = res.is_ok(), ?err, "SetThreadDesktop para 'Default' executado");
        } else if let Err(e) = d {
            warn!(error = %e, "Falha ao abrir desktop 'Default'");
        }
    }
}

pub fn relaunch_on_default_desktop() -> bool {
    if cfg!(test) || std::env::args().any(|a| a == "--no-relaunch" || a.contains("test")) {
        return false;
    }

    unsafe {
        use windows::Win32::System::Threading::*;

        let dt = match GetThreadDesktop(GetCurrentThreadId()) {
            Ok(dt) => dt,
            Err(_) => return false,
        };

        let mut buf = [0u16; 256];
        let mut needed = 0;
        let _ = GetUserObjectInformationW(
            HANDLE(dt.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as _),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        );
        let name = String::from_utf16_lossy(&buf[..(needed as usize / 2).saturating_sub(1)]);
        if name.eq_ignore_ascii_case("default") || name.is_empty() {
            return false;
        }

        info!(current_desktop = %name, "Processo detectado fora do desktop 'Default'. Reiniciando no desktop interativo...");

        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cmd = if args.is_empty() {
            format!("\"{}\"", exe.display())
        } else {
            format!("\"{}\" {}", exe.display(), args.join(" "))
        };

        let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        let mut desktop_w: Vec<u16> = "WinSta0\\Default\0".encode_utf16().collect();

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop_w.as_mut_ptr()),
            ..Default::default()
        };

        let mut pi = PROCESS_INFORMATION::default();

        let ok = CreateProcessW(
            None,
            Some(PWSTR(cmd_w.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &si,
            &mut pi,
        );

        if ok.is_ok() {
            info!(child_pid = pi.dwProcessId, "Processo interativo iniciado. Aguardando execução...");
            let _ = WaitForSingleObject(pi.hProcess, INFINITE);
            let mut exit_code = 0u32;
            let _ = GetExitCodeProcess(pi.hProcess, &mut exit_code);
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
            std::process::exit(exit_code as i32);
        } else {
            warn!(error = ?GetLastError(), "Falha ao reiniciar no desktop 'WinSta0\\Default'. Continuando no desktop atual.");
            false
        }
    }
}

pub fn find_wallpaper_layer() -> Result<WallpaperLayer> {
    let mut progman = find_progman();
    if progman.is_none() {
        ensure_interactive_desktop();
        progman = find_progman();
    }
    let progman = progman.ok_or_else(|| Error::new(E_FAIL, "Progman não encontrado"))?;
    debug!(progman = ?progman, "Progman localizado");

    spawn_workerw(progman);

    // 1. Tenta layout moderno (Win11 24H2+)
    if let Some(w) = find_child(Some(progman), None, w!("WorkerW")) {
        info!(workerw = ?w, "Camada de wallpaper detectada: Layout Modern (filho do Progman)");
        return Ok(WallpaperLayer {
            layout: DesktopLayout::Modern,
            parent: w,
        });
    }

    // 2. Tenta layout legado (Win10 / Win11 antigo)
    for top in top_level_windows() {
        if find_child(Some(top), None, w!("SHELLDLL_DefView")).is_some()
            && let Some(w) = find_child(None, Some(top), w!("WorkerW"))
        {
            info!(workerw = ?w, "Camada de wallpaper detectada: Layout Legacy (WorkerW irmão)");
            return Ok(WallpaperLayer {
                layout: DesktopLayout::Legacy,
                parent: w,
            });
        }
    }

    warn!(progman = ?progman, "Nenhum WorkerW encontrado. Usando Progman como fallback.");
    Ok(WallpaperLayer {
        layout: DesktopLayout::Fallback,
        parent: progman,
    })
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

pub fn repaint_desktop(hwnd: HWND) {
    let _ = unsafe {
        RedrawWindow(
            Some(hwnd),
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        )
    };
}

fn find_progman() -> Option<HWND> {
    let shell = unsafe { GetShellWindow() };
    if !shell.is_invalid() {
        return Some(shell);
    }
    unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }
        .ok()
        .filter(|h| !h.is_invalid())
}

fn spawn_workerw(progman: HWND) {
    for (wp, lp) in [(0xD, 0x1), (0, 0)] {
        unsafe {
            SendMessageTimeoutW(
                progman,
                WM_SPAWN_WORKERW,
                WPARAM(wp),
                LPARAM(lp),
                SMTO_NORMAL,
                1000,
                None,
            );
        }
    }
}

pub fn find_child(parent: Option<HWND>, after: Option<HWND>, class: PCWSTR) -> Option<HWND> {
    unsafe { FindWindowExW(parent, after, class, PCWSTR::null()) }
        .ok()
        .filter(|h| !h.is_invalid())
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe { (*(lparam.0 as *mut Vec<HWND>)).push(hwnd) };
        true.into()
    }
    let mut v: Vec<HWND> = Vec::new();
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut v as *mut _ as isize)) };
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progman_discovery() {
        let res = find_wallpaper_layer();
        assert!(res.is_ok(), "Deve encontrar a camada de wallpaper: {:?}", res.err());
        let layer = res.unwrap();
        println!("Camada encontrada: layout={:?}, parent={:?}", layer.layout, layer.parent);
    }
}
