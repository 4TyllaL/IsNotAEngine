//! Prévia de wallpapers web para o MCP: abre uma janela fora da tela, carrega a página com o
//! mesmo `WebWallpaper` do desktop (mesma política de segurança, sem rede) e captura quadros.

use std::time::{Duration, Instant};

use windows::{
    Win32::Foundation::*,
    Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::UI::WindowsAndMessaging::*,
    core::*,
};

use crate::wallpaper::web::{WebSource, WebWallpaper};

const CLASS: PCWSTR = w!("IsNotAEngine_McpPreview");
/// Soma máxima das esperas de uma chamada.
const MAX_TOTAL_MS: u32 = 60_000;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut msg = MSG::default();
    while Instant::now() < end {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn create_window(w: u32, h: u32) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        let _ = RegisterClassExW(&class); // repetir o registro só devolve erro
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS,
            w!("preview"),
            WS_POPUP,
            -32000,
            -32000,
            w as i32,
            h as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        Ok(hwnd)
    }
}

/// Captura uma página HTML em `w`x`h`, uma vez para cada instante de `waits_ms` (ms após o carregamento).
pub fn preview(html: &str, w: u32, h: u32, waits_ms: &[u32]) -> std::result::Result<Vec<Vec<u8>>, String> {
    if html.len() > super::psl::MAX_SOURCE_BYTES {
        return Err(format!("the page is larger than {} KiB", super::psl::MAX_SOURCE_BYTES / 1024));
    }
    if waits_ms.iter().map(|&n| u64::from(n)).sum::<u64>() > u64::from(MAX_TOTAL_MS) {
        return Err(format!("the capture times add up to more than {} s", MAX_TOTAL_MS / 1000));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join("isnotaengine_mcp").join(format!("web_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("index.html"), html).map_err(|e| e.to_string())?;

    let result = capture(&dir, w, h, waits_ms);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn capture(dir: &std::path::Path, w: u32, h: u32, waits_ms: &[u32]) -> std::result::Result<Vec<Vec<u8>>, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let hwnd = create_window(w, h).map_err(|e| format!("could not create the preview window: {e}"))?;
    let outcome = (|| {
        let page = WebWallpaper::new(hwnd, WebSource::Dir { root: dir.to_path_buf(), entry: "index.html".into() })
            .map_err(|e| format!("could not start WebView2 (is the WebView2 runtime installed?): {e}"))?;
        let mut shots = Vec::new();
        let mut elapsed = 0u32;
        for &at in waits_ms {
            pump_for(u64::from(at.saturating_sub(elapsed)));
            elapsed = elapsed.max(at);
            let png = page.capture_png().ok_or("the screenshot failed (the page may not have finished loading)")?;
            shots.push(png);
        }
        Ok(shots)
    })();
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    outcome
}
