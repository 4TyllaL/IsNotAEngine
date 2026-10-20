//! Janela do IsNotAEngine Studio com WebView2 nativo de alta fidelidade e estilo Dark Glass.

use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{error, info};
use windows::{
    Win32::Foundation::*,
    Win32::Graphics::Gdi::*,
    Win32::System::LibraryLoader::GetModuleHandleW,
    Win32::UI::Controls::Dialogs::*,
    Win32::UI::HiDpi::GetDpiForSystem,
    Win32::UI::Shell::ShellExecuteW,
    Win32::UI::WindowsAndMessaging::*,
    core::*,
};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use webview2_com::{
    CreateCoreWebView2EnvironmentCompletedHandler,
    Microsoft::Web::WebView2::Win32::*,
    wait_with_pump,
};
use wry::{Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows, dpi::{LogicalPosition, PhysicalSize, Position, Size}};

use crate::config::EngineConfig;
use crate::desktop::MonitorInfo;
use crate::render::offscreen::{OffscreenRenderer, preview_constants};
use crate::wallpaper::catalog;

const BASE_WIDTH: i32 = 1160;
const BASE_HEIGHT: i32 = 820;

pub const WM_STUDIO_COMMAND: u32 = WM_USER + 500;

const STUDIO_HTML: &str = include_str!("studio.html");

fn create_webview_environment(data_dir: &Path) -> windows::core::Result<ICoreWebView2Environment> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |error_code, env| {
            let res = (|| {
                error_code?;
                env.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))
            })();
            let _ = tx.send(res);
            Ok(())
        },
    ));

    let _ = std::fs::create_dir_all(data_dir);
    let data_dir_h = windows::core::HSTRING::from(data_dir.to_string_lossy().as_ref());

    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &data_dir_h,
            None,
            &handler,
        )?;
    }

    
    wait_with_pump(rx).map_err(|e| match e {
        webview2_com::Error::WindowsError(we) => we,
        _ => windows::core::Error::from(windows::Win32::Foundation::E_FAIL),
    })?
}

#[derive(serde::Serialize)]
struct MonitorDto {
    id: usize,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
}

impl From<&MonitorInfo> for MonitorDto {
    fn from(m: &MonitorInfo) -> Self {
        Self { id: m.id, width: m.width, height: m.height, x: m.screen_rect.left, y: m.screen_rect.top }
    }
}

static SELECTED_MONITOR_HINT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct StudioCallbacks {
    pub on_wallpaper_change: Box<dyn Fn(usize, String) + Send + Sync>,
    pub on_fps_change: Box<dyn Fn(u32) + Send + Sync>,
    pub on_pause_covered_change: Box<dyn Fn(bool) + Send + Sync>,
    pub on_pause_battery_change: Box<dyn Fn(bool) + Send + Sync>,
    pub on_speed_change: Box<dyn Fn(f32) + Send + Sync>,
    pub on_tint_change: Box<dyn Fn(String) + Send + Sync>,
    pub on_mouse_enabled_change: Box<dyn Fn(bool) + Send + Sync>,
    pub on_parallax_enabled_change: Box<dyn Fn(bool) + Send + Sync>,
    pub on_sync_all: Box<dyn Fn(String) + Send + Sync>,
    pub on_toggle_pause: Box<dyn Fn() + Send + Sync>,
    pub on_autostart_change: Box<dyn Fn(bool) + Send + Sync>,
    pub on_import_file: Box<dyn Fn(String, usize) + Send + Sync>,
    pub on_import_confirmed: Box<dyn Fn(String, usize) + Send + Sync>,
    /// Mensagens `ai_*` do "Criar com IA" (a mensagem inteira, já em JSON).
    pub on_ai: Box<dyn Fn(serde_json::Value) + Send + Sync>,
    pub on_freeze_wallpaper: Box<dyn Fn(String, usize) + Send + Sync>,
    pub on_import_shadertoy: Box<dyn Fn(String, String, usize) + Send + Sync>,
    pub on_lock_screen_change: Box<dyn Fn(bool) + Send + Sync>,
    /// (origem, tipo, valor): permissão de dados locais para um site remoto.
    /// (spec do wallpaper, nome do parâmetro, valor): ajuste de um `// param:`.
    pub on_shader_param: Box<dyn Fn(String, String, f32) + Send + Sync>,
    /// Parâmetros de um wallpaper de volta aos padrões.
    pub on_reset_shader_params: Box<dyn Fn(String) + Send + Sync>,
    /// Remove um wallpaper importado/criado (vai para a Lixeira).
    pub on_remove_wallpaper: Box<dyn Fn(String) + Send + Sync>,
    /// Renomeia (só o título exibido) um wallpaper criado/importado: caminho, novo título ("" = voltar ao nome do arquivo).
    pub on_wallpaper_title: Box<dyn Fn(String, String) + Send + Sync>,
    /// Pede o tamanho dos dados limpáveis (Ajustes › Dados).
    pub on_data_usage: Box<dyn Fn() + Send + Sync>,
    /// Limpa um tipo de dado (`shader_cache`, `package_cache`, `ai_drafts`, `config_backups`).
    pub on_clean_data: Box<dyn Fn(String) + Send + Sync>,
    /// Restaurar a configuração padrão (o motor guarda um backup antes).
    pub on_reset_config: Box<dyn Fn() + Send + Sync>,
    pub on_playlist_change: Box<dyn Fn(crate::config::PlaylistConfig) + Send + Sync>,
    pub on_schedule_change: Box<dyn Fn(crate::config::ScheduleConfig) + Send + Sync>,
    pub on_screensaver_change: Box<dyn Fn(crate::config::ScreensaverConfig) + Send + Sync>,
    pub on_start_screensaver: Box<dyn Fn() + Send + Sync>,
    pub on_tour_done: Box<dyn Fn(bool) + Send + Sync>,
    pub on_show_console: Box<dyn Fn(bool) + Send + Sync>,
    pub on_performance_mode: Box<dyn Fn(crate::config::PerformanceMode) + Send + Sync>,
}

/// Barra de título e borda escuras na cor do fundo do Studio (sem a moldura clara padrão).
/// Atributos inexistentes em versões antigas do Windows são simplesmente ignorados.
fn apply_dark_frame(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };
    unsafe {
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        // COLORREF é 0x00BBGGRR: fundo #0b0b10, borda #1c1a24, texto #f1eff8.
        for (attr, color) in [(DWMWA_CAPTION_COLOR, 0x00100B0Bu32), (DWMWA_BORDER_COLOR, 0x00241A1C), (DWMWA_TEXT_COLOR, 0x00F8EFF1)] {
            let _ = DwmSetWindowAttribute(hwnd, attr, &color as *const _ as _, 4);
        }
    }
}

