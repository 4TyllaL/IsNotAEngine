//! Orquestrador central (Engine): coordena eventos do Windows, monitores,
//! oclusão ("pausa quando coberto"), GPU e loop de frames.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use windows::{
    Win32::Foundation::*, Win32::System::Console::SetConsoleCtrlHandler,
    Win32::System::LibraryLoader::GetModuleHandleW, Win32::System::Threading::*,
    Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON},
    Win32::UI::WindowsAndMessaging::*, core::*,
};

use crate::config::{EngineConfig, PresentMode};
use crate::desktop::{
    DesktopEventListener, MonitorInfo, MonitorManager, OcclusionDetector, SystemTray,
    WallpaperLayer, find_wallpaper_layer, repaint_desktop,
};
use crate::pause::{AutoPause, PauseClock, is_suspended, monitor_suspended};
use crate::render::pipeline::ShaderConstants;
use crate::render::{GpuContext, RenderSurface};
use crate::studio::{StudioCallbacks, StudioHandle};
use crate::wallpaper::WallpaperManager;

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

#[derive(Debug)]
pub enum EngineCommand {
    SetWallpaper(usize, String),
    SyncAllWallpapers(String),
    SetFps(u32),
    SetPauseCovered(bool),
    SetPauseBattery(bool),
    SetShowConsole(bool),
    SetPerformanceMode(crate::config::PerformanceMode),
    SetSpeed(f32),
    SetTint(String),
    SetMouseEnabled(bool),
    SetParallaxEnabled(bool),
    SetAutostart(bool),
    ImportWallpaper(String, usize),
    /// Importação de página/pacote que o usuário confirmou no Studio.
    ImportWallpaperConfirmed(String, usize),
    SetPlaylist(crate::config::PlaylistConfig),
    SetSchedule(crate::config::ScheduleConfig),
    SetScreensaver(crate::config::ScreensaverConfig),
    StartScreensaver,
    SetLockScreen(bool),
    /// Valor de um parâmetro `// param:` de um wallpaper: (spec, nome, valor).
    SetShaderParam(String, String, f32),
    /// Parâmetros de um wallpaper de volta aos padrões do shader.
    ResetShaderParams(String),
    /// Volta a configuração aos padrões (guarda um backup do `config.toml`).
    ResetConfig,
    /// Remove (Lixeira) um wallpaper da pasta de importados.
    RemoveWallpaper(String),
    SetWallpaperTitle(String, String),
    /// Tamanho dos dados que o usuário pode limpar.
    DataUsage,
    /// Limpa um tipo de dado do app.
    CleanData(String),
    /// Permissão de dados locais para um site remoto: (origem, tipo, valor).
    SetTourDone(bool),
    ImportShadertoy { title: String, code: String, monitor: usize },
    /// Renderiza um shader em alta resolução (4K) como PNG e aplica como imagem estática.
    FreezeWallpaper { spec: String, monitor: usize },
    /// Mensagem `ai_*` do Studio (JSON inteiro).
    Ai(serde_json::Value),
    TogglePause,
    /// Vindos do canal de controle (named pipe).
    SetPaused(bool),
    NextWallpaper,
    OpenStudio,
    /// `None` aplica a todos os monitores.
    ControlSetWallpaper(Option<usize>, String),
    /// Muda só os campos presentes da playlist, mantendo o resto.
    PatchPlaylist(crate::control::PlaylistPatch),
}

/// Converte `#RRGGBB` em cor de destaque (w = 1). Qualquer outro valor desativa o destaque.
pub fn parse_hex_color(hex: &str) -> [f32; 4] {
    let clean = hex.trim_start_matches('#');
    if clean.len() >= 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&clean[0..2], 16),
            u8::from_str_radix(&clean[2..4], 16),
            u8::from_str_radix(&clean[4..6], 16),
        ) {
            return [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
        }
    // "auto" (ou valor inválido): cada wallpaper usa as próprias cores.
    [0.0; 4]
}

pub struct Viewport {
    pub monitor: MonitorInfo,
    pub d3d_hwnd: HWND,
    pub web_hwnd: HWND,
    pub surface: RenderSurface,
}

pub struct Engine {
    config: EngineConfig,
    listener: DesktopEventListener,
    _tray: SystemTray,
    /// `None` se o WebView2 do Studio falhar: o motor continua rodando sem a UI.
    studio: Option<StudioHandle>,
    cmd_rx: std::sync::mpsc::Receiver<EngineCommand>,
    layer: WallpaperLayer,
    gpu: GpuContext,
    wallpapers: WallpaperManager,
    viewports: Vec<Viewport>,
    /// Modo desempenho ativo agora (ver `performance.rs`).
    perf_active: bool,
    last_perf_check: Instant,
    /// Wallpapers já refletidos no menu da bandeja (evita recalcular a cada tick).
    tray_wallpapers: Option<std::collections::HashMap<usize, String>>,
    surface_class: PCWSTR,
    start_time: Instant,
    last_reload_check: Instant,
    stats: EngineStats,
    /// Pausa manual (Studio ou bandeja).
    is_paused: bool,
    /// Pausa automática: desktop coberto ou na bateria. Independente da manual.
    auto_paused: bool,
    /// Pausa efetiva por monitor (id atual): manual, bateria ou tela coberta.
    suspended: std::collections::HashMap<usize, bool>,
    /// Há valores de parâmetros ainda não gravados no disco (gravados em lote, ver `flush_params`).
    params_dirty: bool,
    last_params_save: Instant,
    /// Relógio de animação de cada monitor, sem o tempo suspenso.
    clocks: std::collections::HashMap<usize, PauseClock>,
    last_power_check: Instant,
    on_battery: bool,
    speed: f32,
    tint_rgba: [f32; 4],
    mouse_enabled: bool,
    parallax_enabled: bool,
    audio_engine: crate::audio::AudioEngine,
    media: crate::media::MediaWatcher,
    system: crate::system::SystemWatcher,
    playlist: crate::playlist::Playlist,
    monitor_playlists: std::collections::HashMap<usize, crate::playlist::Playlist>,
    last_playlist_switch: Instant,
    last_schedule_applied: std::collections::HashMap<usize, (u16, String)>,
    screensaver: Option<crate::desktop::screensaver::ScreensaverOverlay>,
    last_screensaver_dismiss: Instant,
    transition: Option<Transition>,
    /// Espelho do wallpaper no Windows (Recorte etc.); `None` se desligado ou indisponível.
    system_sync: Option<crate::desktop::SystemWallpaperSync>,
    /// Quando capturar o próximo snapshot (dá tempo de páginas web renderizarem).
    snapshot_due: Option<Instant>,
    snapshot_renderer: Option<crate::render::offscreen::OffscreenRenderer>,
    /// Vigia das janelas dos monitores: última conferência e última recuperação.
    last_layer_check: Instant,
    last_layer_recovery: Option<Instant>,
    /// Canal de controle local (named pipe); `None` se não pôde ser criado.
    control: Option<crate::control::ControlServer>,
    /// Último estado publicado no canal, para só publicar quando algo muda.
    published: PublishedKey,
    /// "Criar com IA": conversa e geração em segundo plano.
    ai: crate::ai::Hub,
}

/// Campos do estado publicado, comparados a cada tick sem alocar.
struct PublishedKey {
    paused: bool,
    auto_paused: bool,
    settings: crate::control::Settings,
    wallpapers: std::collections::HashMap<usize, String>,
}

impl PublishedKey {
    fn empty() -> Self {
        Self {
            paused: false,
            auto_paused: false,
            settings: crate::control::Settings {
                fps: 0,
                speed: 0.0,
                pause_covered: false,
                pause_battery: false,
                mouse: false,
                parallax: false,
                playlist_enabled: false,
                playlist_minutes: 0,
                playlist_shuffle: false,
            },
            wallpapers: std::collections::HashMap::new(),
        }
    }
}

