//! Enumeração e posicionamento de múltiplos monitores.

use tracing::debug;
use windows::{
    Win32::Foundation::*, Win32::Graphics::Gdi::*, Win32::UI::WindowsAndMessaging::*, core::*,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Índice da enumeração atual: muda com a ordem em que o Windows lista as telas.
    pub id: usize,
    /// Identidade persistente (ver [`stable_keys`]): é isto, e não `id`, que vai para o disco.
    pub key: String,
    pub screen_rect: RECT,
    pub client_x: i32,
    pub client_y: i32,
    pub width: u32,
    pub height: u32,
}

pub struct MonitorManager;

impl MonitorManager {
    /// Enumera os monitores conectados e mapeia para coordenadas relativas à janela pai.
    pub fn enumerate_relative_to(parent_hwnd: HWND) -> Result<Vec<MonitorInfo>> {
        let parent_rect = window_rect(parent_hwnd)?;
        let raw = enumerate_monitors();
        let keys = stable_keys(&raw.iter().map(|r| (r.hardware_id.clone(), r.rect)).collect::<Vec<_>>());

        let mut list = Vec::with_capacity(raw.len());
        for (i, (raw, key)) in raw.into_iter().zip(keys).enumerate() {
            let m = raw.rect;
            let width = (m.right - m.left) as u32;
            let height = (m.bottom - m.top) as u32;
            let client_x = m.left - parent_rect.left;
            let client_y = m.top - parent_rect.top;

            debug!(
                id = i,
                width = width,
                height = height,
                screen_x = m.left,
                screen_y = m.top,
                client_x = client_x,
                client_y = client_y,
                "Monitor detectado"
            );

            list.push(MonitorInfo {
                id: i,
                key,
                screen_rect: m,
                client_x,
                client_y,
                width,
                height,
            });
        }

        Ok(list)
    }
}

pub fn window_rect(hwnd: HWND) -> Result<RECT> {
    let mut rc = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rc)? };
    Ok(rc)
}

struct RawMonitor {
    rect: RECT,
    /// Caminho de interface do monitor (inclui modelo e a porta/instância); `None` se o Windows não informou.
    hardware_id: Option<String>,
}

/// Chaves persistentes para uma enumeração. Cada item é `(identidade de hardware, retângulo)`.
///
/// - Com identidade: a própria identidade em minúsculas, então trocar a ordem de enumeração não
///   troca a chave.
/// - Telas idênticas que reportam a mesma identidade: ganham sufixo `#2`, `#3`... na ordem de
///   posição (esquerda→direita, cima→baixo), para nunca colidir.
/// - Sem identidade confiável: `pos:<x>,<y>,<largura>x<altura>` (muda se a tela for movida).
pub fn stable_keys(monitors: &[(Option<String>, RECT)]) -> Vec<String> {
    let base: Vec<String> = monitors
        .iter()
        .map(|(id, r)| match id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => format!("hw:{}", id.to_ascii_lowercase()),
            None => format!("pos:{},{},{}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top),
        })
        .collect();
    let mut order: Vec<usize> = (0..monitors.len()).collect();
    order.sort_by_key(|&i| (monitors[i].1.left, monitors[i].1.top, i));
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut keys = base.clone();
    for i in order {
        let n = seen.entry(base[i].as_str()).or_insert(0);
        *n += 1;
        if *n > 1 {
            keys[i] = format!("{}#{}", base[i], n);
        }
    }
    keys
}

fn hardware_id(monitor: HMONITOR) -> Option<String> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) }.as_bool().then_some(())?;
    let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    // EDD_GET_DEVICE_INTERFACE_NAME (0x1): DeviceID vira `\?\`\?DISPLAY#<modelo>#<instância>#{guid}`.
    unsafe { EnumDisplayDevicesW(PCWSTR(info.szDevice.as_ptr()), 0, &mut dd, 1) }.as_bool().then_some(())?;
    let len = dd.DeviceID.iter().position(|&c| c == 0).unwrap_or(dd.DeviceID.len());
    let id = String::from_utf16_lossy(&dd.DeviceID[..len]);
    (!id.is_empty()).then_some(id)
}

fn enumerate_monitors() -> Vec<RawMonitor> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let raw = RawMonitor { rect: unsafe { *rect }, hardware_id: hardware_id(monitor) };
        unsafe { (*(lparam.0 as *mut Vec<RawMonitor>)).push(raw) };
        true.into()
    }
    let mut v: Vec<RawMonitor> = Vec::new();
    let _ = unsafe {
        EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut v as *mut _ as isize))
    };
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc(l: i32, t: i32, w: i32, h: i32) -> RECT {
        RECT { left: l, top: t, right: l + w, bottom: t + h }
    }
    fn hw(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn key_does_not_depend_on_enumeration_order() {
        let a = (hw(r"\?\`\?DISPLAY#DEL4123#5&aa#{x}"), rc(0, 0, 1920, 1080));
        let b = (hw(r"\?\`\?DISPLAY#GSM5BBF#5&bb#{x}"), rc(1920, 0, 2560, 1440));
        let first = stable_keys(&[a.clone(), b.clone()]);
        let swapped = stable_keys(&[b, a]);
        assert_eq!(first[0], swapped[1]);
        assert_eq!(first[1], swapped[0]);
    }

    #[test]
    fn identical_screens_do_not_collide() {
        let same = r"\?\`\?DISPLAY#DEL4123#5&aa#{x}";
        let keys = stable_keys(&[(hw(same), rc(1920, 0, 1920, 1080)), (hw(same), rc(0, 0, 1920, 1080)), (hw(same), rc(3840, 0, 1920, 1080))]);
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 3);
        // A tela mais à esquerda (índice 1) mantém a chave limpa; as outras ganham sufixo por posição.
        assert!(keys[0].ends_with("#2") && keys[2].ends_with("#3"), "{keys:?}");
    }

    #[test]
    fn missing_identity_falls_back_to_position_and_size() {
        let keys = stable_keys(&[(None, rc(0, 0, 1920, 1080)), (hw("  "), rc(1920, 0, 1280, 720))]);
        assert_eq!(keys, vec!["pos:0,0,1920x1080".to_string(), "pos:1920,0,1280x720".to_string()]);
    }

    #[test]
    fn disconnecting_one_screen_keeps_the_others_keys() {
        let a = (hw("A"), rc(0, 0, 100, 100));
        let b = (hw("B"), rc(100, 0, 100, 100));
        let with_both = stable_keys(&[a.clone(), b.clone()]);
        let only_b = stable_keys(&[b]);
        assert_eq!(with_both[1], only_b[0]);
    }

    #[test]
    fn real_enumeration_gives_unique_nonempty_keys() {
        let raw = enumerate_monitors();
        let keys = stable_keys(&raw.iter().map(|r| (r.hardware_id.clone(), r.rect)).collect::<Vec<_>>());
        assert_eq!(keys.len(), raw.len());
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        assert!(keys.iter().all(|k| !k.is_empty()));
    }
}