struct WindowWrapper(HWND);

impl HasWindowHandle for WindowWrapper {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(NonZeroIsize::new(self.0.0 as isize).unwrap());
        let raw = RawWindowHandle::Win32(handle);
        unsafe { Ok(WindowHandle::borrow_raw(raw)) }
    }
}

pub struct StudioWindow {
    hwnd: HWND,
    webview: Option<Box<WebView>>,
}

static GLOBAL_WEBVIEW_PTR: Mutex<Option<usize>> = Mutex::new(None);

impl StudioWindow {
    pub fn new(
        config: EngineConfig,
        monitors: Vec<MonitorInfo>,
        callbacks: StudioCallbacks,
    ) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class_name = w!("IsNotAEngine_StudioWindowV2");

            let brush = CreateSolidBrush(COLORREF(0x00100B0B));
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(studio_wndproc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: brush,
                hIcon: crate::desktop::tray::app_icon(GetSystemMetrics(SM_CXICON)),
                hIconSm: crate::desktop::tray::app_icon(GetSystemMetrics(SM_CXSMICON)),
                style: CS_HREDRAW | CS_VREDRAW,
                ..Default::default()
            };
            RegisterClassExW(&wc);

            let (dpi, work) = {
                let dpi = GetDpiForSystem().max(96);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
                let _ = GetMonitorInfoW(monitor, &mut info);
                (dpi, info.rcWork)
            };
            let room = |work: i32, size: f32| (work as f32 * 0.94 / size * 96.0).floor() as u32;
            let dpi = dpi
                .min(room(work.right - work.left, BASE_WIDTH as f32))
                .min(room(work.bottom - work.top, BASE_HEIGHT as f32))
                .max(72);
            let scale = dpi as f32 / 96.0;
            let width = (BASE_WIDTH as f32 * scale).round() as i32;
            let height = (BASE_HEIGHT as f32 * scale).round() as i32;

            let x = work.left + (work.right - work.left - width) / 2;
            let y = work.top + ((work.bottom - work.top - height) / 3).max(0);

            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW,
                class_name,
                w!("IsNotAEngine Studio"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                x,
                y,
                width,
                height,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            apply_dark_frame(hwnd);

            let monitors_dto: Vec<MonitorDto> = monitors
                .iter()
                .map(MonitorDto::from)
                .collect();
            let monitors_json = serde_json::to_string(&monitors_dto).unwrap_or_else(|_| "[]".to_string());
            let config_json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".to_string());

            let init_script = format!(
                "window.__ENGINE_INIT__ = {{\n\
                 config: {},\n\
                 monitors: {},\n\
                 catalog: {},\n\
                 imported: {},\n\
                 factory: {},\n\
                 version: \"{}\"\n\
                 }};\n",
                config_json,
                monitors_json,
                catalog::catalog_json(),
                serde_json::to_string(&crate::import::list_imported(&EngineConfig::config_path().parent().unwrap_or(Path::new(".")).join("wallpapers"))).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&crate::wallpaper::factory::list()).unwrap_or_else(|_| "[]".into()),
                env!("CARGO_PKG_VERSION")
            );

            let shared_callbacks = Arc::new(callbacks);

            let data_dir = if cfg!(test) {
                std::env::temp_dir().join(format!("isnotaengine_test_wv_{}", std::process::id()))
            } else {
                EngineConfig::config_path().parent().unwrap_or(Path::new(".")).join("webview_data")
            };
            let webview_res = crate::wallpaper::web::with_busy_retry(|dir| {
                let env = create_webview_environment(&dir)
                    .map_err(|e| wry::Error::WebView2Error(webview2_com::Error::WindowsError(e)))?;
                WebViewBuilder::new()
                    .with_environment(env)
                    .with_bounds(Rect {
                        position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
                        size: Size::Physical(PhysicalSize::new(width.max(1) as u32, height.max(1) as u32)),
                    })
                    .with_url("studio://localhost/")
                    .with_custom_protocol("studio".to_string(), |_id, req| {
                        handle_studio_protocol(&req)
                    })
                    .with_initialization_script(&init_script)
                    .with_ipc_handler({
                        let cbs = shared_callbacks.clone();
                        let hwnd_picker = hwnd;
                        move |req: wry::http::Request<String>| {
                            if !is_studio_origin(req.uri()) {
                                error!(origin = %req.uri(), "Mensagem IPC ignorada: origem não é o Studio");
                                return;
                            }
                            handle_ipc_message(req.body(), &cbs, hwnd_picker);
                        }
                    })
                    .with_drag_drop_handler({
                        let cbs = shared_callbacks.clone();
                        move |event| match event {
                            wry::DragDropEvent::Drop { paths, .. } => {
                                let mon_id = SELECTED_MONITOR_HINT.load(std::sync::atomic::Ordering::Relaxed);
                                for p in paths {
                                    let p_str = p.to_string_lossy().to_string();
                                    (cbs.on_import_file)(p_str, mon_id);
                                }
                                true
                            }
                            _ => true,
                        }
                    })
                    .build(&WindowWrapper(hwnd))
            }, &data_dir);

            let webview = match webview_res {
                Ok(wv) => {
                    info!("WebView2 Evergreen inicializado para o IsNotAEngine Studio");
                    // A janela nasce escondida (sem WS_VISIBLE): a WebView também, ou as prévias da galeria
                    // renderizariam o tempo todo sem ninguém ver. `show()` a torna visível de novo.
                    let _ = wv.set_visible(false);
                    let boxed = Box::new(wv);
                    let ptr = Box::as_ref(&boxed) as *const WebView as usize;
                    if let Ok(mut g) = GLOBAL_WEBVIEW_PTR.lock() {
                        *g = Some(ptr);
                    }
                    Some(boxed)
                }
                Err(e) => {
                    error!(error = %e, "Falha crítica ao inicializar WebView2 no Studio");
                    return Err(Error::new(E_FAIL, format!("Falha crítica ao inicializar WebView2: {:?}", e)));
                }
            };

            Ok(Self { hwnd, webview })
        }
    }

    pub fn show(&self) {
        info!("StudioWindow::show executado");
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
            let _ = BringWindowToTop(self.hwnd);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = UpdateWindow(self.hwnd);
        }
        if let Some(ref wv) = self.webview {
            let _ = wv.set_visible(true);
            let _ = wv.focus();
            let mut rc = RECT::default();
            unsafe {
                let _ = GetClientRect(self.hwnd, &mut rc);
            }
            let w = (rc.right - rc.left).max(1) as u32;
            let h = (rc.bottom - rc.top).max(1) as u32;
            let _ = wv.set_bounds(Rect {
                position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
                size: Size::Physical(PhysicalSize::new(w, h)),
            });
        }
    }

    pub fn update_monitors(&self, monitors: Vec<MonitorInfo>) {
        let dtos: Vec<MonitorDto> = monitors
            .iter()
            .map(MonitorDto::from)
            .collect();
        if let Ok(json) = serde_json::to_string(&dtos)
            && let Some(ref wv) = self.webview {
                let script = format!("updateMonitorsList({});", json);
                let _ = wv.evaluate_script(&script);
            }
    }

    #[allow(dead_code)]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