/// Fade em andamento: escurece, troca os wallpapers pendentes e clareia.
struct Transition {
    started: Instant,
    half: Duration,
    swapped: bool,
    pending: Vec<(usize, String)>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self> {
        info!("Inicializando IsNotAEngine...");

        let listener = DesktopEventListener::new()?;
        let _tray = SystemTray::new(listener.hwnd(), listener.events.is_paused.clone())?;
        let layer = find_wallpaper_layer()?;
        info!(
            layout = ?layer.layout,
            parent = ?layer.parent,
            parent_class = %crate::desktop::discovery::class_name(layer.parent),
            "Camada de wallpaper vinculada ao motor"
        );
        let gpu = GpuContext::new(config.present_mode)?;
        let surface_class = register_surface_class()?;

        let monitors = MonitorManager::enumerate_relative_to(layer.parent)?;

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let callbacks = StudioCallbacks {
            on_wallpaper_change: Box::new({
                let tx = cmd_tx.clone();
                move |id, path| {
                    let _ = tx.send(EngineCommand::SetWallpaper(id, path));
                }
            }),
            on_sync_all: Box::new({
                let tx = cmd_tx.clone();
                move |path| {
                    let _ = tx.send(EngineCommand::SyncAllWallpapers(path));
                }
            }),
            on_fps_change: Box::new({
                let tx = cmd_tx.clone();
                move |fps| {
                    let _ = tx.send(EngineCommand::SetFps(fps));
                }
            }),
            on_pause_covered_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetPauseCovered(val));
                }
            }),
            on_pause_battery_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetPauseBattery(val));
                }
            }),
            on_speed_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetSpeed(val));
                }
            }),
            on_tint_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetTint(val));
                }
            }),
            on_mouse_enabled_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetMouseEnabled(val));
                }
            }),
            on_parallax_enabled_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetParallaxEnabled(val));
                }
            }),
            on_toggle_pause: Box::new({
                let tx = cmd_tx.clone();
                move || {
                    let _ = tx.send(EngineCommand::TogglePause);
                }
            }),
            on_autostart_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetAutostart(val));
                }
            }),
            on_ai: Box::new({
                let tx = cmd_tx.clone();
                move |msg| {
                    let _ = tx.send(EngineCommand::Ai(msg));
                }
            }),
            on_freeze_wallpaper: Box::new({
                let tx = cmd_tx.clone();
                move |spec, monitor| {
                    let _ = tx.send(EngineCommand::FreezeWallpaper { spec, monitor });
                }
            }),
            on_import_shadertoy: Box::new({
                let tx = cmd_tx.clone();
                move |title, code, monitor| {
                    let _ = tx.send(EngineCommand::ImportShadertoy { title, code, monitor });
                }
            }),
            on_lock_screen_change: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetLockScreen(val));
                }
            }),
            on_shader_param: Box::new({
                let tx = cmd_tx.clone();
                move |spec, name, value| {
                    let _ = tx.send(EngineCommand::SetShaderParam(spec, name, value));
                }
            }),
            on_reset_shader_params: Box::new({
                let tx = cmd_tx.clone();
                move |spec| {
                    let _ = tx.send(EngineCommand::ResetShaderParams(spec));
                }
            }),
            on_remove_wallpaper: Box::new({
                let tx = cmd_tx.clone();
                move |path| {
                    let _ = tx.send(EngineCommand::RemoveWallpaper(path));
                }
            }),
            on_wallpaper_title: Box::new({
                let tx = cmd_tx.clone();
                move |path, title| {
                    let _ = tx.send(EngineCommand::SetWallpaperTitle(path, title));
                }
            }),
            on_data_usage: Box::new({
                let tx = cmd_tx.clone();
                move || {
                    let _ = tx.send(EngineCommand::DataUsage);
                }
            }),
            on_clean_data: Box::new({
                let tx = cmd_tx.clone();
                move |what| {
                    let _ = tx.send(EngineCommand::CleanData(what));
                }
            }),
            on_reset_config: Box::new({
                let tx = cmd_tx.clone();
                move || {
                    let _ = tx.send(EngineCommand::ResetConfig);
                }
            }),
            on_playlist_change: Box::new({
                let tx = cmd_tx.clone();
                move |cfg| {
                    let _ = tx.send(EngineCommand::SetPlaylist(cfg));
                }
            }),
            on_schedule_change: Box::new({
                let tx = cmd_tx.clone();
                move |cfg| {
                    let _ = tx.send(EngineCommand::SetSchedule(cfg));
                }
            }),
            on_screensaver_change: Box::new({
                let tx = cmd_tx.clone();
                move |cfg| {
                    let _ = tx.send(EngineCommand::SetScreensaver(cfg));
                }
            }),
            on_start_screensaver: Box::new({
                let tx = cmd_tx.clone();
                move || {
                    let _ = tx.send(EngineCommand::StartScreensaver);
                }
            }),
            on_performance_mode: Box::new({
                let tx = cmd_tx.clone();
                move |mode| {
                    let _ = tx.send(EngineCommand::SetPerformanceMode(mode));
                }
            }),
            on_show_console: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetShowConsole(val));
                }
            }),
            on_tour_done: Box::new({
                let tx = cmd_tx.clone();
                move |val| {
                    let _ = tx.send(EngineCommand::SetTourDone(val));
                }
            }),
            on_import_confirmed: Box::new({
                let tx = cmd_tx.clone();
                move |path, mon_id| {
                    let _ = tx.send(EngineCommand::ImportWallpaperConfirmed(path, mon_id));
                }
            }),
            on_import_file: Box::new({
                let tx = cmd_tx.clone();
                move |path, mon_id| {
                    let _ = tx.send(EngineCommand::ImportWallpaper(path, mon_id));
                }
            }),
        };

        let mut config = config;
        config.autostart = crate::desktop::is_autostart_enabled();

        let studio = match StudioHandle::spawn(config.clone(), monitors.clone(), callbacks) {
            Ok(studio) => {
                info!(studio_hwnd = ?studio.hwnd(), "IsNotAEngine Studio UI pronto e operacional");
                Some(studio)
            }
            Err(e) => {
                error!(error = %e, "Studio indisponível; o motor segue rodando sem a interface");
                None
            }
        };

        let wallpapers = WallpaperManager::new_empty(&gpu.device)?;

        let speed = config.speed;
        let tint_rgba = parse_hex_color(&config.tint);
        let mouse_enabled = config.mouse_enabled;
        let parallax_enabled = config.parallax_enabled;

        let mut engine = Self {
            config,
            listener,
            _tray,
            studio,
            cmd_rx,
            layer,
            gpu,
            wallpapers,
            viewports: Vec::new(),
            tray_wallpapers: None,
            perf_active: false,
            last_perf_check: Instant::now() - Duration::from_secs(10),
            surface_class,
            start_time: Instant::now(),
            last_reload_check: Instant::now(),
            stats: EngineStats::new(),
            is_paused: false,
            auto_paused: false,
            suspended: std::collections::HashMap::new(),
            params_dirty: false,
            last_params_save: Instant::now(),
            clocks: std::collections::HashMap::new(),
            last_power_check: Instant::now() - Duration::from_secs(10),
            on_battery: false,
            speed,
            tint_rgba,
            mouse_enabled,
            parallax_enabled,
            audio_engine: crate::audio::AudioEngine::new(),
            media: crate::media::MediaWatcher::start(),
            system: crate::system::SystemWatcher::start(),
            playlist: crate::playlist::Playlist::new(Vec::new(), false),
            monitor_playlists: std::collections::HashMap::new(),
            last_playlist_switch: Instant::now(),
            last_schedule_applied: std::collections::HashMap::new(),
            screensaver: None,
            last_screensaver_dismiss: Instant::now() - Duration::from_secs(10),
            transition: None,
            system_sync: None,
            snapshot_due: None,
            snapshot_renderer: None,
            last_layer_check: Instant::now(),
            last_layer_recovery: None,
            control: None,
            published: PublishedKey::empty(),
            ai: crate::ai::Hub::default(),
        };
        engine.rebuild_playlist();
        if engine.config.mirror_system_wallpaper {
            engine.system_sync = crate::desktop::SystemWallpaperSync::new();
            engine.schedule_snapshot();
        }

        engine.setup_viewports()?;

        // Canal de controle depois dos viewports, para o primeiro estado já ter os wallpapers.
        engine.published = engine.current_published_key();
        let initial = engine.published_snapshot();
        engine.control = crate::control::ControlServer::start(cmd_tx, &initial);
        Ok(engine)
    }


    /// Cria as janelas filhas e superfícies DXGI para cada monitor.
    fn setup_viewports(&mut self) -> Result<()> {
        self.teardown_viewports();

        let monitors = MonitorManager::enumerate_relative_to(self.layer.parent)?;
        // Valores de parâmetros do usuário: o gerenciador os aplica a cada wallpaper que carregar.
        for spec in self.config.shader_params.keys().cloned().collect::<Vec<_>>() {
            let o = self.config.shader_param_overrides(&spec);
            self.wallpapers.set_shader_params(&spec, o);
        }
        // Liga as atribuições salvas à identidade de cada tela, não ao índice desta enumeração.
        let keys: Vec<String> = monitors.iter().map(|m| m.key.clone()).collect();
        self.config.resolve_monitors(&keys);
        info!(
            total_monitors = monitors.len(),
            "Configurando viewports por monitor (D3D11 + WebView2 dual-surface)"
        );

        for m in monitors {
            let d3d_hwnd = create_child_window(
                self.surface_class,
                self.layer.parent,
                self.config.present_mode,
                m.client_x,
                m.client_y,
                m.width as i32,
                m.height as i32,
            )?;

            let web_hwnd = create_web_child_window(
                self.surface_class,
                self.layer.parent,
                m.client_x,
                m.client_y,
                m.width as i32,
                m.height as i32,
            )?;

            let surface = self
                .gpu
                .create_surface(d3d_hwnd, m.width, m.height)
                .map_err(|e| {
                    Error::new(
                        e.code(),
                        format!(
                            "Falha ao criar swapchain para monitor {}: {}",
                            m.id,
                            e.message()
                        ),
                    )
                })?;

            self.wallpapers.register_monitor(m.id, d3d_hwnd, web_hwnd, m.width, m.height);

            self.viewports.push(Viewport {
                monitor: m,
                d3d_hwnd,
                web_hwnd,
                surface,
            });
        }

        // Aplica os papéis de parede configurados agora que as janelas nativas foram criadas
        let default_wallpapers = ["matrix", "coding"];

        for v in &self.viewports {
            let wp_path = self
                .config
                .monitor_wallpapers
                .get(&v.monitor.id)
                .map(|s| s.as_str())
                .filter(|s| wallpaper_spec_available(s))
                .unwrap_or(default_wallpapers[v.monitor.id % default_wallpapers.len()]);

            let _ = self.wallpapers.set_monitor_wallpaper(&self.gpu.device, v.monitor.id, wp_path);
        }

        Ok(())
    }

    /// As janelas de todos os monitores ainda existem?
    fn viewports_alive(&self) -> bool {
        // SAFETY: IsWindow só consulta o handle; aceita qualquer valor, inclusive já destruído.
        self.viewports.iter().all(|v| unsafe { IsWindow(Some(v.d3d_hwnd)).as_bool() && IsWindow(Some(v.web_hwnd)).as_bool() })
    }

    fn teardown_viewports(&mut self) {
        for v in self.viewports.drain(..) {
            drop(v.surface);
            unsafe {
                let _ = DestroyWindow(v.d3d_hwnd);
                let _ = DestroyWindow(v.web_hwnd);
            }
        }
    }

    /// Executa o loop principal de frames com timing de alta precisão.
    pub fn run(&mut self) -> Result<()> {
        unsafe {
            MAIN_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            SetConsoleCtrlHandler(Some(ctrl_handler), true)?;
        }
        crate::console::set_visible(self.config.show_console);

        info!(
            target_fps = self.config.target_fps,
            "Loop de renderização iniciado. Ctrl+C para sair."
        );

        unsafe {
            let timer = CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )?;

            let mut current_fps = self.config.target_fps;
            let mut frame_duration_100ns = -(10_000_000i64 / current_fps as i64);
            SetWaitableTimer(timer, &frame_duration_100ns, 0, None, None, false)?;

            let mut msg = MSG::default();

            'main_loop: loop {
                // Processa todas as mensagens Win32 da thread (incluindo WebView2 e Studio)
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        info!("Mensagem de encerramento recebida.");
                        break 'main_loop;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                let wait = MsgWaitForMultipleObjects(Some(&[timer]), false, INFINITE, QS_ALLINPUT);

                if wait == WAIT_OBJECT_0 {
                    // Timer disparou: hora do próximo frame
                    if self.config.target_fps != current_fps {
                        current_fps = self.config.target_fps;
                        frame_duration_100ns = -(10_000_000i64 / current_fps as i64);
                    }
                    // Modo desempenho: acorda só uma vez por segundo.
                    let period = if self.perf_active { -10_000_000i64 } else { frame_duration_100ns };
                    SetWaitableTimer(timer, &period, 0, None, None, false)?;

                    // 1. Processa eventos de sistema (Explorer reiniciado, mudança de display, energia, Studio IPC)
                    self.handle_system_events()?;

                    // 1.2 Proteção de tela viva (por ociosidade ou comando explícito)
                    self.tick_screensaver();
                    if self.screensaver.is_some() {
                        self.tick_schedule();
                        self.tick_playlist();
                        self.render_screensaver_viewports()?;
                        self.stats.record_frame();
                        continue;
                    }

                    // 1.5 Modo desempenho (jogo em tela cheia ou escolha do usuário)
                    self.update_performance();
                    if self.listener.events.toggle_performance_requested.swap(false, Ordering::SeqCst) {
                        let next = if self.config.performance_mode == crate::config::PerformanceMode::Always {
                            crate::config::PerformanceMode::Auto
                        } else {
                            crate::config::PerformanceMode::Always
                        };
                        self.set_performance_mode(next);
                    }

                    // 2. Pausa automática: desktop coberto e/ou notebook na bateria
                    let covered: Vec<bool> = if self.config.pause_when_covered && !self.perf_active {
                        let rects: Vec<RECT> = self.viewports.iter().map(|v| v.monitor.screen_rect).collect();
                        OcclusionDetector::occluded_monitors(&rects)
                    } else {
                        vec![false; self.viewports.len()]
                    };
                    let all_covered = !covered.is_empty() && covered.iter().all(|&c| c);
                    if self.last_power_check.elapsed() >= Duration::from_secs(2) {
                        self.on_battery = running_on_battery();
                        self.last_power_check = Instant::now();
                    }
                    let auto = AutoPause::evaluate(
                        self.config.pause_when_covered,
                        all_covered,
                        self.config.pause_on_battery,
                        self.on_battery,
                    );
                    if auto.active() != self.auto_paused {
                        self.auto_paused = auto.active();
                        info!(covered = auto.covered, battery = auto.battery, paused = auto.active(), "Pausa automática alterada");
                        self.apply_pause_state();
                    }
                    // Pausa por monitor: a tela coberta para; as descobertas seguem.
                    self.sync_monitor_suspension(&covered, auto.battery || self.perf_active);
                    let all_suspended = !self.viewports.is_empty()
                        && self.viewports.iter().all(|v| self.suspended.get(&v.monitor.id).copied().unwrap_or(false));
                    if all_suspended || is_suspended(self.is_paused, self.auto_paused) {
                        // Tudo pausado: pula o render para zerar o consumo de GPU
                        continue;
                    }

                    // 3. Hot-reload de shaders a cada 1 segundo (não bloqueante)
                    if self.last_reload_check.elapsed() >= Duration::from_secs(1) {
                        self.wallpapers.check_reloads(&self.gpu.device);
                        self.last_reload_check = Instant::now();
                    }

                    self.flush_params();
                    self.tick_snapshot();

                    // 3.5 Agenda por horário e Playlist
                    self.tick_schedule();
                    self.tick_playlist();

                    // 4. Renderiza viewports de cada monitor
                    self.render_all_viewports()?;
                    self.stats.record_frame();
                }
            }

            let _ = CloseHandle(timer);
        }

        self.cleanup();
        Ok(())
    }

    fn handle_system_events(&mut self) -> Result<()> {
        const DEBOUNCE_DELAY: Duration = Duration::from_millis(200);

        // 0. Comandos recebidos do IsNotAEngine Studio
        let mut wallpaper_changed = false;
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                EngineCommand::SetWallpaper(id, path) => {
                    info!(monitor = id, path = %path, "Atualizando papel de parede via Studio");
                    match self.wallpapers.set_monitor_wallpaper(&self.gpu.device, id, &path) {
                        Ok(()) => {
                            info!(monitor = id, path = %path, "Papel de parede aplicado com sucesso!");
                            self.config.monitor_wallpapers.insert(id, path.clone());
                            let _ = self.config.save();
                            self.notify_applied(Some(id), &path);
                        }
                        Err(e) => {
                            // Não grava o spec ruim: o que estava no ar continua e o Studio é avisado.
                            error!(monitor = id, path = %path, error = %e, "Erro ao aplicar papel de parede");
                            self.notify_apply_error(&[id], &path, &e.message());
                        }
                    }
                    wallpaper_changed = true;
                }
                EngineCommand::SyncAllWallpapers(path) => {
                    self.apply_to_all(&path);
                    wallpaper_changed = true;
                }
                EngineCommand::SetFps(fps) => {
                    info!(fps = fps, "Taxa de quadros (FPS) alterada via Studio");
                    self.config.target_fps = fps;
                    let _ = self.config.save();
                }
                EngineCommand::SetPauseCovered(val) => {
                    info!(enabled = val, "Opção 'Pausar quando coberto' alterada via Studio");
                    self.config.pause_when_covered = val;
                    let _ = self.config.save();
                }
                EngineCommand::SetPauseBattery(val) => {
                    info!(enabled = val, "Opção 'Pausar na bateria' alterada via Studio");
                    self.config.pause_on_battery = val;
                    let _ = self.config.save();
                }
                EngineCommand::SetSpeed(val) => {
                    info!(speed = val, "Velocidade alterada via Studio");
                    self.speed = val;
                    self.config.speed = val;
                    let _ = self.config.save();
                }
                EngineCommand::SetTint(hex) => {
                    info!(tint = %hex, "Matiz/Cor alterada via Studio");
                    self.tint_rgba = parse_hex_color(&hex);
                    self.config.tint = hex;
                    let _ = self.config.save();
                }
                EngineCommand::SetMouseEnabled(val) => {
                    info!(enabled = val, "Interatividade do mouse alterada via Studio");
                    self.mouse_enabled = val;
                    self.config.mouse_enabled = val;
                    let _ = self.config.save();
                }
                EngineCommand::SetParallaxEnabled(val) => {
                    info!(enabled = val, "Parallax 3D alterado via Studio");
                    self.parallax_enabled = val;
                    self.config.parallax_enabled = val;
                    let _ = self.config.save();
                }
                EngineCommand::SetAutostart(val) => {
                    info!(enabled = val, "Inicialização automática com o Windows alterada via Studio");
                    match crate::desktop::set_autostart_enabled(val) {
                        Ok(()) => info!(enabled = val, "Registro do Windows configurado com sucesso para inicialização automática"),
                        Err(e) => error!(error = %e, "Falha ao gravar chave de inicialização no Registro do Windows"),
                    }
                    self.config.autostart = val;
                    let _ = self.config.save();
                }
                EngineCommand::ImportWallpaper(path, mon_id) => {
                    // Página HTML, pasta ou pacote roda código no desktop: o Studio confirma antes.
                    if crate::import::runs_page_code(std::path::Path::new(&path)) && self.studio.is_some() {
                        info!(path = %path, "Importação de página aguardando confirmação no Studio");
                        self.ask_import_confirmation(&path, mon_id);
                    } else {
                        info!(monitor = mon_id, path = %path, "Importando arquivo de wallpaper via Studio");
                        self.import_and_apply_wallpaper(mon_id, &path);
                        wallpaper_changed = true;
                    }
                }
                EngineCommand::ImportWallpaperConfirmed(path, mon_id) => {
                    info!(monitor = mon_id, path = %path, "Importando página confirmada no Studio");
                    self.import_and_apply_wallpaper(mon_id, &path);
                    wallpaper_changed = true;
                }
                EngineCommand::Ai(msg) => self.handle_ai(&msg),
                EngineCommand::FreezeWallpaper { spec, monitor } => {
                    if self.freeze_wallpaper(&spec, monitor) {
                        wallpaper_changed = true;
                    }
                }
                EngineCommand::ImportShadertoy { title, code, monitor } => {
                    match crate::shadertoy::convert(&code, &title) {
                        Ok(c) => {
                            let stem = crate::shadertoy::file_stem(&title);
                            self.import_converted_shader(monitor, &stem, &c.hlsl, &c.warnings);
                            wallpaper_changed = true;
                        }
                        Err(msg) => self.notify_import_error(&msg),
                    }
                }
                EngineCommand::SetLockScreen(val) => {
                    info!(enabled = val, "Wallpaper na tela de bloqueio alterado via Studio");
                    self.config.mirror_lock_screen = val;
                    let _ = self.config.save();
                    if val {
                        self.schedule_snapshot();
                    }
                }
                EngineCommand::SetShaderParam(spec, name, value) => {
                    if self.config.set_shader_param(&spec, &name, value) {
                        let o = self.config.shader_param_overrides(&spec);
                        self.wallpapers.set_shader_params(&spec, o);
                        // Sem salvar a cada passo do controle deslizante: o Studio manda dezenas por segundo.
                        self.params_dirty = true;
                    } else {
                        warn!(%spec, %name, value, "set_shader_param inválido ignorado");
                    }
                }
                EngineCommand::ResetShaderParams(spec) => {
                    self.config.reset_shader_params(&spec);
                    self.wallpapers.set_shader_params(&spec, Vec::new());
                    let _ = self.config.save();
                }
                EngineCommand::RemoveWallpaper(path) => {
                    self.remove_wallpaper(&path);
                    wallpaper_changed = true;
                }
                EngineCommand::SetWallpaperTitle(path, title) => self.set_wallpaper_title(&path, &title),
                EngineCommand::DataUsage => self.notify_data_usage(),
                EngineCommand::CleanData(what) => self.clean_data(&what),
                EngineCommand::ResetConfig => {
                    self.reset_config();
                    wallpaper_changed = true;
                }
                EngineCommand::SetPerformanceMode(mode) => self.set_performance_mode(mode),
                EngineCommand::SetShowConsole(val) => {
                    info!(enabled = val, "Console de log alterado via Studio");
                    self.config.show_console = val;
                    crate::console::set_visible(val);
                    let _ = self.config.save();
                }
                EngineCommand::SetTourDone(val) => {
                    self.config.tour_done = val;
                    let _ = self.config.save();
                }
                EngineCommand::SetPlaylist(cfg) => {
                    let cfg = cfg.sanitized();
                    info!(enabled = cfg.enabled, minutes = cfg.interval_minutes, shuffle = cfg.shuffle, "Playlist alterada via Studio");
                    self.config.playlist = cfg;
                    let _ = self.config.save();
                    self.rebuild_playlist();
                }
                EngineCommand::SetSchedule(cfg) => {
                    let cfg = cfg.sanitized();
                    info!(enabled = cfg.enabled, slots = cfg.slots.len(), "Agenda por horário alterada via Studio");
                    self.config.schedule = cfg;
                    self.last_schedule_applied.clear();
                    let _ = self.config.save();
                    self.tick_schedule();
                }
                EngineCommand::SetScreensaver(cfg) => {
                    let cfg = cfg.sanitized();
                    info!(enabled = cfg.enabled, idle_minutes = cfg.idle_minutes, wallpaper = %cfg.wallpaper, "Proteção de tela viva alterada via Studio");
                    self.config.screensaver = cfg;
                    let _ = self.config.save();
                }
                EngineCommand::StartScreensaver => {
                    self.start_screensaver(false);
                }
                EngineCommand::TogglePause => {
                    self.is_paused = !self.is_paused;
                    self.apply_pause_state();
                    info!(paused = self.is_paused, "Pausar/Retomar alternado via Studio");
                }
                EngineCommand::SetPaused(val) => {
                    self.is_paused = val;
                    self.apply_pause_state();
                    info!(paused = val, "Pausa definida via canal de controle");
                }
                EngineCommand::NextWallpaper => {
                    info!("Próximo wallpaper solicitado via canal de controle");
                    self.start_playlist_switch();
                }
                EngineCommand::OpenStudio => {
                    if let Some(studio) = &self.studio {
                        studio.show();
                    }
                }
                EngineCommand::PatchPlaylist(patch) => {
                    let mut cfg = self.config.playlist.clone();
                    if let Some(enabled) = patch.enabled {
                        cfg.enabled = enabled;
                    }
                    if let Some(minutes) = patch.minutes {
                        cfg.interval_minutes = minutes;
                    }
                    if let Some(shuffle) = patch.shuffle {
                        cfg.shuffle = shuffle;
                    }
                    let cfg = cfg.sanitized();
                    info!(enabled = cfg.enabled, minutes = cfg.interval_minutes, shuffle = cfg.shuffle, "Playlist alterada via canal de controle");
                    self.config.playlist = cfg;
                    let _ = self.config.save();
                    self.rebuild_playlist();
                }
                EngineCommand::ControlSetWallpaper(monitor, spec) => {
                    // O pipe é só do usuário, mas valida igual à bandeja: id do catálogo, arquivo ou URL.
                    if !wallpaper_spec_available(&spec) {
                        warn!(wallpaper = %spec, "Canal de controle: wallpaper inexistente; ignorado");
                        continue;
                    }
                    let ids: Vec<usize> = match monitor {
                        Some(id) if self.viewports.iter().any(|v| v.monitor.id == id) => vec![id],
                        Some(id) => {
                            warn!(monitor = id, "Canal de controle: monitor inexistente; ignorado");
                            continue;
                        }
                        None => self.viewports.iter().map(|v| v.monitor.id).collect(),
                    };
                    for id in ids {
                        match self.wallpapers.set_monitor_wallpaper(&self.gpu.device, id, &spec) {
                            Ok(()) => info!(monitor = id, wallpaper = %spec, "Wallpaper aplicado via canal de controle"),
                            Err(e) => error!(monitor = id, error = %e, "Erro ao aplicar wallpaper via canal de controle"),
                        }
                        self.config.monitor_wallpapers.insert(id, spec.clone());
                    }
                    let _ = self.config.save();
                    wallpaper_changed = true;
                }
            }
        }

        // 1. Encerramento solicitado pela bandeja
        if self.listener.events.exit_requested.load(Ordering::SeqCst) {
            info!("Encerramento solicitado pelo menu da bandeja");
            unsafe {
                PostQuitMessage(0);
            }
        }

        // 2. Pausar / Retomar solicitado pela bandeja
        if self
            .listener
            .events
            .toggle_pause_requested
            .swap(false, Ordering::SeqCst)
        {
            self.is_paused = !self.is_paused;
            self.apply_pause_state();
            info!(
                paused = self.is_paused,
                "Estado de reprodução alternado via bandeja"
            );
        }

        // 2.5 "Próximo wallpaper" na bandeja: avança a playlist agora
        if self.listener.events.next_wallpaper_requested.swap(false, Ordering::SeqCst) {
            info!("Próximo wallpaper solicitado pela bandeja");
            self.start_playlist_switch();
        }

        // 3. Troca de wallpaper solicitada pela bandeja
        let new_wp = self
            .listener
            .events
            .switch_wallpaper_requested
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());

        if let Some(wp_path) = new_wp {
            info!(
                path = %wp_path,
                "Alternando papel de parede para todos os monitores via bandeja"
            );
            self.apply_to_all(&wp_path);
            self.schedule_snapshot();
        }

        // 4. Abertura do Studio solicitada pela bandeja
        if self
            .listener
            .events
            .open_studio_requested
            .swap(false, Ordering::SeqCst)
        {
            info!("Abrindo IsNotAEngine Studio...");
            if let Some(studio) = &self.studio {
                studio.show();
            }
        }

        // 5. Se o Explorer reiniciou, aplica debounce para absorver rajadas do shell/docks
        if self
            .listener
            .events
            .explorer_restarted
            .load(Ordering::SeqCst)
        {
            let ready = if let Ok(lock) = self.listener.events.last_restart_signal.lock() {
                lock.is_none_or(|t| t.elapsed() >= DEBOUNCE_DELAY)
            } else {
                true
            };

            if ready {
                self.listener
                    .events
                    .explorer_restarted
                    .store(false, Ordering::SeqCst);
                warn!("Recuperando camada de wallpaper após reinício do Explorer (debounced)...");
                self._tray.add();
                self.teardown_viewports();
                self.layer = find_wallpaper_layer()?;
                self.setup_viewports()?;
                let monitors = MonitorManager::enumerate_relative_to(self.layer.parent)?;
                if let Some(studio) = &self.studio {
                    studio.update_monitors(monitors);
                }
                info!("Camada de wallpaper e viewports restaurados com sucesso!");
            }
        }

        // 5b. Vigia: se as janelas dos monitores foram destruídas por fora (ex.: o Explorer recria a camada do
        // desktop quando o wallpaper do Windows muda), o motor seguiria "renderizando" em janelas mortas.
        // Reaproveita a recuperação do reinício do Explorer, no máximo uma vez a cada 10 s.
        if self.last_layer_check.elapsed() >= Duration::from_secs(1) {
            self.last_layer_check = Instant::now();
            let throttled = self.last_layer_recovery.is_some_and(|t| t.elapsed() < Duration::from_secs(10));
            if !throttled && !self.viewports.is_empty() && !self.viewports_alive() {
                warn!("Janelas dos monitores foram destruídas por fora; recuperando a camada de wallpaper...");
                self.last_layer_recovery = Some(Instant::now());
                self.teardown_viewports();
                match find_wallpaper_layer() {
                    Ok(layer) => {
                        self.layer = layer;
                        if let Err(e) = self.setup_viewports() {
                            error!(error = %e, "Falha ao recriar os viewports; nova tentativa em 10 s");
                        } else {
                            if let (Some(studio), Ok(monitors)) = (&self.studio, MonitorManager::enumerate_relative_to(self.layer.parent)) {
                                studio.update_monitors(monitors);
                            }
                            info!("Camada de wallpaper recuperada após a destruição das janelas");
                        }
                    }
                    Err(e) => error!(error = %e, "Camada de wallpaper indisponível; nova tentativa em 10 s"),
                }
            }
        }

        // 6. Se a resolução ou os monitores mudaram (com debounce)
        if self.listener.events.display_changed.load(Ordering::SeqCst) {
            let ready = if let Ok(lock) = self.listener.events.last_display_signal.lock() {
                lock.is_none_or(|t| t.elapsed() >= DEBOUNCE_DELAY)
            } else {
                true
            };

            if ready {
                self.listener
                    .events
                    .display_changed
                    .store(false, Ordering::SeqCst);
                info!("Recalculando geometria dos monitores após WM_DISPLAYCHANGE (debounced)...");
                self.setup_viewports()?;
                let monitors = MonitorManager::enumerate_relative_to(self.layer.parent)?;
                if let Some(studio) = &self.studio {
                    studio.update_monitors(monitors);
                }
            }
        }

        if wallpaper_changed {
            self.schedule_snapshot();
            let _ = self.render_all_viewports();
        }

        if self.tray_wallpapers.as_ref() != Some(&self.config.monitor_wallpapers) {
            self.refresh_tray_info();
            self.tray_wallpapers = Some(self.config.monitor_wallpapers.clone());
        }
        self.publish_state_if_changed();
        Ok(())
    }

    fn current_settings(&self) -> crate::control::Settings {
        crate::control::Settings {
            fps: self.config.target_fps,
            speed: self.speed,
            pause_covered: self.config.pause_when_covered,
            pause_battery: self.config.pause_on_battery,
            mouse: self.mouse_enabled,
            parallax: self.parallax_enabled,
            playlist_enabled: self.config.playlist.enabled,
            playlist_minutes: self.config.playlist.interval_minutes,
            playlist_shuffle: self.config.playlist.shuffle,
        }
    }

    fn current_published_key(&self) -> PublishedKey {
        PublishedKey {
            paused: self.is_paused,
            auto_paused: self.auto_paused,
            settings: self.current_settings(),
            wallpapers: self.config.monitor_wallpapers.clone(),
        }
    }

    fn published_snapshot(&self) -> crate::control::StateSnapshot {
        let k = &self.published;
        crate::control::StateSnapshot::new(k.paused, k.auto_paused, k.settings, &k.wallpapers)
    }

    /// Empurra o estado aos clientes do canal de controle só quando algo mudou (sem alocar se igual).
    fn publish_state_if_changed(&mut self) {
        let Some(control) = &self.control else { return };
        let p = &self.published;
        let unchanged = p.paused == self.is_paused
            && p.auto_paused == self.auto_paused
            && p.settings == self.current_settings()
            && p.wallpapers == self.config.monitor_wallpapers;
        if unchanged {
            return;
        }
        self.published = self.current_published_key();
        control.publish(&self.published_snapshot());
    }

    /// Propaga a pausa manual para a bandeja. Os wallpapers são atualizados por monitor em
    /// [`Self::sync_monitor_suspension`], que roda a cada tick (logo após qualquer mudança).
    fn apply_pause_state(&self) {
        // A bandeja mostra só a pausa manual ("Pausar"/"Retomar" é sobre a escolha do usuário).
        self.listener.events.is_paused.store(self.is_paused, Ordering::SeqCst);
    }

    /// Recalcula a pausa de cada monitor (manual, bateria ou coberto) e aplica as mudanças.
    fn sync_monitor_suspension(&mut self, covered: &[bool], battery: bool) {
        let now = Instant::now();
        let ids: Vec<usize> = self.viewports.iter().map(|v| v.monitor.id).collect();
        for (id, &cov) in ids.into_iter().zip(covered) {
            let suspended = monitor_suspended(self.is_paused, battery, cov);
            if self.suspended.insert(id, suspended).unwrap_or(false) != suspended {
                debug!(monitor = id, suspended, "Pausa do monitor alterada");
                self.wallpapers.set_monitor_paused(id, suspended);
            }
            self.clocks.entry(id).or_default().set_suspended(suspended, now);
        }
    }

    fn render_all_viewports(&mut self) -> Result<()> {
        let (cursor_pos, lbutton, rbutton) = get_mouse_state();
        let now = Instant::now();
        let (clock_time, clock_date) = system_clock();
        let audio_snap = self.audio_engine.snapshot();
        let media = self.media.snapshot();
        let system = self.system.snapshot();
        let fade = self.fade_amount();
        let mut needs_device_recovery = false;

        for v in &self.viewports {
            // Tela suspensa (coberta, bateria ou pausa manual): sem render nem ponte de dados.
            if self.suspended.get(&v.monitor.id).copied().unwrap_or(false) {
                continue;
            }
            let time = self.clocks.entry(v.monitor.id).or_default().elapsed(self.start_time, now);
            if self.wallpapers.is_web_monitor(v.monitor.id) {
                // Para wallpaper Web, injeta telemetria de áudio WASAPI em tempo real (0% D3D11 GPU)
                self.wallpapers.update_web_audio(v.monitor.id, &audio_snap);
                self.wallpapers.update_web_media(v.monitor.id, &media);
                self.wallpapers.update_web_system(v.monitor.id, &system);
                self.wallpapers.update_web_tint(v.monitor.id, self.tint_rgba);
                self.wallpapers.update_web_fps(v.monitor.id, self.config.target_fps);
                continue;
            }

            let mx = (cursor_pos.x - v.monitor.screen_rect.left) as f32
                / v.monitor.width.max(1) as f32;
            let my = 1.0 - ((cursor_pos.y - v.monitor.screen_rect.top) as f32
                / v.monitor.height.max(1) as f32);

            let constants = ShaderConstants {
                resolution: [v.monitor.width as f32, v.monitor.height as f32],
                time,
                speed: self.speed,
                tint: self.tint_rgba,
                mouse: [mx, my],
                mouse_click: [
                    if lbutton { 1.0 } else { 0.0 },
                    if rbutton { 1.0 } else { 0.0 },
                ],
                mouse_enabled: if self.mouse_enabled { 1.0 } else { 0.0 },
                parallax_enabled: if self.parallax_enabled { 1.0 } else { 0.0 },
                tint_ref: [0.0, 0.0],
                clock_time,
                clock_date,
                audio_reactive: audio_snap.reactive,
                audio_spectrum: audio_snap.spectrum,
                params: [0.0; 8],
                fade: [fade, 0.0, 0.0, 0.0],
            };

            if let Err(e) = self.wallpapers.render_monitor(
                v.monitor.id,
                &self.gpu.context,
                &v.surface,
                constants,
            ) {
                if self.gpu.is_device_lost(&Err(e.clone())) {
                    warn!("Perda de dispositivo GPU detectada durante o render!");
                    needs_device_recovery = true;
                    break;
                } else {
                    error!(monitor = v.monitor.id, error = %e, "Erro ao renderizar viewport");
                }
            }
        }

        if needs_device_recovery {
            self.recover_gpu()?;
        }

        Ok(())
    }

    /// Monta a playlist global e as filas por monitor a partir da config (lista vazia = shaders do catálogo).
    fn rebuild_playlist(&mut self) {
        use crate::wallpaper::catalog::{CATALOG, Kind};
        let cfg = &self.config.playlist;
        let items: Vec<String> = if cfg.items.is_empty() {
            CATALOG.iter().filter(|e| e.kind == Kind::Shader).map(|e| e.id.to_string()).collect()
        } else {
            cfg.items.iter().filter(|i| wallpaper_spec_available(i)).cloned().collect()
        };
        self.playlist = crate::playlist::Playlist::new(items, cfg.shuffle);
        self.monitor_playlists.clear();
        for (&mon_id, list) in &cfg.monitor_items {
            let valid: Vec<String> = list.iter().filter(|i| wallpaper_spec_available(i)).cloned().collect();
            if !valid.is_empty() {
                self.monitor_playlists.insert(mon_id, crate::playlist::Playlist::new(valid, cfg.shuffle));
            }
        }
        self.last_playlist_switch = Instant::now();
    }

    fn fade_amount(&self) -> f32 {
        self.transition
            .as_ref()
            .map_or(0.0, |t| crate::playlist::fade_amount(t.started.elapsed(), t.half, t.swapped))
    }

    /// Avança a transição em curso ou, vencido o intervalo, inicia a próxima troca.
    fn tick_playlist(&mut self) {
        if let Some(t) = &mut self.transition {
            if t.started.elapsed() < t.half {
                return;
            }
            if t.swapped {
                self.transition = None;
                return;
            }
            t.swapped = true;
            t.started = Instant::now();
            let pending = std::mem::take(&mut t.pending);
            for (id, spec) in pending {
                self.apply_playlist_item(id, spec);
            }
            return;
        }

        let cfg = &self.config.playlist;
        if !cfg.enabled || (self.playlist.is_empty() && self.monitor_playlists.is_empty()) {
            return;
        }
        let interval = Duration::from_secs(u64::from(cfg.interval_minutes.max(1)) * 60);
        if self.last_playlist_switch.elapsed() < interval {
            return;
        }
        self.start_playlist_switch();
    }

    /// Escolhe o próximo item de cada monitor (usando a fila própria do monitor, se configurada, ou a global) e troca.
    fn start_playlist_switch(&mut self) {
        if self.transition.is_some() || (self.playlist.is_empty() && self.monitor_playlists.is_empty()) {
            return;
        }
        self.last_playlist_switch = Instant::now();

        let cfg = &self.config.playlist;
        let half = Duration::from_millis(u64::from(cfg.transition_ms) / 2);
        let mut pending = Vec::new();
        for v in &self.viewports {
            let id = v.monitor.id;
            let current = self.config.monitor_wallpapers.get(&id).map(String::as_str);
            let next = if let Some(mp) = self.monitor_playlists.get_mut(&id) {
                mp.next(current)
            } else {
                self.playlist.next(current)
            };
            if let Some(next) = next {
                pending.push((id, next));
            }
        }
        self.queue_or_apply_wallpapers(pending, half);
    }

    fn queue_or_apply_wallpapers(&mut self, pending: Vec<(usize, String)>, half: Duration) {
        if pending.is_empty() {
            return;
        }
        let can_fade = !half.is_zero() && self.viewports.iter().all(|v| self.wallpapers.supports_fade(v.monitor.id));
        if can_fade && self.transition.is_none() {
            self.transition = Some(Transition { started: Instant::now(), half, swapped: false, pending });
        } else {
            for (id, spec) in pending {
                self.apply_playlist_item(id, spec);
            }
        }
    }

    /// Avalia a agenda por horário do dia e aplica o wallpaper agendado quando o período muda.
    fn tick_schedule(&mut self) {
        if !self.config.schedule.enabled || self.config.schedule.slots.is_empty() || self.transition.is_some() {
            return;
        }
        let (clock_time, _) = system_clock();
        let now_minutes = (clock_time[0] as u16) * 60 + (clock_time[1] as u16);
        let mut pending = Vec::new();
        for v in &self.viewports {
            let id = v.monitor.id;
            if let Some((start_min, spec)) = crate::playlist::active_schedule_slot(&self.config.schedule.slots, id, now_minutes)
                && wallpaper_spec_available(spec)
            {
                let key = (start_min, spec.to_string());
                if self.last_schedule_applied.get(&id) != Some(&key) {
                    self.last_schedule_applied.insert(id, key);
                    if self.config.monitor_wallpapers.get(&id).map(String::as_str) != Some(spec) {
                        pending.push((id, spec.to_string()));
                    }
                }
            }
        }
        if !pending.is_empty() {
            let half = Duration::from_millis(u64::from(self.config.playlist.transition_ms) / 2);
            self.queue_or_apply_wallpapers(pending, half);
        }
    }

    /// Inicia a proteção de tela viva em tela cheia sobre todos os monitores.
    pub fn start_screensaver(&mut self, exit_on_dismiss: bool) {
        if self.screensaver.is_some() {
            return;
        }
        let monitors: Vec<MonitorInfo> = self.viewports.iter().map(|v| v.monitor.clone()).collect();
        if monitors.is_empty() {
            return;
        }
        match crate::desktop::screensaver::ScreensaverOverlay::activate(
            &self.gpu,
            &monitors,
            &self.config.screensaver.wallpaper,
            exit_on_dismiss,
        ) {
            Ok(overlay) => {
                self.screensaver = Some(overlay);
            }
            Err(e) => {
                error!(error = %e, "Falha ao ativar proteção de tela viva");
            }
        }
    }

    fn tick_screensaver(&mut self) {
        if self.listener.events.start_screensaver_requested.swap(false, Ordering::SeqCst) {
            self.start_screensaver(false);
            return;
        }

        if let Some(ss) = &mut self.screensaver {
            if ss.should_dismiss() {
                let exit = ss.exit_on_dismiss;
                self.screensaver = None;
                self.last_screensaver_dismiss = Instant::now();
                if self.config.screensaver.lock_on_dismiss {
                    crate::desktop::screensaver::lock_workstation();
                }
                if exit {
                    unsafe {
                        PostQuitMessage(0);
                    }
                }
            }
            return;
        }

        if self.config.screensaver.enabled && !self.perf_active && !self.on_battery && !self.is_paused {
            let idle_limit = Duration::from_secs(u64::from(self.config.screensaver.idle_minutes.max(1)) * 60);
            if self.last_screensaver_dismiss.elapsed() >= Duration::from_secs(5)
                && crate::desktop::screensaver::system_idle_duration() >= idle_limit
            {
                self.start_screensaver(false);
            }
        }
    }

    fn render_screensaver_viewports(&mut self) -> Result<()> {
        let Some(ss) = &self.screensaver else {
            return Ok(());
        };
        let now = Instant::now();
        let (clock_time, clock_date) = system_clock();
        let audio_snap = self.audio_engine.snapshot();
        let fade = self.fade_amount();

        for sv in &ss.viewports {
            let time = now.duration_since(self.start_time).as_secs_f32();
            let constants = ShaderConstants {
                resolution: [sv.monitor.width as f32, sv.monitor.height as f32],
                time,
                speed: self.speed,
                tint: self.tint_rgba,
                mouse: [0.5, 0.5],
                mouse_click: [0.0, 0.0],
                mouse_enabled: 0.0,
                parallax_enabled: 0.0,
                tint_ref: [0.0, 0.0],
                clock_time,
                clock_date,
                audio_reactive: audio_snap.reactive,
                audio_spectrum: audio_snap.spectrum,
                params: [0.0; 8],
                fade: [fade, 0.0, 0.0, 0.0],
            };

            let use_custom = ss.force_custom || self.wallpapers.is_web_monitor(sv.monitor.id);
            if use_custom {
                if let Some(wp) = &ss.custom_wallpaper {
                    let _ = wp.render(&self.gpu.context, &sv.surface, constants);
                }
            } else {
                let _ = self.wallpapers.render_monitor(
                    sv.monitor.id,
                    &self.gpu.context,
                    &sv.surface,
                    constants,
                );
            }
        }
        Ok(())
    }

    fn apply_playlist_item(&mut self, monitor_id: usize, spec: String) {
        info!(monitor = monitor_id, wallpaper = %spec, "Playlist: trocando wallpaper");
        if let Err(e) = self.wallpapers.set_monitor_wallpaper(&self.gpu.device, monitor_id, &spec) {
            error!(monitor = monitor_id, error = %e, "Playlist: falha ao aplicar wallpaper");
        }
        self.config.monitor_wallpapers.insert(monitor_id, spec);
        let _ = self.config.save();
        self.schedule_snapshot();
    }

    fn schedule_snapshot(&mut self) {
        if self.system_sync.is_some() || self.config.mirror_system_wallpaper || self.config.mirror_lock_screen {
            self.snapshot_due = Some(Instant::now() + Duration::from_secs(3));
        }
    }

    /// Grava um quadro de cada monitor como wallpaper do Windows.
    fn tick_snapshot(&mut self) {
        if self.snapshot_due.is_none_or(|t| Instant::now() < t) || self.transition.is_some() {
            return;
        }
        self.snapshot_due = None;
        if self.system_sync.is_none() && !self.config.mirror_lock_screen {
            return;
        }
        if self.snapshot_renderer.is_none() {
            self.snapshot_renderer = crate::render::offscreen::OffscreenRenderer::new()
                .map_err(|e| warn!(error = %e, "Renderizador de snapshot indisponível"))
                .ok();
        }
        let Some(renderer) = self.snapshot_renderer.as_mut() else { return };
        let time = self.start_time.elapsed().as_secs_f32();
        // A tela de bloqueio é uma só: usa o monitor principal (origem 0,0) ou o primeiro.
        let primary = self
            .viewports
            .iter()
            .find(|v| v.monitor.screen_rect.left == 0 && v.monitor.screen_rect.top == 0)
            .or(self.viewports.first())
            .map(|v| v.monitor.id);
        for v in &self.viewports {
            let mut c = crate::render::offscreen::preview_constants(v.monitor.width, v.monitor.height, time, self.tint_rgba);
            c.audio_reactive = [0.0; 4];
            c.audio_spectrum = [0.0; 16];
            c.speed = self.speed;
            if let Some(png) = self.wallpapers.snapshot_png(v.monitor.id, renderer, c) {
                if let Some(sync) = self.system_sync.as_mut() {
                    sync.apply(&v.monitor.screen_rect, &png);
                }
                if self.config.mirror_lock_screen && Some(v.monitor.id) == primary
                    && let Some(dir) = crate::config::EngineConfig::config_path().parent() {
                        crate::desktop::lock_screen::set_image_async(dir.to_path_buf(), png);
                    }
            }
        }
    }

    fn recover_gpu(&mut self) -> Result<()> {
        self.teardown_viewports();
        self.gpu.recover()?;
        self.wallpapers = WallpaperManager::new_empty(&self.gpu.device)?;
        self.setup_viewports()?;
        Ok(())
    }


    /// Mensagens `ai_*` do Studio. A geração roda em outra thread e avisa a página por `emit`.
    fn handle_ai(&mut self, msg: &serde_json::Value) {
        use crate::ai::{Hub, Request, keys, provider::Kind};
        use serde_json::json;

        let studio = self.studio.clone();
        let emit: crate::ai::Emit = std::sync::Arc::new(move |event| {
            if let Some(s) = &studio {
                s.post_script(format!("window.onAiEvent&&window.onAiEvent({event});"));
            }
        });
        let kind = msg.get("provider").and_then(|p| p.as_str()).and_then(Kind::parse);
        match msg.get("action").and_then(|a| a.as_str()).unwrap_or("") {
            "ai_status" => emit(json!({"type": "keys", "keys": Hub::key_status()})),
            "ai_cancel" => self.ai.cancel(),
            "ai_open_keypage" => {
                if let Some(kind) = kind {
                    crate::ai::open_key_page(kind);
                }
            }
            "ai_drafts" => emit(json!({"type": "drafts", "drafts": self.ai.drafts_json()})),
            "ai_draft_remove" => {
                if let Some(name) = msg.get("name").and_then(|n| n.as_str()) {
                    self.ai.remove_draft(name);
                }
                emit(json!({"type": "drafts", "drafts": self.ai.drafts_json()}));
            }
            "ai_set_limits" => {
                let steps = msg.get("max_steps").and_then(|v| v.as_u64()).unwrap_or(self.config.ai.max_steps as u64);
                let tokens = msg.get("max_total_tokens").and_then(|v| v.as_u64()).unwrap_or(self.config.ai.max_total_tokens);
                let mut ai = self.config.ai.clone();
                ai.max_steps = steps.min(u32::MAX as u64) as u32;
                ai.max_total_tokens = tokens;
                self.config.ai = ai.sanitized();
                let _ = self.config.save();
                emit(json!({"type": "limits", "max_steps": self.config.ai.max_steps, "max_total_tokens": self.config.ai.max_total_tokens}));
            }
            "ai_new" => {
                self.ai.reset();
                emit(json!({"type": "reset"}));
            }
            "ai_set_key" => {
                let Some(kind) = kind else { return };
                let key = msg.get("key").and_then(|k| k.as_str()).unwrap_or("").trim();
                if key.is_empty() {
                    keys::delete(kind.id());
                } else if let Err(e) = keys::save(kind.id(), key) {
                    emit(json!({"type": "error", "message": e}));
                }
                emit(json!({"type": "keys", "keys": Hub::key_status()}));
            }
            "ai_models" => match kind {
                Some(kind) => Hub::list_models(kind, emit),
                None => emit(json!({"type": "error", "message": "Unknown provider."})),
            },
            "ai_start" => {
                let Some(kind) = kind else {
                    emit(json!({"type": "error", "message": "Unknown provider."}));
                    return;
                };
                let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("").trim().to_string();
                let vision = kind != Kind::OpenRouter || msg.get("vision").and_then(|v| v.as_bool()).unwrap_or(true);
                let prompt = msg.get("prompt").and_then(|p| p.as_str()).unwrap_or("").to_string();
                // Lembra a escolha para a próxima abertura do Studio.
                self.config.ai.provider = kind.id().to_string();
                if !model.is_empty() {
                    self.config.ai.models.insert(kind.id().to_string(), model.clone());
                }
                if kind == Kind::OpenRouter {
                    self.config.ai.openrouter_vision = vision;
                }
                let _ = self.config.save();
                if let Err(e) = self.ai.start(Request { kind, model, vision, prompt, limits: self.config.ai.limits() }, emit.clone()) {
                    emit(json!({"type": "error", "message": e}));
                }
            }
            _ => {}
        }
    }

    /// Renderiza o shader `spec` em ~4K (lado menor 2160, no formato do monitor) e o aplica como
    /// imagem. Devolve se aplicou.
    fn freeze_wallpaper(&mut self, spec: &str, monitor_id: usize) -> bool {
        let Some(v) = self.viewports.iter().find(|v| v.monitor.id == monitor_id) else {
            self.notify_import_error("Monitor não encontrado.");
            return false;
        };
        let Some((key, code)) = WallpaperManager::shader_source(spec) else {
            self.notify_import_error("Só dá para congelar shaders (os wallpapers web e de vídeo não).");
            return false;
        };
        let (w, h) = freeze_size(v.monitor.width, v.monitor.height);
        if self.snapshot_renderer.is_none() {
            self.snapshot_renderer = crate::render::offscreen::OffscreenRenderer::new()
                .map_err(|e| warn!(error = %e, "Renderizador offscreen indisponível"))
                .ok();
        }
        let Some(renderer) = self.snapshot_renderer.as_mut() else {
            self.notify_import_error("A GPU não está disponível para renderizar a imagem.");
            return false;
        };
        let started = Instant::now();
        let mut c = crate::render::offscreen::preview_constants(w, h, self.start_time.elapsed().as_secs_f32(), self.tint_rgba);
        c.audio_reactive = [0.0; 4];
        c.audio_spectrum = [0.0; 16];
        c.speed = self.speed;
        let png = renderer
            .render(&key, &code, c)
            .ok()
            .and_then(|px| crate::render::offscreen::encode_png(&px, w, h));
        let Some(png) = png else {
            self.notify_import_error("Falha ao renderizar o shader em alta resolução.");
            return false;
        };
        let dir = EngineConfig::config_path().parent().unwrap_or(std::path::Path::new(".")).join("wallpapers");
        let file = dir.join(format!("{}_{w}x{h}.png", crate::shadertoy::file_stem(&key)));
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, &png)) {
            self.notify_import_error(&format!("Não consegui gravar a imagem: {e}"));
            return false;
        }
        info!(shader = %key, width = w, height = h, ms = started.elapsed().as_millis() as u64, "Shader congelado em imagem");
        self.import_and_apply_wallpaper(monitor_id, &file.to_string_lossy());
        true
    }

    /// Grava no disco os valores de parâmetros alterados (no máximo uma vez por segundo).
    fn flush_params(&mut self) {
        if self.params_dirty && self.last_params_save.elapsed() >= Duration::from_secs(1) {
            self.params_dirty = false;
            self.last_params_save = Instant::now();
            let _ = self.config.save();
        }
    }

    fn data_dir() -> std::path::PathBuf {
        EngineConfig::config_path().parent().unwrap_or(std::path::Path::new(".")).to_path_buf()
    }

    fn notify_removed(&self, payload: serde_json::Value) {
        if let Some(studio) = &self.studio {
            studio.eval_script(&format!("window.onWallpaperRemoved && window.onWallpaperRemoved({payload});"));
        }
    }

    /// Remove um wallpaper importado/criado. Os monitores que o exibem voltam ao padrão (`matrix`), o
    /// que dependia dele na configuração (playlist, ajustes de parâmetros) é limpo e o arquivo vai para
    /// a Lixeira do Windows (dá para restaurar de lá). Fora da pasta de importados, nada é tocado.
    /// Dá um nome de exibição a um wallpaper da pasta de importados (o arquivo não muda). Título vazio remove o nome.
    fn set_wallpaper_title(&mut self, path: &str, raw: &str) {
        let dir = Self::data_dir().join("wallpapers");
        if let Err(msg) = crate::import::resolve_inside(&dir, path) {
            warn!(path, %msg, "Renomear recusado: só vale para wallpapers da pasta de importados");
            return;
        }
        let title = crate::config::clean_title(raw);
        if title.is_empty() {
            self.config.wallpaper_titles.remove(path);
        } else {
            self.config.wallpaper_titles.insert(path.to_string(), title);
        }
        let _ = self.config.save();
        self.refresh_tray_info();
    }

    fn remove_wallpaper(&mut self, spec: &str) {
        let dir = Self::data_dir().join("wallpapers");
        let file = match crate::import::resolve_inside(&dir, spec) {
            Ok(f) => f,
            Err(msg) => {
                warn!(spec, %msg, "Remoção de wallpaper recusada");
                self.notify_removed(serde_json::json!({"ok": false, "path": spec, "message": msg}));
                return;
            }
        };
        // Os monitores que o usam saem antes (libera o arquivo: vídeo e página mantêm handles abertos).
        let same = |s: &str| std::path::Path::new(s).canonicalize().is_ok_and(|p| p == file) || s == spec;
        let using: Vec<usize> = self.config.monitor_wallpapers.iter().filter(|(_, s)| same(s)).map(|(id, _)| *id).collect();
        for id in &using {
            let _ = self.wallpapers.set_monitor_wallpaper(&self.gpu.device, *id, "matrix");
            self.config.monitor_wallpapers.insert(*id, "matrix".into());
        }
        self.config.playlist.items.retain(|i| !same(i));
        let keys: Vec<String> = self.config.shader_params.keys().filter(|k| same(k)).cloned().collect();
        for k in keys {
            self.config.shader_params.remove(&k);
            self.wallpapers.set_shader_params(&k, Vec::new());
        }
        self.rebuild_playlist();
        self.config.wallpaper_titles.remove(spec);
        match crate::import::recycle(&file) {
            Ok(()) => {
                if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("livewallpaper")) {
                    let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    crate::import::clean_package_cache(&Self::data_dir().join("cache"), &stem);
                }
                info!(path = %file.display(), monitors = ?using, "Wallpaper movido para a Lixeira");
                let _ = self.config.save();
                self.notify_removed(serde_json::json!({"ok": true, "path": spec, "replaced": using, "replacement": "matrix"}));
            }
            Err(e) => {
                error!(path = %file.display(), error = %e, "Não consegui mandar o wallpaper para a Lixeira");
                let _ = self.config.save();
                self.notify_removed(serde_json::json!({"ok": false, "path": spec, "message": format!("Não consegui mandar para a Lixeira: {e}"), "replaced": using, "replacement": "matrix"}));
            }
        }
    }

    fn notify_data_usage(&self) {
        let rows: Vec<serde_json::Value> = crate::import::data_usage(&Self::data_dir())
            .into_iter()
            .map(|(name, files, bytes)| serde_json::json!({"what": name, "files": files, "bytes": bytes}))
            .collect();
        if let Some(studio) = &self.studio {
            studio.eval_script(&format!("window.onDataUsage && window.onDataUsage({});", serde_json::Value::Array(rows)));
        }
    }

    /// Limpa um tipo de dado do app: caches e rascunhos são apagados; backups da configuração vão para a
    /// Lixeira. Nunca toca em wallpapers do usuário nem no `config.toml`.
    fn clean_data(&mut self, what: &str) {
        use crate::import::{DataKind, clean_data, delete_path, recycle};
        let Some(kind) = DataKind::parse(what) else {
            warn!(what, "Tipo de dado desconhecido ignorado");
            return;
        };
        let (n, bytes) = if kind.recoverable() { clean_data(&Self::data_dir(), kind, &recycle) } else { clean_data(&Self::data_dir(), kind, &delete_path) };
        info!(what, items = n, bytes, "Dados limpos");
        if let Some(studio) = &self.studio {
            studio.eval_script(&format!("window.onDataCleaned && window.onDataCleaned({});", serde_json::json!({"what": what, "files": n, "bytes": bytes})));
        }
        self.notify_data_usage();
    }

    /// Avisa o Studio que o motor confirmou a troca (`None` = todos os monitores).
    fn notify_applied(&self, monitor: Option<usize>, path: &str) {
        if let Some(studio) = &self.studio {
            let mon = monitor.map_or("null".to_string(), |m| m.to_string());
            let path = serde_json::to_string(path).unwrap_or_default();
            studio.eval_script(&format!("window.onApplied && window.onApplied({mon}, {path});"));
        }
    }

    /// Reavalia o modo desempenho (no máximo a cada segundo) e aplica a mudança.
    fn update_performance(&mut self) {
        use crate::performance::{fullscreen_app_running, should_be_active};
        if self.last_perf_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_perf_check = Instant::now();
        let mode = self.config.performance_mode;
        let fullscreen = mode == crate::config::PerformanceMode::Auto && fullscreen_app_running();
        let want = should_be_active(mode, fullscreen);
        if want == self.perf_active {
            return;
        }
        self.perf_active = want;
        info!(active = want, mode = mode.as_str(), "Modo desempenho alterado");
        if want {
            self.audio_engine.suspend();
        } else {
            self.audio_engine.resume();
        }
        self.media.set_sleeping(want);
        self.system.set_sleeping(want);
        crate::performance::set_low_power(want);
        self.notify_performance();
    }

    /// Escolha do usuário (Ajustes ou bandeja): grava, reavalia na hora e avisa Studio e bandeja.
    fn set_performance_mode(&mut self, mode: crate::config::PerformanceMode) {
        info!(mode = mode.as_str(), "Modo desempenho escolhido");
        self.config.performance_mode = mode;
        let _ = self.config.save();
        self.last_perf_check = Instant::now() - Duration::from_secs(10);
        self.update_performance();
        self.tray_wallpapers = None; // força o menu da bandeja a refletir a escolha
        self.notify_performance();
    }

    fn notify_performance(&self) {
        if let Some(studio) = &self.studio {
            studio.eval_script(&format!(
                "window.onPerformance && window.onPerformance({}, '{}');",
                self.perf_active,
                self.config.performance_mode.as_str()
            ));
        }
    }

    /// Aplica `path` em todos os monitores (Studio e bandeja). Quem falha mantém o que estava no ar;
    /// o Studio é avisado do resultado.
    fn apply_to_all(&mut self, path: &str) {
        info!(path = %path, "Sincronizando papel de parede em todos os monitores");
        let ids: Vec<usize> = self.viewports.iter().map(|v| v.monitor.id).collect();
        let mut failed = Vec::new();
        let mut last_error = String::new();
        for id in ids {
            match self.wallpapers.set_monitor_wallpaper(&self.gpu.device, id, path) {
                Ok(()) => {
                    info!(monitor = id, path = %path, "Monitor sincronizado com sucesso!");
                    self.config.monitor_wallpapers.insert(id, path.to_string());
                }
                Err(e) => {
                    error!(monitor = id, path = %path, error = %e, "Erro ao sincronizar monitor");
                    last_error = e.message();
                    failed.push(id);
                }
            }
        }
        let _ = self.config.save();
        if failed.is_empty() {
            self.notify_applied(None, path);
        } else {
            self.notify_apply_error(&failed, path, &last_error);
        }
    }

    /// Atualiza para o menu da bandeja o que está no ar: um nome só se todos os monitores usam o mesmo.
    fn refresh_tray_info(&self) {
        let paths: Vec<&String> = self.viewports.iter().filter_map(|v| self.config.monitor_wallpapers.get(&v.monitor.id)).collect();
        let mut info = match paths.first() {
            None => crate::desktop::tray::TrayInfo::default(),
            Some(first) if paths.iter().all(|p| p == first) => crate::desktop::tray::TrayInfo {
                current_title: self.config.wallpaper_titles.get(first.as_str()).cloned().unwrap_or_else(|| crate::desktop::tray::wallpaper_title(first)),
                current_path: Some((*first).clone()),
                ..Default::default()
            },
            Some(_) => crate::desktop::tray::TrayInfo { current_title: "vários wallpapers".into(), ..Default::default() },
        };
        info.performance_always = self.config.performance_mode == crate::config::PerformanceMode::Always;
        info.titles = self.config.wallpaper_titles.clone();
        if let Ok(mut slot) = self.listener.events.tray_info.lock() {
            *slot = info;
        }
    }

    /// Avisa o Studio que a troca falhou e informa o que continua no ar em cada monitor.
    fn notify_apply_error(&self, monitors: &[usize], path: &str, message: &str) {
        if let Some(studio) = &self.studio {
            let current: std::collections::HashMap<usize, &String> =
                monitors.iter().filter_map(|m| self.config.monitor_wallpapers.get(m).map(|s| (*m, s))).collect();
            let payload = serde_json::json!({ "monitors": monitors, "path": path, "message": message, "current": current });
            studio.eval_script(&format!("window.onApplyError && window.onApplyError({payload});"));
        }
    }

    /// Restaura os padrões. Guarda antes uma cópia do `config.toml` (`config.toml.bak-<unix>`) e
    /// preserva o tour já visto e as preferências da IA (as chaves de API ficam no Windows).
    fn reset_config(&mut self) {
        let path = EngineConfig::config_path();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let backup = path.with_extension(format!("toml.bak-{stamp}"));
        let backed_up = std::fs::copy(&path, &backup).is_ok();
        if !backed_up && path.exists() {
            // Sem backup não restaura: melhor não mexer do que perder a configuração.
            error!(path = %path.display(), "Não consegui copiar o config.toml; restauração cancelada");
            if let Some(studio) = &self.studio {
                studio.eval_script("window.onConfigReset && window.onConfigReset(null);");
            }
            return;
        }
        self.config.reset_to_defaults();
        self.speed = self.config.speed;
        self.tint_rgba = parse_hex_color(&self.config.tint);
        self.mouse_enabled = self.config.mouse_enabled;
        self.parallax_enabled = self.config.parallax_enabled;
        let _ = crate::desktop::set_autostart_enabled(self.config.autostart);
        self.rebuild_playlist();
        if let Err(e) = self.setup_viewports() {
            error!(error = %e, "Falha ao recriar os monitores depois de restaurar a configuração");
        }
        let _ = self.config.save();
        info!(backup = %backup.display(), "Configuração restaurada para os padrões");
        if let Some(studio) = &self.studio {
            let b = serde_json::to_string(&backup.to_string_lossy()).unwrap_or_default();
            studio.eval_script(&format!("window.onConfigReset && window.onConfigReset({b});"));
        }
    }

    fn notify_import_error(&self, msg: &str) {
        warn!(error = %msg, "Importação do Shadertoy recusada");
        if let Some(studio) = &self.studio {
            let json = serde_json::to_string(msg).unwrap_or_default();
            studio.eval_script(&format!("window.onImportError && window.onImportError({json});"));
        }
    }

    /// Grava o HLSL convertido numa pasta temporária e segue o fluxo normal de importação
    /// (cópia sem sobrescrever homônimos, aplicação e aviso ao Studio).
    fn import_converted_shader(&mut self, monitor_id: usize, stem: &str, hlsl: &str, warnings: &[String]) {
        let dir = std::env::temp_dir().join("isnotaengine_shadertoy");
        let file = dir.join(format!("{stem}.hlsl"));
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, hlsl)) {
            self.notify_import_error(&format!("Não consegui gravar o shader convertido: {e}"));
            return;
        }
        for w in warnings {
            warn!(shader = %stem, warning = %w, "Aviso da conversão do Shadertoy");
        }
        self.import_and_apply_wallpaper(monitor_id, &file.to_string_lossy());
    }

    /// Pede ao Studio que o usuário confirme a importação de uma página (que roda código no desktop).
    fn ask_import_confirmation(&self, path: &str, monitor: usize) {
        if let Some(studio) = &self.studio {
            let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (path, name) = (serde_json::to_string(path).unwrap_or_default(), serde_json::to_string(&name).unwrap_or_default());
            studio.eval_script(&format!("window.onImportConfirm && window.onImportConfirm({path}, {name}, {monitor});"));
        }
    }

    fn import_and_apply_wallpaper(&mut self, monitor_id: usize, src_path_str: &str) {
        let src_path = std::path::Path::new(src_path_str);
        if !src_path.exists() {
            warn!(path = %src_path_str, "Arquivo de wallpaper a importar não existe");
            return;
        }

        // Arquivos GLSL do Shadertoy são convertidos para HLSL antes de importar.
        let is_glsl = src_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "glsl" | "frag"));
        if is_glsl {
            let title = src_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let converted = std::fs::read_to_string(src_path)
                .map_err(|e| format!("Não consegui ler o arquivo: {e}"))
                .and_then(|code| crate::shadertoy::convert(&code, &title));
            match converted {
                Ok(c) => {
                    let stem = crate::shadertoy::file_stem(&title);
                    self.import_converted_shader(monitor_id, &stem, &c.hlsl, &c.warnings);
                }
                Err(msg) => self.notify_import_error(&msg),
            }
            return;
        }

        let wp_dir = EngineConfig::config_path()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("wallpapers");
        let final_path_str = match crate::import::import_into(src_path, &wp_dir) {
            Ok(target) => {
                info!(from = %src_path_str, to = %target.display(), "Wallpaper importado e salvo localmente");
                target.to_string_lossy().to_string()
            }
            Err(e) => {
                warn!(error = %e, "Não foi possível copiar wallpaper para pasta local; usando caminho original");
                src_path.to_string_lossy().to_string()
            }
        };

        match self.wallpapers.set_monitor_wallpaper(&self.gpu.device, monitor_id, &final_path_str) {
            Ok(()) => {
                info!(monitor = monitor_id, path = %final_path_str, "Wallpaper importado aplicado com sucesso!");
                self.config.monitor_wallpapers.insert(monitor_id, final_path_str.clone());
                let _ = self.config.save();

                let wp_type = crate::import::kind_label(std::path::Path::new(&final_path_str));

                let title = std::path::Path::new(&final_path_str)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Custom Wallpaper".to_string());

                if let Some(studio) = &self.studio {
                    studio.notify_wallpaper_imported(monitor_id, &title, &final_path_str, wp_type);
                }
            }
            Err(e) => {
                error!(monitor = monitor_id, error = %e, "Erro ao aplicar wallpaper importado");
            }
        }
    }

    fn cleanup(&mut self) {
        info!("Encerrando motor e liberando recursos nativos...");
        self.teardown_viewports();
        if let Some(sync) = self.system_sync.as_mut() {
            sync.restore();
        }
        repaint_desktop(self.layer.parent);
        info!("Encerramento concluído com sucesso.");
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.cleanup();
    }
}

