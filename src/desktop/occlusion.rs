//! Detecção de oclusão do desktop ("Pausa quando coberto").
//!
//! Permite suspender ou reduzir a renderização quando o desktop não está visível
//! porque outro aplicativo está em tela cheia ou maximizado cobrindo o monitor.

use windows::Win32::Foundation::*;
use windows::core::BOOL;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct OcclusionDetector;

/// `window` cobre `monitor` por inteiro: suas bordas ultrapassam ou igualam as do monitor.
fn covers(window: &RECT, monitor: &RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// Uma janela cobre o monitor se suas bordas o ultrapassam (tela cheia) ou se está maximizada
/// naquele monitor (a área de trabalho maximizada descontaria a barra de tarefas, então não "cobre"
/// o retângulo inteiro, mas esconde o wallpaper do mesmo jeito).
fn window_covers(window: &RECT, zoomed: bool, window_monitor: Option<&RECT>, monitor: &RECT) -> bool {
    covers(window, monitor) || (zoomed && window_monitor == Some(monitor))
}

/// Janelas do shell e as internas do app (superfícies do wallpaper, listener, prévias) nunca contam
/// como cobertura. A janela do Studio conta: maximizada, ela esconde o wallpaper como qualquer outra.
fn is_ignored_class(name: &str) -> bool {
    matches!(name, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
        || (name.starts_with("IsNotAEngine") && !name.contains("StudioWindow"))
}

struct Scan<'a> {
    monitors: &'a [RECT],
    covered: Vec<bool>,
}

unsafe extern "system" fn scan_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` é o `&mut Scan` passado por `occluded_monitors`, vivo durante o EnumWindows.
    let scan = unsafe { &mut *(lparam.0 as *mut Scan) };
    if scan.covered.iter().all(|&c| c) {
        return false.into(); // todos já cobertos: encerra a enumeração
    }
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return true.into();
        }
        // Janelas "escondidas" do Windows (apps UWP suspensos etc.) estão visíveis mas cloaked.
        let mut cloaked = 0u32;
        let _ = windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return true.into();
        }
        // Sobreposições que não escondem nada de fato: ferramentas, sem foco e clique-através.
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0) != 0 {
            return true.into();
        }
        let mut class_buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 && is_ignored_class(&String::from_utf16_lossy(&class_buf[..len as usize])) {
            return true.into();
        }
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return true.into();
        }
        let zoomed = IsZoomed(hwnd).as_bool();
        let window_monitor = if zoomed {
            use windows::Win32::Graphics::Gdi::*;
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            GetMonitorInfoW(hmon, &mut info).as_bool().then_some(info.rcMonitor)
        } else {
            None
        };
        for (i, m) in scan.monitors.iter().enumerate() {
            if !scan.covered[i] && window_covers(&wr, zoomed, window_monitor.as_ref(), m) {
                scan.covered[i] = true;
            }
        }
    }
    true.into()
}

impl OcclusionDetector {
    /// Para cada monitor, `true` se alguma janela visível o cobre por inteiro (tela cheia) ou está
    /// maximizada nele. Olha TODAS as janelas de nível superior, não só a que tem o foco: com
    /// programas maximizados em três telas, as três estão cobertas.
    pub fn occluded_monitors(monitors: &[RECT]) -> Vec<bool> {
        let mut scan = Scan { monitors, covered: vec![false; monitors.len()] };
        if monitors.is_empty() {
            return Vec::new();
        }
        // SAFETY: o callback só roda durante esta chamada, com o ponteiro de `scan` válido.
        let _ = unsafe { EnumWindows(Some(scan_window), LPARAM(&mut scan as *mut _ as isize)) };
        scan.covered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn covers_edge_cases() {
        let monitor = rect(0, 0, 1920, 1080);
        // Tela cheia exata e janela maior que o monitor (bordas fora da tela) cobrem.
        assert!(covers(&rect(0, 0, 1920, 1080), &monitor));
        assert!(covers(&rect(-8, -8, 1928, 1088), &monitor));
        // Um pixel faltando em qualquer borda não cobre.
        assert!(!covers(&rect(1, 0, 1920, 1080), &monitor));
        assert!(!covers(&rect(0, 1, 1920, 1080), &monitor));
        assert!(!covers(&rect(0, 0, 1919, 1080), &monitor));
        assert!(!covers(&rect(0, 0, 1920, 1079), &monitor));
    }

    #[test]
    fn secondary_monitor_with_negative_origin() {
        let left_monitor = rect(-1920, 0, 0, 1080);
        // Janela maximizada no monitor principal não cobre o da esquerda.
        assert!(!covers(&rect(0, 0, 1920, 1080), &left_monitor));
        assert!(covers(&rect(-1920, 0, 0, 1080), &left_monitor));
        // Janela que ocupa os dois monitores cobre ambos.
        let both = rect(-1920, 0, 1920, 1080);
        assert!(covers(&both, &left_monitor) && covers(&both, &rect(0, 0, 1920, 1080)));
    }

    #[test]
    fn maximized_window_covers_its_monitor_even_without_the_taskbar_area() {
        let monitor = rect(0, 0, 1920, 1080);
        // Maximizada com barra de tarefas: a janela termina em 1040, não cobre o retângulo inteiro...
        let work_area = rect(-8, -8, 1928, 1048);
        assert!(!covers(&work_area, &monitor));
        // ...mas conta quando está maximizada neste monitor, e não conta em outro.
        assert!(window_covers(&work_area, true, Some(&monitor), &monitor));
        assert!(!window_covers(&work_area, true, Some(&rect(1920, 0, 3840, 1080)), &monitor));
        assert!(!window_covers(&work_area, false, None, &monitor), "janela comum parcial não cobre");
    }

    #[test]
    fn shell_and_own_windows_never_count() {
        for c in ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "IsNotAEngine_Surface", "IsNotAEngine_WebChild", "IsNotAEngine_SystemListener"] {
            assert!(is_ignored_class(c), "{c}");
        }
        assert!(!is_ignored_class("IsNotAEngine_StudioWindowV2"), "o Studio maximizado esconde o wallpaper");
        assert!(!is_ignored_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn real_scan_returns_one_flag_per_monitor() {
        let monitors = [rect(0, 0, 1920, 1080), rect(1920, 0, 3840, 1080)];
        assert_eq!(OcclusionDetector::occluded_monitors(&monitors).len(), 2);
        assert!(OcclusionDetector::occluded_monitors(&[]).is_empty());
    }
}
