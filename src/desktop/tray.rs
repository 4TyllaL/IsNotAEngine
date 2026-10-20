//! Gerenciador de Ícone na Bandeja do Sistema (System Tray / Área de Notificação).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tracing::{debug, info, warn};
use windows::{
    core::*,
    Win32::Foundation::*,
    Win32::UI::Shell::*,
    Win32::UI::WindowsAndMessaging::*,
};

pub const WM_TRAY_ICON: u32 = WM_USER + 101;

pub const IDM_OPEN_STUDIO: usize = 2001;
pub const IDM_TOGGLE_PAUSE: usize = 2002;
pub const IDM_TOGGLE_PERFORMANCE: usize = 2006;
pub const IDM_NEXT_WALLPAPER: usize = 2005;
pub const IDM_START_SCREENSAVER: usize = 2007;
pub const IDM_EXIT: usize = 2099;
/// Itens do submenu "Aplicar em todos": `IDM_WALLPAPER_BASE + posição` em [`MENU_WALLPAPERS`].
pub const IDM_WALLPAPER_BASE: usize = 3000;
const MAX_IMPORTED_IN_MENU: usize = 15;

/// O que o motor informa à bandeja para o menu: o que está no ar agora.
#[derive(Debug, Clone, Default)]
pub struct TrayInfo {
    /// Nome para mostrar ("Matrix", "Vários wallpapers"); vazio = ainda desconhecido.
    pub current_title: String,
    /// Wallpaper (id ou caminho) se todos os monitores usam o mesmo; serve para marcar o item.
    pub current_path: Option<String>,
    /// Modo desempenho em "sempre ligado" (item marcado do menu).
    pub performance_always: bool,
    /// Nomes escolhidos pelo usuário (caminho -> título) para wallpapers criados/importados.
    pub titles: std::collections::HashMap<String, String>,
}

/// Ids/caminhos do último menu aberto, na ordem dos itens (o menu é modal, na mesma thread).
static MENU_WALLPAPERS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Wallpaper (id ou caminho) que o comando de menu `cmd` representa, se for um item de wallpaper.
pub fn menu_wallpaper(cmd: usize) -> Option<String> {
    let idx = cmd.checked_sub(IDM_WALLPAPER_BASE)?;
    MENU_WALLPAPERS.lock().ok()?.get(idx).cloned()
}