#[allow(dead_code)]
pub enum StudioThreadCommand {
    /// Executa um script na página do Studio (sempre na thread da UI).
    Eval(String),
    UpdateMonitors(Vec<MonitorInfo>),
    Show,
    Hide,
}

#[derive(Clone)]
pub struct StudioHandle {
    hwnd_raw: usize,
    thread_id: u32,
    tx: std::sync::mpsc::Sender<StudioThreadCommand>,
}

impl StudioHandle {
    pub fn spawn(
        config: EngineConfig,
        monitors: Vec<MonitorInfo>,
        callbacks: StudioCallbacks,
    ) -> Result<Self> {
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let open_on_start = config.open_studio;

        std::thread::Builder::new()
            .name("StudioUIThread".to_string())
            .spawn(move || {
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(
                        None,
                        windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
                    );
                }

                let thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

                let studio = match StudioWindow::new(config, monitors, callbacks) {
                    Ok(s) => s,
                    Err(e) => {
                        error!(error = %e, "Erro ao instanciar StudioWindow na thread da UI");
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };

                let hwnd = studio.hwnd;
                if open_on_start {
                    studio.show();
                }
                let _ = init_tx.send(Ok((hwnd.0 as usize, thread_id)));

                // Loop de mensagens Win32 exclusivo da thread do Studio
                unsafe {
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        while let Ok(cmd) = cmd_rx.try_recv() {
                            match cmd {
                                StudioThreadCommand::Eval(script) => {
                                    with_global_webview(|wv| {
                                        let _ = wv.evaluate_script(&script);
                                    });
                                }
                                StudioThreadCommand::UpdateMonitors(mons) => {
                                    studio.update_monitors(mons);
                                }
                                StudioThreadCommand::Show => {
                                    info!("StudioUIThread: comando Show executado");
                                    studio.show();
                                }
                                StudioThreadCommand::Hide => {
                                    info!("StudioUIThread: comando Hide executado");
                                    let _ = ShowWindow(studio.hwnd, SW_HIDE);
                                }
                            }
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .map_err(|e| Error::new(E_FAIL, format!("Falha ao iniciar thread da UI do Studio: {}", e)))?;

        let (hwnd_raw, thread_id) = init_rx
            .recv()
            .map_err(|_| Error::new(E_FAIL, "Thread do Studio encerrou prematuramente"))??;

        Ok(Self {
            hwnd_raw,
            thread_id,
            tx: cmd_tx,
        })
    }

    pub fn hwnd(&self) -> HWND {
        HWND(self.hwnd_raw as *mut _)
    }

    pub fn show(&self) {
        info!("StudioHandle::show solicitado");
        let _ = self.tx.send(StudioThreadCommand::Show);
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                WM_STUDIO_COMMAND,
                WPARAM(0),
                LPARAM(0),
            );
            let _ = PostMessageW(Some(self.hwnd()), WM_STUDIO_COMMAND, WPARAM(0), LPARAM(0));
        }
    }

    #[allow(dead_code)]
    pub fn hide(&self) {
        let _ = self.tx.send(StudioThreadCommand::Hide);
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                WM_STUDIO_COMMAND,
                WPARAM(0),
                LPARAM(0),
            );
            let _ = PostMessageW(Some(self.hwnd()), WM_STUDIO_COMMAND, WPARAM(0), LPARAM(0));
        }
    }

    pub fn update_monitors(&self, monitors: Vec<MonitorInfo>) {
        let _ = self.tx.send(StudioThreadCommand::UpdateMonitors(monitors));
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                WM_STUDIO_COMMAND,
                WPARAM(0),
                LPARAM(0),
            );
            let _ = PostMessageW(Some(self.hwnd()), WM_STUDIO_COMMAND, WPARAM(0), LPARAM(0));
        }
    }

    /// Roda `script` na página do Studio a partir de qualquer thread (o WebView só pode ser tocado
    /// pela thread da UI, então o script vai por fila e a thread é acordada).
    pub fn post_script(&self, script: String) {
        let _ = self.tx.send(StudioThreadCommand::Eval(script));
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(self.thread_id, WM_STUDIO_COMMAND, WPARAM(0), LPARAM(0));
        }
    }

    /// Roda `script` na página do Studio. Pode ser chamado de qualquer thread: vai pela fila da thread da
    /// UI (como `post_script`). Chamar o WebView2 direto da thread do motor derruba o processo
    /// (objeto COM de outra thread: `EmbeddedBrowserWebView.dll` aborta com 0x80000003).
    pub fn eval_script(&self, script: &str) {
        self.post_script(script.to_string());
    }

    pub fn notify_wallpaper_imported(
        &self,
        monitor_id: usize,
        title: &str,
        path: &str,
        wp_type: &str,
    ) {
        let title_json = serde_json::to_string(title).unwrap_or_default();
        let path_json = serde_json::to_string(path).unwrap_or_default();
        let type_json = serde_json::to_string(wp_type).unwrap_or_default();
        let script = format!(
            "window.onWallpaperImported && window.onWallpaperImported({}, {}, {}, {});",
            monitor_id, title_json, path_json, type_json
        );
        self.eval_script(&script);
    }
}

impl Drop for StudioWindow {
    fn drop(&mut self) {
        if let Ok(mut g) = GLOBAL_WEBVIEW_PTR.lock() {
            *g = None;
        }
    }
}

/// Executa `f` com a WebView global, se existir.
fn with_global_webview(f: impl FnOnce(&WebView)) {
    if let Ok(g) = GLOBAL_WEBVIEW_PTR.lock()
        && let Some(ptr) = *g {
            // SAFETY: o ponteiro é gravado pela thread da UI enquanto o `StudioWindow` vive e
            // zerado no `Drop`, sob o mesmo mutex; segurar o lock impede o uso após a liberação.
            f(unsafe { &*(ptr as *const WebView) });
        }
}

/// Mostra/esconde a WebView do Studio. Escondida, a página recebe `visibilitychange` (hidden)
/// e o WebView2 suspende renderização e animações.
fn set_webview_visible(visible: bool) {
    with_global_webview(|wv| {
        let _ = wv.set_visible(visible);
    });
}