struct EngineStats {
    since: Instant,
    frames: u32,
    cpu_at: u64,
}

impl EngineStats {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            cpu_at: process_cpu_100ns(),
        }
    }

    fn record_frame(&mut self) {
        self.frames += 1;
        let elapsed = self.since.elapsed();
        if elapsed >= Duration::from_secs(5) {
            let cpu_now = process_cpu_100ns();
            let cpu_secs = (cpu_now - self.cpu_at) as f64 / 10_000_000.0;
            let fps = self.frames as f64 / elapsed.as_secs_f64();
            let cpu_pct = 100.0 * cpu_secs / elapsed.as_secs_f64();

            info!(
                fps = format!("{:.1}", fps),
                cpu_usage = format!("{:.2}%", cpu_pct),
                "Telemetria de desempenho"
            );

            *self = Self {
                since: Instant::now(),
                frames: 0,
                cpu_at: cpu_now,
            };
        }
    }
}

fn process_cpu_100ns() -> u64 {
    let (mut c, mut e, mut k, mut u) = Default::default();
    let _ = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    ft(k) + ft(u)
}

fn register_surface_class() -> Result<PCWSTR> {
    let class = w!("IsNotAEngine_Surface");
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(surface_wndproc),
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            // Ignora se já estiver registrada
            debug!("Classe de janela já registrada ou erro recuperável.");
        }
    }
    Ok(class)
}

