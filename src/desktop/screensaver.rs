//! Proteção de tela viva (Live Screensaver) nativa para Windows.
//!
//! Suporta tanto ativação automática por tempo ocioso (`GetLastInputInfo`) dentro do motor
//! quanto chamada direta via argumentos de screensaver do Windows (`/s`, `--screensaver`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::{info, warn};
use windows::{
    Win32::Foundation::*,
    Win32::Graphics::Gdi::CreateSolidBrush,
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::System::SystemInformation::GetTickCount,
    Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
    Win32::UI::WindowsAndMessaging::*,
    core::*,
};

use crate::config::PresentMode;
use crate::desktop::MonitorInfo;
use crate::render::{GpuContext, RenderSurface};
use crate::wallpaper::{Wallpaper, WallpaperManager};

static DISMISS_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Retorna o `dwTime` (em ms de uptime) do último evento de teclado/mouse registrado pelo Windows.
pub fn last_input_tick() -> u32 {
    unsafe {
        let mut lii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut lii).as_bool() {
            lii.dwTime
        } else {
            GetTickCount()
        }
    }
}

/// Tempo decorrido desde a última interação do usuário (mouse ou teclado).
pub fn system_idle_duration() -> Duration {
    let now = unsafe { GetTickCount() };
    let last = last_input_tick();
    Duration::from_millis(u64::from(now.wrapping_sub(last)))
}

/// Bloqueia a sessão do Windows (equivalente a pressionar `Win + L`), exibindo a tela de senha/PIN.
pub fn lock_workstation() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn LockWorkStation() -> i32;
    }
    unsafe {
        if LockWorkStation() != 0 {
            info!("Sessão do Windows bloqueada ao encerrar a proteção de tela");
        } else {
            warn!("Não foi possível bloquear a sessão do Windows (LockWorkStation)");
        }
    }
}

pub struct ScreensaverViewport {
    pub monitor: MonitorInfo,
    pub hwnd: HWND,
    pub surface: RenderSurface,
}

pub struct ScreensaverOverlay {
    pub viewports: Vec<ScreensaverViewport>,
    /// Wallpaper dedicado para a proteção de tela (ou fallback para monitores que usam WebWallpaper).
    pub custom_wallpaper: Option<Box<dyn Wallpaper>>,
    pub force_custom: bool,
    pub exit_on_dismiss: bool,
    initial_cursor: POINT,
    initial_tick: u32,
    started_at: Instant,
}

impl ScreensaverOverlay {
    /// Abre janelas `WS_POPUP` `HWND_TOPMOST` cobrindo cada monitor conectado.
    pub fn activate(
        gpu: &GpuContext,
        monitors: &[MonitorInfo],
        custom_spec: &str,
        exit_on_dismiss: bool,
    ) -> Result<Self> {
        DISMISS_REQUESTED.store(false, Ordering::SeqCst);
        let class_name = register_screensaver_class()?;
        let instance = unsafe { GetModuleHandleW(None)? };

        let mut viewports = Vec::with_capacity(monitors.len());
        for m in monitors {
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                    class_name,
                    w!("IsNotAEngine Live Screensaver"),
                    WS_POPUP | WS_VISIBLE,
                    m.screen_rect.left,
                    m.screen_rect.top,
                    m.width as i32,
                    m.height as i32,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )?
            };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    m.screen_rect.left,
                    m.screen_rect.top,
                    m.width as i32,
                    m.height as i32,
                    SWP_SHOWWINDOW,
                );
            }

            match RenderSurface::new(
                &gpu.device,
                &gpu.factory,
                None,
                hwnd,
                PresentMode::Hwnd,
                m.width,
                m.height,
            ) {
                Ok(surface) => {
                    viewports.push(ScreensaverViewport {
                        monitor: m.clone(),
                        hwnd,
                        surface,
                    });
                }
                Err(e) => {
                    warn!(monitor = m.id, error = %e, "Falha ao criar superfície da proteção de tela");
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
            }
        }

        let force_custom = !custom_spec.trim().is_empty();
        let custom_wallpaper = WallpaperManager::build_d3d_standalone(&gpu.device, custom_spec.trim());

        let mut initial_cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut initial_cursor);
            ShowCursor(false);
        }

        info!(
            monitors = viewports.len(),
            custom = %custom_spec,
            "Proteção de tela viva iniciada"
        );

        Ok(Self {
            viewports,
            custom_wallpaper,
            force_custom,
            exit_on_dismiss,
            initial_cursor,
            initial_tick: last_input_tick(),
            started_at: Instant::now(),
        })
    }

    /// Verifica se o usuário mexeu no mouse ou pressionou alguma tecla para encerrar a proteção de tela.
    pub fn should_dismiss(&mut self) -> bool {
        // Pequena janela de tolerância ao abrir (ex.: soltar o botão do mouse no "Testar agora").
        if self.started_at.elapsed() < Duration::from_millis(500) {
            self.initial_tick = last_input_tick();
            unsafe {
                let _ = GetCursorPos(&mut self.initial_cursor);
            }
            DISMISS_REQUESTED.store(false, Ordering::SeqCst);
            return false;
        }

        if DISMISS_REQUESTED.swap(false, Ordering::SeqCst) {
            return true;
        }

        if last_input_tick() != self.initial_tick {
            return true;
        }

        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        (pt.x - self.initial_cursor.x).abs() > 6 || (pt.y - self.initial_cursor.y).abs() > 6
    }
}

impl Drop for ScreensaverOverlay {
    fn drop(&mut self) {
        for v in self.viewports.drain(..) {
            drop(v.surface);
            unsafe {
                let _ = DestroyWindow(v.hwnd);
            }
        }
        unsafe {
            ShowCursor(true);
        }
        info!("Proteção de tela viva encerrada");
    }
}

fn register_screensaver_class() -> Result<PCWSTR> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("IsNotAEngine_ScreensaverOverlay");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(screensaver_wndproc),
            hInstance: instance.into(),
            hCursor: HCURSOR::default(),
            hbrBackground: CreateSolidBrush(COLORREF(0x00000000)),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        Ok(class_name)
    }
}

unsafe extern "system" fn screensaver_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL => {
            DISMISS_REQUESTED.store(true, Ordering::SeqCst);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            unsafe {
                SetCursor(None);
            }
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