/// Procedimento da janela do Studio (chamado pelo Windows na thread da UI).
unsafe extern "system" fn studio_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: argumentos vêm direto do Windows para esta mesma janela.
    let default = || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_SIZE => {
            // Minimizar não esconde a WebView para o WebView2: sem isto as prévias continuam
            // renderizando (GPU/CPU) com a janela fora da tela. Esconde ao minimizar, mostra ao restaurar.
            set_webview_visible(wparam.0 as u32 != SIZE_MINIMIZED);
            let width = (lparam.0 & 0xFFFF) as u32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if width > 0 && height > 0 {
                with_global_webview(|wv| {
                    let _ = wv.set_bounds(Rect {
                        position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
                        size: Size::Physical(PhysicalSize::new(width, height)),
                    });
                });
            }
            default()
        }
        WM_SHOWWINDOW => {
            // Qualquer caminho que esconda a janela (fechar, bandeja, comando Hide) suspende a WebView.
            if wparam.0 == 0 {
                set_webview_visible(false);
            }
            default()
        }
        WM_CLOSE => {
            // Esconde também a WebView para o WebView2 suspender animações e prévias.
            set_webview_visible(false);
            // SAFETY: `hwnd` é a janela deste procedimento.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        WM_STUDIO_COMMAND => LRESULT(0),
        _ => default(),
    }
}

/// Política de conteúdo da UI do Studio: nada de rede nem de recursos de fora (o WebView2 serve o protocolo
/// `studio://` como `http(s)://studio.localhost`), sem plugins, sem `<base>` e sem formulários. Os scripts
/// e estilos inline da UI continuam permitidos (`unsafe-inline`), mas não carregam código de outra origem.
const STUDIO_CSP: &str = "default-src 'none'; \
    script-src 'self' 'unsafe-inline' studio: http://studio.localhost https://studio.localhost; \
    style-src 'self' 'unsafe-inline' studio: http://studio.localhost https://studio.localhost; \
    img-src 'self' data: blob: studio: http://studio.localhost https://studio.localhost; \
    media-src 'self' blob: studio: http://studio.localhost https://studio.localhost; \
    font-src 'self' data: studio: http://studio.localhost https://studio.localhost; \
    connect-src 'self' studio: http://studio.localhost https://studio.localhost; \
    frame-src 'self' studio: http://studio.localhost https://studio.localhost; \
    object-src 'none'; base-uri 'none'; form-action 'none'";

/// O IPC só vale para a página do próprio Studio (`studio://` ou `http(s)://studio.localhost`).
fn is_studio_origin(uri: &wry::http::Uri) -> bool {
    let ours = uri.scheme_str() == Some("studio") || uri.host() == Some("studio.localhost");
    // Só o documento do Studio: as prévias de wallpapers importados (`/imp/file/...`, em iframe) e o Lab
    // (`/lab`) vivem na mesma origem, mas não podem mandar comandos ao motor.
    ours && matches!(uri.path(), "" | "/" | "/index.html")
}

fn handle_ipc_message(req: &str, cbs: &Arc<StudioCallbacks>, hwnd: HWND) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(req)
        && let Some(action) = v.get("action").and_then(|a| a.as_str()) {
            tracing::debug!(action, "Mensagem IPC do Studio");
            match action {
                "set_wallpaper" => {
                    if let (Some(id), Some(path)) = (
                        v.get("monitor").and_then(|m| m.as_u64()),
                        v.get("path").and_then(|p| p.as_str()),
                    ) {
                        (cbs.on_wallpaper_change)(id as usize, path.to_string());
                    }
                }
                "sync_all" => {
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        (cbs.on_sync_all)(path.to_string());
                    }
                }
                "set_speed" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_f64()) {
                        (cbs.on_speed_change)(val as f32);
                    }
                }
                "set_tint" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_str()) {
                        (cbs.on_tint_change)(val.to_string());
                    }
                }
                "set_mouse_enabled" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_mouse_enabled_change)(val);
                    }
                }
                "set_parallax_enabled" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_parallax_enabled_change)(val);
                    }
                }
                "set_fps" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_u64()) {
                        (cbs.on_fps_change)(val as u32);
                    }
                }
                "set_pause_covered" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_pause_covered_change)(val);
                    }
                }
                "set_pause_battery" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_pause_battery_change)(val);
                    }
                }
                "toggle_pause" => {
                    (cbs.on_toggle_pause)();
                }
                "select_monitor" => {
                    if let Some(m) = v.get("monitor").and_then(|m| m.as_u64()) {
                        SELECTED_MONITOR_HINT.store(m as usize, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                a if a.starts_with("ai_") => (cbs.on_ai)(v.clone()),
                "freeze_wallpaper" => {
                    let mon_id = v.get("monitor").and_then(|m| m.as_u64()).unwrap_or(0) as usize;
                    if let Some(spec) = v.get("spec").and_then(|s| s.as_str()) {
                        (cbs.on_freeze_wallpaper)(spec.to_string(), mon_id);
                    }
                }
                "import_shadertoy" => {
                    let mon_id = v.get("monitor").and_then(|m| m.as_u64()).unwrap_or(0) as usize;
                    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
                    if let Some(code) = v.get("code").and_then(|c| c.as_str()) {
                        (cbs.on_import_shadertoy)(title, code.to_string(), mon_id);
                    }
                }
                "set_lock_screen" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_lock_screen_change)(val);
                    }
                }
                "reset_config" => (cbs.on_reset_config)(),
                "remove_wallpaper" => {
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        (cbs.on_remove_wallpaper)(path.to_string());
                    }
                }
                "set_wallpaper_title" => {
                    if let (Some(path), Some(title)) = (v.get("path").and_then(|p| p.as_str()), v.get("title").and_then(|t| t.as_str())) {
                        (cbs.on_wallpaper_title)(path.to_string(), title.to_string());
                    }
                }
                "data_usage" => (cbs.on_data_usage)(),
                "clean_data" => {
                    if let Some(what) = v.get("what").and_then(|w| w.as_str()) {
                        (cbs.on_clean_data)(what.to_string());
                    }
                }
                "set_shader_param" => {
                    if let (Some(spec), Some(name), Some(val)) = (
                        v.get("spec").and_then(|s| s.as_str()),
                        v.get("name").and_then(|n| n.as_str()),
                        v.get("value").and_then(|x| x.as_f64()),
                    ) {
                        (cbs.on_shader_param)(spec.to_string(), name.to_string(), val as f32);
                    }
                }
                "reset_shader_params" => {
                    if let Some(spec) = v.get("spec").and_then(|s| s.as_str()) {
                        (cbs.on_reset_shader_params)(spec.to_string());
                    }
                }
                "set_playlist" => {
                    match v.get("value").and_then(|val| serde_json::from_value(val.clone()).ok()) {
                        Some(cfg) => (cbs.on_playlist_change)(cfg),
                        None => error!("set_playlist com valor inválido"),
                    }
                }
                "set_schedule" => {
                    match v.get("value").and_then(|val| serde_json::from_value(val.clone()).ok()) {
                        Some(cfg) => (cbs.on_schedule_change)(cfg),
                        None => error!("set_schedule com valor inválido"),
                    }
                }
                "set_screensaver" => {
                    match v.get("value").and_then(|val| serde_json::from_value(val.clone()).ok()) {
                        Some(cfg) => (cbs.on_screensaver_change)(cfg),
                        None => error!("set_screensaver com valor inválido"),
                    }
                }
                "start_screensaver" => {
                    (cbs.on_start_screensaver)();
                }
                "set_tour_done" => {
                    let val = v.get("value").and_then(|val| val.as_bool()).unwrap_or(true);
                    (cbs.on_tour_done)(val);
                }
                "set_performance_mode" => {
                    if let Some(mode) = v.get("value").and_then(|val| val.as_str()).and_then(crate::config::PerformanceMode::parse) {
                        (cbs.on_performance_mode)(mode);
                    }
                }
                "set_show_console" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_show_console)(val);
                    }
                }
                "set_autostart" => {
                    if let Some(val) = v.get("value").and_then(|val| val.as_bool()) {
                        (cbs.on_autostart_change)(val);
                    }
                }
                "import_file" => {
                    let mon_id = v.get("monitor").and_then(|m| m.as_u64()).unwrap_or(0) as usize;
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        (cbs.on_import_file)(path.to_string(), mon_id);
                    }
                }
                "confirm_import" => {
                    let mon_id = v.get("monitor").and_then(|m| m.as_u64()).unwrap_or(0) as usize;
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        (cbs.on_import_confirmed)(path.to_string(), mon_id);
                    }
                }
                "choose_file" => {
                    let mon_id = v.get("monitor").and_then(|m| m.as_u64()).unwrap_or(0) as usize;
                    SELECTED_MONITOR_HINT.store(mon_id, std::sync::atomic::Ordering::Relaxed);
                    if let Some(path) = choose_wallpaper_file(hwnd) {
                        (cbs.on_import_file)(path, mon_id);
                    }
                }
                "open_config_folder" => {
                    let path = EngineConfig::config_path();
                    let folder = path.parent().unwrap_or(Path::new("."));
                    let path_u16: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
                    unsafe {
                        ShellExecuteW(None, w!("open"), PCWSTR(path_u16.as_ptr()), None, None, SW_SHOW);
                    }
                }
                _ => {}
            }
        }
}