fn create_child_window(
    class: PCWSTR,
    parent: HWND,
    mode: PresentMode,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<HWND> {
    let ex_style = match mode {
        PresentMode::Composition => WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE,
        PresentMode::Hwnd => WS_EX_NOACTIVATE,
    };
    unsafe {
        CreateWindowExW(
            ex_style,
            class,
            w!("IsNotAEngine_Child"),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            x,
            y,
            w,
            h,
            Some(parent),
            None,
            Some(GetModuleHandleW(None)?.into()),
            None,
        )
    }
}

fn create_web_child_window(
    class: PCWSTR,
    parent: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            class,
            w!("IsNotAEngine_WebChild"),
            WS_CHILD | WS_CLIPSIBLINGS,
            x,
            y,
            w,
            h,
            Some(parent),
            None,
            Some(GetModuleHandleW(None)?.into()),
            None,
        )
    }
}

extern "system" fn surface_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> BOOL {
    let _ = unsafe {
        PostThreadMessageW(
            MAIN_THREAD.load(Ordering::SeqCst),
            WM_QUIT,
            WPARAM(0),
            LPARAM(0),
        )
    };
    true.into()
}

fn get_mouse_state() -> (POINT, bool, bool) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let lbutton = (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0;
        let rbutton = (GetAsyncKeyState(VK_RBUTTON.0 as i32) as u16 & 0x8000) != 0;
        (pt, lbutton, rbutton)
    }
}

