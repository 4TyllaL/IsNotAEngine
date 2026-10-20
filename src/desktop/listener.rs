use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{info, warn};
use windows::{
    Win32::Foundation::*, Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::UI::WindowsAndMessaging::*, core::*,
};

use crate::desktop::tray::{
    IDM_EXIT, IDM_NEXT_WALLPAPER, IDM_OPEN_STUDIO, IDM_START_SCREENSAVER, IDM_TOGGLE_PAUSE, IDM_TOGGLE_PERFORMANCE, TrayInfo, WM_TRAY_ICON, menu_wallpaper,
    show_tray_menu,
};

#[derive(Clone)]
pub struct SystemEvents {
    pub explorer_restarted: Arc<AtomicBool>,
    pub display_changed: Arc<AtomicBool>,
    pub power_changed: Arc<AtomicBool>,
    pub suspended: Arc<AtomicBool>,
    pub is_paused: Arc<AtomicBool>,
    pub last_restart_signal: Arc<Mutex<Option<Instant>>>,
    pub last_display_signal: Arc<Mutex<Option<Instant>>>,
    pub open_studio_requested: Arc<AtomicBool>,
    pub toggle_pause_requested: Arc<AtomicBool>,
    pub next_wallpaper_requested: Arc<AtomicBool>,
    pub toggle_performance_requested: Arc<AtomicBool>,
    pub start_screensaver_requested: Arc<AtomicBool>,
    pub switch_wallpaper_requested: Arc<Mutex<Option<String>>>,
    pub exit_requested: Arc<AtomicBool>,
    /// O que está no ar, para o menu da bandeja (o motor atualiza).
    pub tray_info: Arc<Mutex<TrayInfo>>,
}

impl Default for SystemEvents {
    fn default() -> Self {
        Self {
            explorer_restarted: Arc::new(AtomicBool::new(false)),
            display_changed: Arc::new(AtomicBool::new(false)),
            power_changed: Arc::new(AtomicBool::new(false)),
            suspended: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
            last_restart_signal: Arc::new(Mutex::new(None)),
            last_display_signal: Arc::new(Mutex::new(None)),
            open_studio_requested: Arc::new(AtomicBool::new(false)),
            toggle_pause_requested: Arc::new(AtomicBool::new(false)),
            next_wallpaper_requested: Arc::new(AtomicBool::new(false)),
            toggle_performance_requested: Arc::new(AtomicBool::new(false)),
            start_screensaver_requested: Arc::new(AtomicBool::new(false)),
            switch_wallpaper_requested: Arc::new(Mutex::new(None)),
            exit_requested: Arc::new(AtomicBool::new(false)),
            tray_info: Arc::new(Mutex::new(TrayInfo::default())),
        }
    }
}

pub struct DesktopEventListener {
    hwnd: HWND,
    pub events: SystemEvents,
}

impl DesktopEventListener {
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

static GLOBAL_EVENTS: Mutex<Option<SystemEvents>> = Mutex::new(None);
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

impl DesktopEventListener {
    pub fn new() -> Result<Self> {
        let events = SystemEvents::default();

        if let Ok(mut g) = GLOBAL_EVENTS.lock() {
            *g = Some(events.clone());
        }
        let taskbar_msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        TASKBAR_CREATED_MSG.store(taskbar_msg, Ordering::SeqCst);

        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class_name = w!("IsNotAEngine_SystemListener");

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(listener_wndproc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);

            // Janela top-level oculta (0x0, WS_POPUP sem WS_VISIBLE).
            // Precisa ser top-level para receber mensagens de broadcast do sistema.
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("IsNotAEngine_Listener"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?;

            info!(
                hwnd = ?hwnd,
                taskbar_msg = taskbar_msg,
                "Listener de eventos de sistema inicializado"
            );

            Ok(Self { hwnd, events })
        }
    }
}

impl Drop for DesktopEventListener {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        if let Ok(mut g) = GLOBAL_EVENTS.lock() {
            *g = None;
        }
    }
}

extern "system" fn listener_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let events = match GLOBAL_EVENTS.lock() {
        Ok(guard) => match &*guard {
            Some(e) => e.clone(),
            None => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        },
        Err(_) => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };

    let taskbar_created_msg = TASKBAR_CREATED_MSG.load(Ordering::SeqCst);
    if msg == taskbar_created_msg && taskbar_created_msg != 0 {
        warn!("Explorer reiniciado detectado (TaskbarCreated recebido)!");
        events.explorer_restarted.store(true, Ordering::SeqCst);
        if let Ok(mut lock) = events.last_restart_signal.lock() {
            *lock = Some(Instant::now());
        }
        return LRESULT(0);
    }

    if msg == WM_TRAY_ICON {
        let event_type = (lparam.0 & 0xffff) as u32;
        if event_type == WM_RBUTTONUP || event_type == WM_CONTEXTMENU {
            let info = events.tray_info.lock().map(|i| i.clone()).unwrap_or_default();
            show_tray_menu(hwnd, events.is_paused.load(Ordering::SeqCst), &info);
        } else if event_type == WM_LBUTTONUP || event_type == WM_LBUTTONDBLCLK {
            events.open_studio_requested.store(true, Ordering::SeqCst);
        }
        return LRESULT(0);
    }

    match msg {
        WM_COMMAND => {
            let cmd = wparam.0 & 0xffff;
            match cmd {
                IDM_OPEN_STUDIO => {
                    events.open_studio_requested.store(true, Ordering::SeqCst);
                }
                IDM_TOGGLE_PAUSE => {
                    events.toggle_pause_requested.store(true, Ordering::SeqCst);
                }
                IDM_TOGGLE_PERFORMANCE => {
                    events.toggle_performance_requested.store(true, Ordering::SeqCst);
                }
                IDM_NEXT_WALLPAPER => {
                    events.next_wallpaper_requested.store(true, Ordering::SeqCst);
                }
                IDM_START_SCREENSAVER => {
                    events.start_screensaver_requested.store(true, Ordering::SeqCst);
                }
                IDM_EXIT => {
                    events.exit_requested.store(true, Ordering::SeqCst);
                }
                other => {
                    if let (Some(wp), Ok(mut lock)) = (menu_wallpaper(other), events.switch_wallpaper_requested.lock()) {
                        *lock = Some(wp);
                    }
                }
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {

            info!("Configuração de exibição alterada (WM_DISPLAYCHANGE recebido)");
            events.display_changed.store(true, Ordering::SeqCst);
            if let Ok(mut lock) = events.last_display_signal.lock() {
                *lock = Some(Instant::now());
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            const PBT_APMSUSPEND: usize = 0x0004;
            const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
            const PBT_APMRESUMESUSPEND: usize = 0x0007;

            match wparam.0 {
                PBT_APMSUSPEND => {
                    warn!("Sistema entrando em suspensão (PBT_APMSUSPEND)");
                    events.suspended.store(true, Ordering::SeqCst);
                }
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                    info!("Sistema retornando de suspensão");
                    events.suspended.store(false, Ordering::SeqCst);
                    events.power_changed.store(true, Ordering::SeqCst);
                }
                _ => {
                    events.power_changed.store(true, Ordering::SeqCst);
                }
            }
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