use std::os::windows::ffi::OsStrExt;

fn choose_wallpaper_file(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut file_buf = [0u16; 1024];
        let filter: Vec<u16> =
            "Todos os Formatos Suportados (*.hlsl;*.glsl;*.png;*.jpg;*.webp;*.livewallpaper;*.mp4;*.webm;*.html)\0*.hlsl;*.glsl;*.frag;*.png;*.jpg;*.jpeg;*.bmp;*.webp;*.tif;*.tiff;*.livewallpaper;*.mp4;*.webm;*.wmv;*.mov;*.avi;*.mkv;*.html;*.htm\0Imagens (*.png;*.jpg;*.webp;*.bmp;*.tif)\0*.png;*.jpg;*.jpeg;*.bmp;*.webp;*.tif;*.tiff\0Shaders HLSL (*.hlsl)\0*.hlsl\0Shadertoy GLSL (*.glsl;*.frag)\0*.glsl;*.frag\0Pacotes LiveWallpaper (*.livewallpaper)\0*.livewallpaper\0Vídeos (*.mp4;*.webm;*.mov)\0*.mp4;*.webm;*.wmv;*.mov;*.avi;*.mkv\0Web Wallpapers (*.html;*.htm)\0*.html;*.htm\0Todos os arquivos (*.*)\0*.*\0\0"
                .encode_utf16()
                .collect();

        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: hwnd,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(file_buf.as_mut_ptr()),
            nMaxFile: file_buf.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
            ..Default::default()
        };

        if GetOpenFileNameW(&mut ofn).as_bool() {
            let len = file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len());
            Some(String::from_utf16_lossy(&file_buf[..len]))
        } else {
            None
        }
    }
}

/// Script injetado nas prévias web do Studio: simula o áudio do motor.
const PREVIEW_BRIDGE: &str = r#"<script>
window.engine = { audio: { bass: 0, mid: 0, treble: 0, volume: 0, spectrum: new Array(16).fill(0) }, _cbs: [],
  onAudio(cb) { this._cbs.push(cb); },
  media: { title: "Faixa de demonstração", artist: "IsNotAEngine", album: "Prévia do Studio", app: "Studio",
           playing: true, position_ms: 42000, duration_ms: 214000, cover: "" },
  onMedia(cb) { cb(this.media); } };
setInterval(() => {
  const t = performance.now() / 1000, k = Math.exp(-((t * 2.1) % 1) * 6), a = engine.audio;
  a.bass = 0.25 + 0.6 * k; a.mid = 0.2 + 0.2 * Math.sin(t * 1.7); a.treble = 0.15 + 0.15 * Math.sin(t * 3.1);
  a.volume = 0.45 + 0.2 * k;
  a.spectrum = a.spectrum.map((_, i) => Math.max(0, Math.min(1, (1 - i / 22) * (0.35 + 0.65 * (0.5 + 0.5 * Math.sin(t * (1.3 + i * 0.27) + i))) * (i < 3 ? 0.5 + 0.5 * k : 0.8))));
  for (const cb of engine._cbs) cb(a);
}, 33);
</script>"#;

thread_local! {
    static RENDERER: std::cell::RefCell<Option<OffscreenRenderer>> = const { std::cell::RefCell::new(None) };
}

fn query_param(query: &str, key: &str) -> Option<f32> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| v.parse().ok())
}

type Resp = wry::http::Response<std::borrow::Cow<'static, [u8]>>;

fn respond(status: u16, mime: &str, body: std::borrow::Cow<'static, [u8]>) -> Resp {
    wry::http::Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap()
}