/// `true` se o PC tem bateria e está desconectado da tomada.
fn running_on_battery() -> bool {
    let mut status = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
    if unsafe { windows::Win32::System::Power::GetSystemPowerStatus(&mut status) }.is_err() {
        return false;
    }
    // ACLineStatus 0 = na bateria; BatteryFlag 128 = sem bateria (desktop)
    status.ACLineStatus == 0 && status.BatteryFlag != 128
}

/// Um wallpaper salvo na config ainda existe? (id do catálogo, caminho em disco ou URL)
/// Ids de wallpapers removidos do catálogo caem no padrão, em vez de virar o fallback escuro.
/// Tamanho da imagem de um shader congelado: lado menor 2160 (4K) no formato do monitor,
/// com o lado maior limitado a 7680.
fn freeze_size(monitor_w: u32, monitor_h: u32) -> (u32, u32) {
    let (mw, mh) = (monitor_w.max(1) as f64, monitor_h.max(1) as f64);
    let long = (2160.0 * (mw / mh).max(mh / mw)).round().min(7680.0) as u32;
    if monitor_w >= monitor_h { (long, 2160) } else { (2160, long) }
}

fn wallpaper_spec_available(spec: &str) -> bool {
    std::path::Path::new(spec).exists()
        || crate::wallpaper::catalog::resolve(spec).is_some()
        || crate::wallpaper::factory::resolve(spec).is_some()
}