/// Nome de exibição de um wallpaper: título do catálogo ou nome do arquivo.
pub fn wallpaper_title(path_or_id: &str) -> String {
    if path_or_id.starts_with(crate::wallpaper::factory::PREFIX) {
        return crate::wallpaper::factory::title(path_or_id).unwrap_or_else(|| path_or_id.trim_start_matches(crate::wallpaper::factory::PREFIX).to_string());
    }
    match crate::wallpaper::catalog::resolve(path_or_id) {
        Some(e) => e.title.to_string(),
        None => std::path::Path::new(path_or_id)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_or_id.to_string()),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ícone do aplicativo (recurso 1 em `app.rc`) no tamanho pedido, com fallback para o genérico.
/// O id do recurso é passado como MAKEINTRESOURCE (o próprio número no lugar do ponteiro).
/// Não trocar por `ptr::dangling()` (que é o endereço 2, outro recurso): foi esse o bug do ícone vazio.
pub fn app_icon(size: i32) -> HICON {
    unsafe {
        let inst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        LoadImageW(Some(inst.into()), PCWSTR(std::ptr::without_provenance::<u16>(1)), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

pub struct SystemTray {
    hwnd: HWND,
    uid: u32,
    _is_paused: Arc<AtomicBool>,
}

impl SystemTray {
    pub fn new(hwnd: HWND, is_paused: Arc<AtomicBool>) -> Result<Self> {
        let tray = Self { hwnd, uid: 1, _is_paused: is_paused };
        tray.add();
        Ok(tray)
    }

    /// Registra o ícone na bandeja. Também serve para recriá-lo: o Explorer perde todos os ícones quando
    /// reinicia (`TaskbarCreated`) e o app precisa pedir de novo.
    pub fn add(&self) {
        unsafe {
            let mut tip = [0u16; 128];
            for (i, c) in "IsNotAEngine — Animated Wallpaper Engine".encode_utf16().enumerate().take(127) {
                tip[i] = c;
            }
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: self.uid,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: WM_TRAY_ICON,
                hIcon: app_icon(GetSystemMetrics(SM_CXSMICON)),
                szTip: tip,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                info!("Ícone na bandeja do sistema registrado");
            } else {
                warn!("O Windows recusou registrar o ícone na bandeja");
            }
        }
    }
}

/// Menus Win32 no tema escuro (APIs não documentadas do uxtheme, estáveis desde o Windows 10 1903):
/// `SetPreferredAppMode(ForceDark)` (ordinal 135) e `FlushMenuThemes` (ordinal 136).
fn enable_dark_menus() {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(2);
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    });
}

/// Exibe o menu de contexto na posição atual do mouse para a janela alvo.
pub fn show_tray_menu(hwnd: HWND, is_paused: bool, info: &TrayInfo) {
    enable_dark_menus();
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        let Ok(menu) = CreatePopupMenu() else { return };

        // Cabeçalho informativo (desabilitado): nome e estado do motor.
        let header = if is_paused { w!("IsNotAEngine  ·  pausado") } else { w!("IsNotAEngine  ·  rodando") };
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, header);
        if !info.current_title.is_empty() {
            let line = wide(&format!("Agora: {}", info.current_title.replace('&', "&&")));
            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(line.as_ptr()));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN_STUDIO, w!("Abrir o Studio"));
        let _ = SetMenuDefaultItem(menu, IDM_OPEN_STUDIO as u32, 0);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let pause_text = if is_paused { w!("Retomar animação") } else { w!("Pausar animação") };
        let _ = AppendMenuW(menu, MF_STRING, IDM_TOGGLE_PAUSE, pause_text);
        let _ = AppendMenuW(menu, MF_STRING, IDM_NEXT_WALLPAPER, w!("Próximo wallpaper"));
        let _ = AppendMenuW(menu, MF_STRING, IDM_START_SCREENSAVER, w!("Proteção de tela viva"));
        let perf_flags = if info.performance_always { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, perf_flags, IDM_TOGGLE_PERFORMANCE, w!("Modo desempenho (sempre)"));

        if let Ok(sub_menu) = CreatePopupMenu() {
            let mut ids: Vec<String> = Vec::new();
            let mut add = |m: HMENU, path: &str, title: &str| {
                let label = wide(&title.replace('&', "&&"));
                let checked = info.current_path.as_deref() == Some(path);
                let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
                let _ = AppendMenuW(m, flags, IDM_WALLPAPER_BASE + ids.len(), PCWSTR(label.as_ptr()));
                ids.push(path.to_string());
            };
            // Embutidos agrupados por categoria (um bloco por categoria).
            let mut last_cat = "";
            for e in crate::wallpaper::catalog::CATALOG {
                if !last_cat.is_empty() && e.category != last_cat {
                    let _ = AppendMenuW(sub_menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                last_cat = e.category;
                add(sub_menu, e.id, e.title);
            }
            // Vídeos de fábrica (pasta `videos` do app).
            let videos = crate::wallpaper::factory::list();
            if !videos.is_empty() {
                let _ = AppendMenuW(sub_menu, MF_SEPARATOR, 0, PCWSTR::null());
                for v in &videos {
                    add(sub_menu, &v.id, &v.title);
                }
            }
            // Os do usuário (importados/criados), logo depois.
            let dir = crate::config::EngineConfig::config_path().parent().map(|p| p.join("wallpapers"));
            let mine = dir.map(|d| crate::import::list_imported(&d)).unwrap_or_default();
            if !mine.is_empty() {
                let _ = AppendMenuW(sub_menu, MF_SEPARATOR, 0, PCWSTR::null());
                for it in mine.iter().take(MAX_IMPORTED_IN_MENU) {
                    add(sub_menu, &it.path, info.titles.get(&it.path).unwrap_or(&it.title));
                }
                if mine.len() > MAX_IMPORTED_IN_MENU {
                    let _ = AppendMenuW(sub_menu, MF_STRING | MF_GRAYED, 0, w!("… mais no Studio"));
                }
            }
            if let Ok(mut slot) = MENU_WALLPAPERS.lock() {
                *slot = ids;
            }
            let _ = AppendMenuW(menu, MF_POPUP | MF_STRING, sub_menu.0 as usize, w!("Aplicar em todos"));
        }

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("Sair"));

        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        unsafe {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: self.uid,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            debug!("Ícone da bandeja removido");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_the_catalog_or_the_file_name() {
        assert_eq!(wallpaper_title("matrix"), "Matrix");
        assert_eq!(wallpaper_title(r"C:\x\meu_wallpaper.hlsl"), "meu_wallpaper");
    }

    #[test]
    fn menu_commands_map_to_the_items_of_the_last_menu() {
        *MENU_WALLPAPERS.lock().unwrap() = vec!["matrix".into(), "a.hlsl".into()];
        assert_eq!(menu_wallpaper(IDM_WALLPAPER_BASE + 1).as_deref(), Some("a.hlsl"));
        assert_eq!(menu_wallpaper(IDM_WALLPAPER_BASE + 2), None);
        assert_eq!(menu_wallpaper(IDM_EXIT), None);
    }
}