/// Renderiza um frame do shader `id` com o mesmo HLSL usado no desktop.
fn render_frame(id: &str, query: &str) -> Option<Vec<u8>> {
    let entry = catalog::find(id).filter(|e| e.kind == catalog::Kind::Shader)?;
    let w = query_param(query, "w").unwrap_or(384.0).clamp(16.0, 1920.0) as u32;
    let h = query_param(query, "h").unwrap_or(216.0).clamp(16.0, 1080.0) as u32;
    let t = query_param(query, "t").unwrap_or(6.0);
    let tint = match (query_param(query, "r"), query_param(query, "g"), query_param(query, "b")) {
        (Some(r), Some(g), Some(b)) => [r, g, b, 1.0],
        _ => [0.0; 4],
    };
    let dev = Path::new("assets/wallpapers/shaders").join(format!("{id}.hlsl"));
    let code = std::fs::read_to_string(dev).unwrap_or_else(|_| entry.source.to_string());
    let overrides = crate::render::params::parse_overrides(&query_str(query, "p").unwrap_or_default());
    RENDERER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = OffscreenRenderer::new().map_err(|e| error!(error = %e, "Renderizador de prévias indisponível")).ok();
        }
        let renderer = slot.as_mut()?;
        renderer
            .render_with_params(id, &catalog::with_common(&code), preview_constants(w, h, t, tint), Some(&overrides))
            .map_err(|e| error!(id, error = %e, "Falha ao renderizar prévia"))
            .ok()
    })
}

const LAB_HTML: &str = include_str!("lab.html");

/// Rotas do Shader Lab embutido (`/lab/...`): compilar, quadro, salvar e fontes do catálogo.
fn handle_lab(route: &str, query: &str, body: &[u8]) -> Resp {
    use std::borrow::Cow;
    let text = |status: u16, s: String| respond(status, "text/plain; charset=utf-8", Cow::Owned(s.into_bytes()));
    match route {
        "code" => match super::lab::set_code(&String::from_utf8_lossy(body)) {
            Ok(()) => text(200, "ok".into()),
            Err(e) => text(422, e),
        },
        "frame" => {
            let w = query_param(query, "w").unwrap_or(640.0) as u32;
            let h = query_param(query, "h").unwrap_or(360.0) as u32;
            let t = query_param(query, "t").unwrap_or(0.0);
            let tint = match (query_param(query, "r"), query_param(query, "g"), query_param(query, "b")) {
                (Some(r), Some(g), Some(b)) => [r, g, b, 1.0],
                _ => [0.0; 4],
            };
            let overrides = crate::render::params::parse_overrides(&query_str(query, "p").unwrap_or_default());
            match super::lab::frame(w, h, t, tint, &overrides) {
                Some(px) => respond(200, "application/octet-stream", Cow::Owned(px)),
                None => respond(404, "text/plain", Cow::Borrowed(b"404")),
            }
        }
        "save" => match super::lab::save(&query_str(query, "name").unwrap_or_default(), &String::from_utf8_lossy(body)) {
            Ok(p) => text(200, p.to_string_lossy().into_owned()),
            Err(e) => text(500, e),
        },
        "open" => match super::lab::read_saved(&query_str(query, "path").unwrap_or_default()) {
            Ok(src) => text(200, src),
            Err(e) => text(404, e),
        },
        "list" => {
            // Modelos de partida (com parâmetros descritos) primeiro, depois os shaders do catálogo.
            let mut items: Vec<_> = catalog::TEMPLATES
                .iter()
                .map(|t| serde_json::json!({ "id": format!("tpl:{}", t.id), "title": t.title, "group": "Modelos", "description": t.description }))
                .collect();
            items.extend(
                catalog::CATALOG
                    .iter()
                    .filter(|e| e.kind == catalog::Kind::Shader)
                    .map(|e| serde_json::json!({ "id": e.id, "title": e.title, "group": "Catálogo" })),
            );
            respond(200, "application/json", Cow::Owned(serde_json::to_vec(&items).unwrap_or_default()))
        }
        other => {
            let source = other.strip_prefix("source/").and_then(|id| match id.strip_prefix("tpl:") {
                Some(t) => catalog::TEMPLATES.iter().find(|x| x.id == t).map(|x| x.source),
                None => catalog::find(id).filter(|e| e.kind == catalog::Kind::Shader).map(|e| e.source),
            });
            match source {
                Some(src) => text(200, src.to_string()),
                None => text(404, "404".into()),
            }
        }
    }
}

/// Protocolo `studio://`: a própria UI, frames de shaders e prévias de wallpapers web.
fn handle_studio_protocol(req: &wry::http::Request<Vec<u8>>) -> Resp {
    use std::borrow::Cow;
    let path = req.uri().path();
    let query = req.uri().query().unwrap_or("");

    if path == "/lab" {
        let mut resp = respond(200, "text/html; charset=utf-8", Cow::Borrowed(LAB_HTML.as_bytes()));
        resp.headers_mut().insert("Content-Security-Policy", STUDIO_CSP.parse().unwrap());
        return resp;
    }
    if let Some(rest) = path.strip_prefix("/lab/") {
        return handle_lab(rest, query, req.body());
    }
    if path == "/" || path == "/index.html" {
        let mut resp = respond(200, "text/html; charset=utf-8", Cow::Borrowed(STUDIO_HTML.as_bytes()));
        resp.headers_mut().insert("Content-Security-Policy", STUDIO_CSP.parse().unwrap());
        return resp;
    }
    if let Some(id) = path.strip_prefix("/frame/") {
        return match render_frame(id, query) {
            Some(px) => respond(200, "application/octet-stream", Cow::Owned(px)),
            None => respond(404, "text/plain", Cow::Borrowed(b"404")),
        };
    }
    if let Some(id) = path.strip_prefix("/web/")
        && let Some(entry) = catalog::find(id).filter(|e| e.kind == catalog::Kind::Web) {
            let html = match entry.source.find("<head>") {
                Some(i) => format!("{}{}{}", &entry.source[..i + 6], PREVIEW_BRIDGE, &entry.source[i + 6..]),
                None => format!("{PREVIEW_BRIDGE}{}", entry.source),
            };
            return respond(200, "text/html; charset=utf-8", Cow::Owned(html.into_bytes()));
        }
    // Prévias de wallpapers importados: shader renderizado ou arquivo servido do disco.
    if path == "/imp/frame" {
        let file = query_str(query, "path").unwrap_or_default();
        return match render_imported_frame(&file, query, &imported_roots()) {
            Some(px) => respond(200, "application/octet-stream", Cow::Owned(px)),
            None => respond(404, "text/plain", Cow::Borrowed(b"404")),
        };
    }
    if let Some(rest) = path.strip_prefix("/imp/file/") {
        return serve_imported_file(&crate::wallpaper::web::percent_decode(rest), req, &imported_roots());
    }
    respond(404, "text/plain", Cow::Borrowed(b"404"))
}

fn query_str(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| crate::wallpaper::web::percent_decode(&v.replace('+', " ")))
}