pub fn system_clock() -> ([f32; 4], [f32; 4]) {
    unsafe {
        let st = windows::Win32::System::SystemInformation::GetLocalTime();
        let hour = st.wHour as f32;
        let minute = st.wMinute as f32;
        let second = st.wSecond as f32;
        let millis = st.wMilliseconds as f32;
        let continuous_sec = second + (millis / 1000.0);

        let clock_time = [hour, minute, second, continuous_sec];
        let clock_date = [
            st.wYear as f32,
            st.wMonth as f32,
            st.wDay as f32,
            st.wDayOfWeek as f32,
        ];
        (clock_time, clock_date)
    }
}

#[cfg(test)]
mod freeze_tests {
    use super::freeze_size;

    #[test]
    fn freeze_size_is_4k_class_in_the_monitor_shape() {
        assert_eq!(freeze_size(1920, 1080), (3840, 2160));
        assert_eq!(freeze_size(2560, 1440), (3840, 2160));
        assert_eq!(freeze_size(3440, 1440), (5160, 2160)); // ultrawide 21:9
        assert_eq!(freeze_size(1080, 1920), (2160, 3840)); // retrato
        assert_eq!(freeze_size(5120, 1440), (7680, 2160)); // super ultrawide: lado maior limitado
        assert_eq!(freeze_size(0, 0), (2160, 2160)); // degenerado
    }
}

#[cfg(test)]
mod spec_tests {
    use super::wallpaper_spec_available;

    #[test]
    fn removed_ids_fall_back_to_the_default() {
        assert!(wallpaper_spec_available("matrix"));
        assert!(wallpaper_spec_available("coding"));
        // endereços da internet não são mais wallpapers (config antiga cai no padrão)
        assert!(!wallpaper_spec_available("https://exemplo.com/wallpaper"));
        assert!(!wallpaper_spec_available("http://localhost:8765/"));
        // ids de wallpapers que saíram do catálogo (config antiga do usuário)
        assert!(!wallpaper_spec_available("milky_way"));
        assert!(!wallpaper_spec_available("synthwave_sunset"));
        assert!(!wallpaper_spec_available("C:/nao/existe/meu.hlsl"));
        // um arquivo que existe continua valendo (wallpaper importado)
        assert!(wallpaper_spec_available(file!()) || wallpaper_spec_available("Cargo.toml"));
    }
}