/// Pastas de onde as prévias podem ler: wallpapers importados e pacotes extraídos.
fn imported_roots() -> Vec<PathBuf> {
    let base = EngineConfig::config_path();
    let base = base.parent().unwrap_or(Path::new("."));
    // Vídeos de fábrica entram só para a prévia (leitura); o Studio nunca remove nada dali.
    let mut roots = vec![base.join("wallpapers"), base.join("cache")];
    roots.extend(crate::wallpaper::factory::dirs());
    roots
}

/// Caminho canônico de `file` se (e só se) estiver dentro de uma das `roots`.
fn confine_to_roots(file: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(file).canonicalize().ok()?;
    roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| path.starts_with(r))
        .then_some(path)
}

/// Frame de um shader `.hlsl` importado (mesmo caminho de compilação do desktop).
fn render_imported_frame(file: &str, query: &str, roots: &[PathBuf]) -> Option<Vec<u8>> {
    let confined = confine_to_roots(file, roots)?;
    let path = confined.as_path();
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("hlsl")) {
        return None;
    }
    let code = std::fs::read_to_string(path).ok()?;
    let w = query_param(query, "w").unwrap_or(384.0).clamp(16.0, 1920.0) as u32;
    let h = query_param(query, "h").unwrap_or(216.0).clamp(16.0, 1080.0) as u32;
    let t = query_param(query, "t").unwrap_or(6.0);
    let tint = match (query_param(query, "r"), query_param(query, "g"), query_param(query, "b")) {
        (Some(r), Some(g), Some(b)) => [r, g, b, 1.0],
        _ => [0.0; 4],
    };
    // A chave de cache inclui a data de modificação: editar o arquivo recompila a prévia.
    let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
    let key = format!("imp:{file}@{stamp}");
    RENDERER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = OffscreenRenderer::new().map_err(|e| error!(error = %e, "Renderizador de prévias indisponível")).ok();
        }
        slot.as_mut()?
            .render(&key, &catalog::with_common(&code), preview_constants(w, h, t, tint))
            .map_err(|e| error!(file, error = %e, "Falha ao renderizar prévia importada"))
            .ok()
    })
}

/// Serve arquivos de wallpapers importados (página web, seus recursos, vídeo) para as prévias.
/// Só tipos de mídia/web; vídeos respondem a `Range` para não carregar o arquivo inteiro.
fn serve_imported_file(file: &str, req: &wry::http::Request<Vec<u8>>, roots: &[PathBuf]) -> Resp {
    use std::borrow::Cow;
    use std::io::{Read, Seek, SeekFrom};
    const ALLOWED: &[&str] = &[
        "html", "htm", "css", "js", "mjs", "json", "png", "jpg", "jpeg", "bmp", "gif", "svg", "webp", "woff", "woff2",
        "ttf", "otf", "mp3", "wav", "ogg", "mp4", "webm", "mov", "mkv", "wmv", "avi",
    ];
    let Some(confined) = confine_to_roots(file, roots) else {
        return respond(404, "text/plain", Cow::Borrowed(b"404"));
    };
    let path = confined.as_path();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !ALLOWED.contains(&ext.as_str()) || !path.is_file() {
        return respond(404, "text/plain", Cow::Borrowed(b"404"));
    }
    let mime = match ext.as_str() {
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "wmv" => "video/x-ms-wmv",
        "avi" => "video/x-msvideo",
        _ => crate::wallpaper::web::mime_type(file),
    };

    if ext == "html" || ext == "htm" {
        let Ok(src) = std::fs::read_to_string(path) else { return respond(404, "text/plain", Cow::Borrowed(b"404")) };
        let html = match src.find("<head>") {
            Some(i) => format!("{}{}{}", &src[..i + 6], PREVIEW_BRIDGE, &src[i + 6..]),
            None => format!("{PREVIEW_BRIDGE}{src}"),
        };
        let mut resp = respond(200, mime, Cow::Owned(html.into_bytes()));
        resp.headers_mut().insert(
            "Content-Security-Policy",
            crate::wallpaper::web::SANDBOX_CSP.parse().unwrap(),
        );
        return resp;
    }

    let Ok(mut f) = std::fs::File::open(path) else { return respond(404, "text/plain", Cow::Borrowed(b"404")) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let range = req.headers().get("Range").and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok())));
    const CHUNK: u64 = 4 << 20;
    let (start, end, status) = match range {
        Some((a, b)) if a < len => (a, b.unwrap_or(u64::MAX).min(a + CHUNK - 1).min(len - 1), 206),
        Some(_) => return respond(416, "text/plain", Cow::Borrowed(b"416")),
        None if len <= CHUNK => (0, len.saturating_sub(1), 200),
        None => (0, CHUNK - 1, 206),
    };
    let mut buf = vec![0u8; if len == 0 { 0 } else { (end - start + 1) as usize }];
    if f.seek(SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut buf)).is_err() {
        return respond(500, "text/plain", Cow::Borrowed(b"500"));
    }
    let mut builder = wry::http::Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Cache-Control", "no-store");
    if status == 206 {
        builder = builder.header("Content-Range", format!("bytes {start}-{end}/{len}"));
    }
    builder.body(Cow::Owned(buf)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_only_comes_from_the_studio_document_not_from_previews_or_the_lab() {
        let ok = |u: &str| is_studio_origin(&u.parse().unwrap());
        assert!(ok("studio://localhost/"));
        assert!(ok("http://studio.localhost/index.html"));
        assert!(ok("https://studio.localhost/"));
        // prévia de wallpaper importado e Lab: mesma origem, mas sem direito a comandos
        assert!(!ok("studio://localhost/imp/file/C%3A/x/index.html"));
        assert!(!ok("http://studio.localhost/lab"));
        assert!(!ok("http://studio.localhost/web/matrix"));
        // outras origens
        assert!(!ok("https://exemplo.com/"));
        assert!(!ok("http://studio.localhost.evil.com/"));
        assert!(!ok("wallpaper://localhost/index.html"));
    }

    #[test]
    fn studio_documents_carry_a_content_policy_without_remote_sources() {
        for path in ["/", "/lab"] {
            let req = wry::http::Request::builder().uri(format!("studio://localhost{path}")).body(Vec::new()).unwrap();
            let resp = handle_studio_protocol(&req);
            let csp = resp.headers().get("Content-Security-Policy").expect("sem CSP").to_str().unwrap().to_string();
            assert!(csp.contains("default-src 'none'") && csp.contains("object-src 'none'") && csp.contains("base-uri 'none'"), "{path}: {csp}");
            assert!(!csp.contains("http://*") && !csp.contains("https:;") && !csp.contains(" *"), "{csp}");
        }
    }

    /// O WebView2 só pode ser tocado pela thread da UI: chamá-lo da thread do motor derruba o processo
    /// (foi o crash ao aplicar wallpapers pelo Studio). Fora de `web.rs` (que vive na thread da UI), só
    /// podem existir os dois pontos conhecidos deste arquivo; o resto deve usar `StudioHandle::post_script`.
    #[test]
    fn webview_scripts_only_run_from_the_ui_thread_paths() {
        fn scan(dir: &std::path::Path, hits: &mut Vec<(String, usize)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    scan(&path, hits);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let n = std::fs::read_to_string(&path).unwrap().matches(".evaluate_script(").count();
                    if n > 0 {
                        hits.push((path.file_name().unwrap().to_string_lossy().into_owned(), n));
                    }
                }
            }
        }
        let mut hits = Vec::new();
        scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
        for (file, n) in &hits {
            match file.as_str() {
                "web.rs" => {}
                "window.rs" => assert_eq!(*n, 3, "window.rs: só StudioWindow::update_monitors, o tratador da fila da UI e este teste"),
                other => panic!("{other} chama evaluate_script direto; use StudioHandle::post_script (thread da UI)"),
            }
        }
    }

    #[test]
    fn lab_routes_compile_render_and_report_errors() {
        let ok = "float4 ps_main(float4 pos : SV_Position) : SV_Target { return float4(0,1,0,1); }";
        let r = handle_lab("code", "", ok.as_bytes());
        if r.status() != 200 {
            return; // sem GPU neste ambiente
        }
        let f = handle_lab("frame", "w=32&h=16&t=1", b"");
        assert_eq!(f.status(), 200);
        assert_eq!(f.body().len(), 32 * 16 * 4);
        let bad = handle_lab("code", "", b"float4 ps_main(float4 p : SV_Position) : SV_Target { return x; }");
        assert_eq!(bad.status(), 422);
        assert!(!bad.body().is_empty());
        assert_eq!(handle_lab("source/aurora", "", b"").status(), 200);
        assert_eq!(handle_lab("source/matrix", "", b"").status(), 404, "só shaders do catálogo");
        assert_eq!(handle_lab("nada", "", b"").status(), 404);
    }

    #[test]
    fn imported_previews_are_served() {
        let base = std::env::temp_dir().join("isnotaengine_imp_test");
        let dir = base.join("wallpapers");
        std::fs::create_dir_all(&dir).unwrap();
        let roots = vec![dir.clone()];
        let serve = |p: &std::path::Path, range: Option<&str>| {
            let mut b = wry::http::Request::builder().uri("studio://localhost/x");
            if let Some(r) = range {
                b = b.header("Range", r);
            }
            serve_imported_file(&p.to_string_lossy(), &b.body(Vec::new()).unwrap(), &roots)
        };
        let html = dir.join("página.html");
        std::fs::write(&html, "<html><head></head><body>oi</body></html>").unwrap();
        let video = dir.join("clip.mp4");
        std::fs::write(&video, vec![7u8; 1000]).unwrap();
        std::fs::write(dir.join("segredo.txt"), "x").unwrap();
        let fora = base.join("fora.json");
        std::fs::write(&fora, "{}").unwrap();

        let r = serve(&html, None);
        assert_eq!(r.status(), 200);
        let body = String::from_utf8_lossy(r.body());
        assert!(body.contains("window.engine") && body.contains("oi"));
        assert!(r.headers().get("Content-Security-Policy").is_some());

        let r = serve(&video, Some("bytes=10-19"));
        assert_eq!(r.status(), 206);
        assert_eq!(r.body().len(), 10);
        assert_eq!(r.headers()["Content-Range"], "bytes 10-19/1000");

        let shader = dir.join("meu shader.hlsl");
        std::fs::write(&shader, crate::wallpaper::manager::FALLBACK_SOURCE).unwrap();
        let px = render_imported_frame(&shader.to_string_lossy(), "w=64&h=36&t=2", &roots).unwrap();
        assert_eq!(px.len(), 64 * 36 * 4);

        // Fora das pastas de wallpapers (mesmo com extensão permitida), nada é servido.
        assert_eq!(serve(&fora, None).status(), 404);
        assert_eq!(serve(&dir.join("..").join("fora.json"), None).status(), 404);
        let outside_shader = base.join("fora.hlsl");
        std::fs::write(&outside_shader, "x").unwrap();
        assert!(render_imported_frame(&outside_shader.to_string_lossy(), "", &roots).is_none());

        // Tipos fora da lista não são servidos.
        assert_eq!(serve(&dir.join("segredo.txt"), None).status(), 404);
    }

    #[test]
    fn test_studio_window_creation() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }

        let config = EngineConfig::default();
        let callbacks = StudioCallbacks {
            on_wallpaper_change: Box::new(|_, _| {}),
            on_sync_all: Box::new(|_| {}),
            on_fps_change: Box::new(|_| {}),
            on_pause_covered_change: Box::new(|_| {}),
            on_pause_battery_change: Box::new(|_| {}),
            on_speed_change: Box::new(|_| {}),
            on_tint_change: Box::new(|_| {}),
            on_mouse_enabled_change: Box::new(|_| {}),
            on_parallax_enabled_change: Box::new(|_| {}),
            on_toggle_pause: Box::new(|| {}),
            on_autostart_change: Box::new(|_| {}),
            on_import_file: Box::new(|_, _| {}),
            on_import_confirmed: Box::new(|_, _| {}),
            on_ai: Box::new(|_| {}),
            on_freeze_wallpaper: Box::new(|_, _| {}),
            on_import_shadertoy: Box::new(|_, _, _| {}),
            on_lock_screen_change: Box::new(|_| {}),
            on_shader_param: Box::new(|_, _, _| {}),
            on_reset_shader_params: Box::new(|_| {}),
            on_remove_wallpaper: Box::new(|_| {}),
            on_wallpaper_title: Box::new(|_, _| {}),
            on_data_usage: Box::new(|| {}),
            on_clean_data: Box::new(|_| {}),
            on_reset_config: Box::new(|| {}),
            on_playlist_change: Box::new(|_| {}),
            on_schedule_change: Box::new(|_| {}),
            on_screensaver_change: Box::new(|_| {}),
            on_start_screensaver: Box::new(|| {}),
            on_tour_done: Box::new(|_| {}),
            on_show_console: Box::new(|_| {}),
            on_performance_mode: Box::new(|_| {}),
        };

        let studio_handle = StudioHandle::spawn(config, vec![], callbacks);
        assert!(studio_handle.is_ok(), "StudioHandle deve spawnar com sucesso: {:?}", studio_handle.err());
        let handle = studio_handle.unwrap();
        assert!(unsafe { IsWindow(Some(handle.hwnd())).as_bool() });
    }
}
