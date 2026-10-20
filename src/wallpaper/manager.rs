//! Gerenciador de wallpapers por monitor e suporte a hot-reload.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::core::Result;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNA, ShowWindow};

use super::Wallpaper;
use super::catalog;
use super::image::{self, ImageWallpaper};
use super::shader::ShaderWallpaper;
use super::video::VideoWallpaper;
use super::web::{WebSource, WebWallpaper};
use crate::render::pipeline::ShaderConstants;
use crate::render::RenderSurface;


/// Shader de fallback interno (fora do catálogo): usado quando um wallpaper não carrega.
pub const FALLBACK_SOURCE: &str = include_str!("../../assets/wallpapers/fallback.hlsl");

/// Carrega um shader do catálogo: do disco (hot-reload) durante o desenvolvimento,
/// senão a versão embutida no binário.
fn load_catalog_shader(device: &ID3D11Device, entry: &catalog::Entry) -> Result<ShaderWallpaper> {
    let dev_path = Path::new("assets/wallpapers/shaders").join(format!("{}.hlsl", entry.id));
    if dev_path.exists() {
        match ShaderWallpaper::from_file(device, dev_path) {
            Ok(wp) => return Ok(wp),
            Err(e) => warn!(id = entry.id, error = %e, "Shader de desenvolvimento falhou; usando o embutido"),
        }
    }
    ShaderWallpaper::from_embedded(device, entry.id, entry.source)
}

fn fallback_shader(device: &ID3D11Device) -> Result<ShaderWallpaper> {
    ShaderWallpaper::from_embedded(device, "fallback", FALLBACK_SOURCE)
}

pub struct WallpaperManager {
    monitors: HashMap<usize, Box<dyn Wallpaper>>,
    monitor_windows: HashMap<usize, (HWND, HWND, u32, u32)>,
    fallback: Box<dyn Wallpaper>,
    /// Monitores suspensos: valem também para um wallpaper aplicado depois da pausa.
    paused: std::collections::HashSet<usize>,
    /// Último spec carregado com sucesso em cada monitor (para restaurar se a troca falhar).
    specs: HashMap<usize, String>,
    /// Valores de parâmetros escolhidos pelo usuário, por spec do wallpaper.
    param_overrides: HashMap<String, Vec<(String, f32)>>,
}

impl WallpaperManager {
    /// Inicializa uma instância vazia com shader fallback embutido.
    pub fn new_empty(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            monitors: HashMap::new(),
            monitor_windows: HashMap::new(),
            fallback: Box::new(fallback_shader(device)?),
            paused: std::collections::HashSet::new(),
            specs: HashMap::new(),
            param_overrides: HashMap::new(),
        })
    }

    /// Registra as janelas filhas nativas (D3D11 HWND e Web HWND) e dimensões de um monitor.
    pub fn register_monitor(&mut self, monitor_id: usize, d3d_hwnd: HWND, web_hwnd: HWND, width: u32, height: u32) {
        self.monitor_windows.insert(monitor_id, (d3d_hwnd, web_hwnd, width, height));
    }

    /// Alterna qual superfície do monitor fica visível (D3D11 ou WebView2).
    fn show_surface(&self, monitor_id: usize, web: bool) {
        if let Some(&(d3d_hwnd, web_hwnd, _, _)) = self.monitor_windows.get(&monitor_id) {
            let (show, hide) = if web { (web_hwnd, d3d_hwnd) } else { (d3d_hwnd, web_hwnd) };
            unsafe {
                let _ = ShowWindow(hide, SW_HIDE);
                let _ = ShowWindow(show, SW_SHOWNA);
            }
        }
    }

    fn create_web(&mut self, monitor_id: usize, source: WebSource) -> Option<Box<dyn Wallpaper>> {
        let &(_, web_hwnd, _, _) = self.monitor_windows.get(&monitor_id)?;
        // Libera a WebView anterior deste monitor antes de criar outra na mesma janela.
        self.monitors.remove(&monitor_id);
        match WebWallpaper::new(web_hwnd, source) {
            Ok(wp) => Some(Box::new(wp)),
            Err(e) => {
                warn!(monitor = monitor_id, error = %e, "Falha ao criar WebWallpaper");
                None
            }
        }
    }

    /// Atribui um wallpaper a um monitor. Aceita um id do catálogo (`aurora`), um caminho
    /// de arquivo (`.hlsl`, vídeo, `.html`, pasta com `index.html`, `.livewallpaper`) ou URL.
    ///
    /// Se `spec` não carregar, o wallpaper que já estava no monitor **continua** (e a função devolve
    /// `Err`); só quando o monitor ainda não tem nenhum entra o shader de fallback.
    pub fn set_monitor_wallpaper(&mut self, device: &ID3D11Device, monitor_id: usize, spec: &str) -> Result<()> {
        let Some(mut wallpaper) = self.build(device, monitor_id, spec) else {
            warn!(monitor = monitor_id, spec = %spec, "Wallpaper não pôde ser carregado");
            // A WebView anterior é liberada antes de criar a nova; se a criação falhou, o monitor pode
            // ter ficado sem nada: tenta recolocar o anterior e, sem ele, usa o fallback.
            if !self.monitors.contains_key(&monitor_id) {
                let previous = self.specs.get(&monitor_id).filter(|p| p.as_str() != spec).cloned();
                let restored = previous.and_then(|p| self.build(device, monitor_id, &p));
                let wp = restored.unwrap_or_else(|| Box::new(fallback_shader(device).expect("shader de fallback compila")));
                self.show_surface(monitor_id, wp.is_web());
                if self.paused.contains(&monitor_id) {
                    wp.set_paused(true);
                }
                self.monitors.insert(monitor_id, wp);
            }
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, format!("não foi possível carregar {spec}")));
        };
        self.specs.insert(monitor_id, spec.to_string());
        if let Some(o) = self.param_overrides.get(spec) {
            wallpaper.set_params(o);
        }
        self.show_surface(monitor_id, wallpaper.is_web());
        if self.paused.contains(&monitor_id) {
            wallpaper.set_paused(true);
        }
        info!(monitor = monitor_id, wallpaper = %wallpaper.name(), spec = %spec, "Wallpaper aplicado");
        self.monitors.insert(monitor_id, wallpaper);
        Ok(())
    }

    fn build(&mut self, device: &ID3D11Device, monitor_id: usize, spec: &str) -> Option<Box<dyn Wallpaper>> {
        // Vídeo de fábrica (`video:<nome>`): o arquivo fica na pasta `videos` do app.
        if spec.starts_with(super::factory::PREFIX) {
            let file = super::factory::resolve(spec)?;
            return VideoWallpaper::from_file(device, file)
                .map_err(|e| warn!(spec, error = %e, "Vídeo de fábrica não pôde ser carregado"))
                .ok()
                .map(|w| Box::new(w) as _);
        }
        let path = Path::new(spec);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

        if path.exists() {
            if path.is_dir() && path.join("index.html").exists() {
                return self.create_web(monitor_id, WebSource::Dir { root: path.to_path_buf(), entry: "index.html".into() });
            }
            match ext.as_str() {
                "html" | "htm" => {
                    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                    let entry = path.file_name()?.to_string_lossy().to_string();
                    return self.create_web(monitor_id, WebSource::Dir { root, entry });
                }
                _ if image::is_image(path) => {
                    return ImageWallpaper::from_file(device, path.to_path_buf())
                        .map_err(|e| warn!(path = %path.display(), error = %e, "Imagem não pôde ser carregada"))
                        .ok()
                        .map(|w| Box::new(w) as _);
                }
                "hlsl" => return ShaderWallpaper::from_file(device, path.to_path_buf()).ok().map(|w| Box::new(w) as _),
                "mp4" | "webm" | "wmv" | "mov" | "avi" | "mkv" => {
                    return VideoWallpaper::from_file(device, path.to_path_buf()).ok().map(|w| Box::new(w) as _);
                }
                "livewallpaper" => return self.load_package(device, monitor_id, path),
                _ => {}
            }
        }

        let entry = catalog::resolve(spec)?;
        match entry.kind {
            catalog::Kind::Shader => load_catalog_shader(device, entry).ok().map(|w| Box::new(w) as _),
            catalog::Kind::Web => self.create_web(monitor_id, WebSource::Builtin(entry)),
        }
    }

    /// Instancia um wallpaper D3D11 autônomo (Shader, Vídeo ou Imagem) para uso na proteção de tela viva.
    pub fn build_d3d_standalone(device: &ID3D11Device, spec: &str) -> Option<Box<dyn Wallpaper>> {
        let path = Path::new(spec);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if path.exists() {
            match ext.as_str() {
                _ if image::is_image(path) => {
                    return ImageWallpaper::from_file(device, path.to_path_buf()).ok().map(|w| Box::new(w) as _);
                }
                "hlsl" => return ShaderWallpaper::from_file(device, path.to_path_buf()).ok().map(|w| Box::new(w) as _),
                "mp4" | "webm" | "wmv" | "mov" | "avi" | "mkv" => {
                    return VideoWallpaper::from_file(device, path.to_path_buf()).ok().map(|w| Box::new(w) as _);
                }
                _ => {}
            }
        }
        let entry = catalog::resolve(spec)
            .filter(|e| e.kind == catalog::Kind::Shader)
            .or_else(|| catalog::find("aurora"))?;
        load_catalog_shader(device, entry).ok().map(|w| Box::new(w) as _)
    }

    /// Pacotes `.livewallpaper` (ZIP com manifest.json e content/).
    fn load_package(&mut self, device: &ID3D11Device, monitor_id: usize, path: &Path) -> Option<Box<dyn Wallpaper>> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
        let (kind, mut entry_name) = match read_package_manifest(&mut archive) {
            Ok(m) => m,
            Err(why) => {
                warn!(package = %path.display(), %why, "Pacote .livewallpaper recusado");
                return None;
            }
        };

        let cache_dir = crate::config::EngineConfig::config_path().parent()?.join("cache").join(package_cache_name(path)?);
        if let Err(why) = extract_package(&mut archive, &cache_dir, MAX_PACKAGE_BYTES) {
            warn!(package = %path.display(), %why, "Pacote .livewallpaper recusado na extração");
            return None;
        }

        if entry_name.is_empty() {
            entry_name = match kind.as_str() {
                "video" => "content/video.mp4",
                "web" => "content/index.html",
                _ => "content/shader.hlsl",
            }
            .to_string();
        }
        let Some(entry_path) = resolve_package_entry(&cache_dir, &entry_name) else {
            warn!(package = %path.display(), entry = %entry_name, "Pacote .livewallpaper recusado: `entry` fora da pasta do pacote");
            return None;
        };
        info!(package = %path.display(), entry = %entry_path.display(), "Pacote .livewallpaper extraído");
        self.build(device, monitor_id, &entry_path.to_string_lossy())
    }

    /// Renderiza o wallpaper do monitor especificado.
    pub fn render_monitor(
        &self,
        monitor_id: usize,
        context: &ID3D11DeviceContext,
        surface: &RenderSurface,
        constants: ShaderConstants,
    ) -> Result<()> {
        let wp = self.monitors.get(&monitor_id).unwrap_or(&self.fallback);
        wp.render(context, surface, constants)
    }

    /// Executa hot-reload se algum arquivo de shader tiver sido editado.
    pub fn check_reloads(&mut self, device: &ID3D11Device) {
        for (id, wp) in self.monitors.iter_mut() {
            if let Ok(true) = wp.check_reload(device) {
                debug!(monitor = id, wallpaper = %wp.name(), "Hot-reload concluído");
            }
        }
    }

    /// Guarda os valores de parâmetros de um wallpaper e os aplica em todo monitor que o exibe agora
    /// (e em qualquer um que o receba depois). Lista vazia = padrões do shader.
    pub fn set_shader_params(&mut self, spec: &str, overrides: Vec<(String, f32)>) {
        for (id, s) in &self.specs {
            if s == spec && let Some(wp) = self.monitors.get_mut(id) {
                wp.set_params(&overrides);
            }
        }
        if overrides.is_empty() {
            self.param_overrides.remove(spec);
        } else {
            self.param_overrides.insert(spec.to_string(), overrides);
        }
    }

    /// Suspende ou retoma só o wallpaper de um monitor (suspensão de GPU/WebView2).
    pub fn set_monitor_paused(&mut self, monitor_id: usize, paused: bool) {
        if paused {
            self.paused.insert(monitor_id);
        } else {
            self.paused.remove(&monitor_id);
        }
        if let Some(wp) = self.monitors.get(&monitor_id) {
            wp.set_paused(paused);
        }
    }

    /// Se o wallpaper atual do monitor obedece ao fade (o fallback é um shader, então sim).
    pub fn supports_fade(&self, monitor_id: usize) -> bool {
        self.monitors.get(&monitor_id).unwrap_or(&self.fallback).supports_fade()
    }

    /// Chave e código HLSL (com o header comum) de um shader do catálogo ou de um `.hlsl`,
    /// para renderizar um quadro fora do desktop. `None` para qualquer outro tipo de wallpaper.
    pub fn shader_source(spec: &str) -> Option<(String, String)> {
        let path = Path::new(spec);
        if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("hlsl")) {
            let code = std::fs::read_to_string(path).ok()?;
            let key = path.file_stem()?.to_string_lossy().to_string();
            return Some((key, catalog::with_common(&code)));
        }
        let entry = catalog::resolve(spec).filter(|e| e.kind == catalog::Kind::Shader)?;
        let dev = Path::new("assets/wallpapers/shaders").join(format!("{}.hlsl", entry.id));
        let code = std::fs::read_to_string(dev).unwrap_or_else(|_| entry.source.to_string());
        Some((entry.id.to_string(), catalog::with_common(&code)))
    }

    /// PNG do quadro atual do monitor: render offscreen (shader), captura da página (web), imagem ou vídeo.
    pub fn snapshot_png(
        &self,
        monitor_id: usize,
        renderer: &mut crate::render::offscreen::OffscreenRenderer,
        constants: ShaderConstants,
    ) -> Option<Vec<u8>> {
        let wp = self.monitors.get(&monitor_id).unwrap_or(&self.fallback);
        if let Some(web) = wp.as_web() {
            return web.capture_png();
        }
        let (w, h) = (constants.resolution[0] as u32, constants.resolution[1] as u32);
        if let Some(path) = wp.snapshot_image() {
            let rgba = image::cover_rgba(path, w, h).ok()?;
            return crate::render::offscreen::encode_png(&rgba, w, h);
        }
        if let Some((vw, vh, rgba)) = wp.snapshot_rgba() {
            return crate::render::offscreen::encode_png(&rgba, vw, vh);
        }
        let (key, code) = wp.snapshot_shader()?;
        let rgba = renderer.render(&key, &code, constants).ok()?;
        crate::render::offscreen::encode_png(&rgba, w, h)
    }

    /// Retorna se o monitor ativo está executando um papel de parede Web.
    pub fn is_web_monitor(&self, monitor_id: usize) -> bool {
        self.monitors.get(&monitor_id).map(|w| w.is_web()).unwrap_or(false)
    }

    /// Repassa a música tocando agora para o WebWallpaper do monitor.
    pub fn update_web_media(&self, monitor_id: usize, media: &(u64, String)) {
        if let Some(web) = self.monitors.get(&monitor_id).and_then(|w| w.as_web()) {
            web.update_media(media.0, &media.1);
        }
    }

    /// Repassa as métricas do sistema ao WebWallpaper do monitor.
    pub fn update_web_system(&self, monitor_id: usize, system: &(u64, String)) {
        if let Some(web) = self.monitors.get(&monitor_id).and_then(|w| w.as_web()) {
            web.update_system(system.0, &system.1);
        }
    }

    /// Repassa o FPS configurado ao WebWallpaper do monitor.
    pub fn update_web_fps(&self, monitor_id: usize, fps: u32) {
        if let Some(web) = self.monitors.get(&monitor_id).and_then(|w| w.as_web()) {
            web.update_fps(fps);
        }
    }

    /// Repassa a cor de destaque do Studio ao WebWallpaper do monitor.
    pub fn update_web_tint(&self, monitor_id: usize, tint: [f32; 4]) {
        if let Some(web) = self.monitors.get(&monitor_id).and_then(|w| w.as_web()) {
            web.update_tint(tint);
        }
    }

    /// Injeta os dados de áudio WASAPI em tempo real para um WebWallpaper ativo no monitor.
    pub fn update_web_audio(&self, monitor_id: usize, audio: &crate::audio::AudioSnapshot) {
        if let Some(wp) = self.monitors.get(&monitor_id)
            && let Some(web_wp) = wp.as_web() {
                web_wp.update_audio(
                    audio.reactive[0],
                    audio.reactive[1],
                    audio.reactive[2],
                    audio.reactive[3],
                    &audio.spectrum,
                );
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    unsafe extern "system" fn test_wndproc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    #[test]
    fn test_manager_web_and_shader_switching() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );

            let instance = GetModuleHandleW(None).unwrap();
            let class_name = windows::core::w!("TestMgrClass");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(test_wndproc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);

            let d3d_hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                windows::core::w!("TestMgrD3D"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                600,
                None,
                None,
                Some(instance.into()),
                None,
            ).unwrap();

            let web_hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                windows::core::w!("TestMgrWeb"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                600,
                None,
                None,
                Some(instance.into()),
                None,
            ).unwrap();

            // Cria D3D11 device para o teste
            let gpu = crate::render::GpuContext::new(crate::config::PresentMode::Hwnd).unwrap();
            let mut manager = WallpaperManager::new_empty(&gpu.device).unwrap();

            manager.register_monitor(0, d3d_hwnd, web_hwnd, 800, 600);

            // 1. Aplica wallpaper Web
            let res = manager.set_monitor_wallpaper(&gpu.device, 0, "matrix");
            assert!(res.is_ok(), "set_monitor_wallpaper com Web falhou: {:?}", res.err());
            assert!(manager.is_web_monitor(0), "Monitor 0 deve ser Web");

            // 2. Testa injeção de áudio sem panicar
            let audio_snap = crate::audio::AudioSnapshot {
                reactive: [0.8, 0.5, 0.3, 0.7],
                spectrum: [0.5; 16],
            };
            manager.update_web_audio(0, &audio_snap);

            // 3. Alterna para shader HLSL
            let shader_path = std::env::temp_dir().join("isnotaengine_test_fallback.hlsl");
            std::fs::write(&shader_path, FALLBACK_SOURCE).unwrap();
            let res2 = manager.set_monitor_wallpaper(&gpu.device, 0, &shader_path.to_string_lossy());
            assert!(res2.is_ok(), "set_monitor_wallpaper com Shader falhou: {:?}", res2.err());
            assert!(!manager.is_web_monitor(0), "Monitor 0 não deve mais ser Web");

            // 4. Um wallpaper que não carrega devolve erro e NÃO derruba o que estava no ar.
            let bad = manager.set_monitor_wallpaper(&gpu.device, 0, "nao-existe-xyz");
            assert!(bad.is_err(), "spec inexistente deve falhar");
            assert!(manager.monitors.contains_key(&0), "o wallpaper anterior deve continuar");
            assert_eq!(manager.specs.get(&0).map(String::as_str), Some(shader_path.to_string_lossy().as_ref()));

            let _ = DestroyWindow(d3d_hwnd);
            let _ = DestroyWindow(web_hwnd);
        }
    }
}

/// Limites de descompactação de um `.livewallpaper` (proteção contra zip bomb).
const MAX_PACKAGE_ENTRIES: usize = 2000;
const MAX_PACKAGE_BYTES: u64 = 512 << 20;

/// Teto do `manifest.json`: ele só carrega `type` e `entry`, então 64 KiB sobra.
const MAX_MANIFEST_BYTES: u64 = 64 << 10;

/// Valida os limites do pacote e lê `(type, entry)` do manifest, sem extrair nada.
/// Sem manifest, usa os padrões (`shader`, entry vazio); manifest grande ou JSON inválido é erro.
fn read_package_manifest<R: std::io::Read + std::io::Seek>(archive: &mut zip::ZipArchive<R>) -> std::result::Result<(String, String), String> {
    use std::io::Read;
    if archive.len() > MAX_PACKAGE_ENTRIES {
        return Err(format!("entradas demais ({})", archive.len()));
    }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let f = archive.by_index_raw(i).map_err(|e| format!("ilegível: {e}"))?;
        total = total.saturating_add(f.size());
    }
    if total > MAX_PACKAGE_BYTES {
        return Err(format!("tamanho declarado grande demais ({total} bytes)"));
    }
    let mut kind = "shader".to_string();
    let mut entry = String::new();
    if let Ok(manifest) = archive.by_name("manifest.json") {
        if manifest.size() > MAX_MANIFEST_BYTES {
            return Err(format!("manifest.json grande demais ({} bytes)", manifest.size()));
        }
        let mut text = String::new();
        manifest.take(MAX_MANIFEST_BYTES + 1).read_to_string(&mut text).map_err(|e| format!("manifest.json ilegível: {e}"))?;
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("manifest.json maior que o declarado".into());
        }
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("manifest.json inválido: {e}"))?;
        if let Some(t) = v.get("type").and_then(|t| t.as_str()) {
            kind = t.to_string();
        }
        if let Some(e) = v.get("entry").and_then(|e| e.as_str()) {
            entry = e.to_string();
        }
    }
    Ok((kind, entry))
}

/// Nome da pasta de cache: `<nome>-<hash>` do caminho, tamanho e data do arquivo. Pacotes
/// homônimos em pastas diferentes não colidem, e um pacote alterado ganha pasta nova em vez de
/// apagar a que um wallpaper em uso ainda lê.
fn package_cache_name(path: &Path) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let meta = std::fs::metadata(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    Some(format!("{stem}-{:016x}", h.finish()))
}

/// Extrai o pacote para uma pasta de staging exclusiva e só a promove a `dest` quando tudo deu
/// certo (uma pasta `dest` já pronta é reaproveitada). Recusa caminhos que escapam da raiz, links
/// simbólicos, pacotes aninhados e mais de `max_bytes` realmente descompactados (o tamanho
/// declarado no ZIP pode mentir).
fn extract_package<R: std::io::Read + std::io::Seek>(archive: &mut zip::ZipArchive<R>, dest: &Path, max_bytes: u64) -> std::result::Result<(), String> {
    use std::io::Read;
    if dest.is_dir() {
        return Ok(());
    }
    let parent = dest.parent().ok_or("destino sem pasta pai")?;
    let staging = parent.join(format!(
        "{}.tmp-{}",
        dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("staging: {e}"))?;

    let result = (|| -> std::result::Result<(), String> {
        let mut left = max_bytes;
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).map_err(|e| format!("entrada ilegível: {e}"))?;
            let name = f.name().map(|n| n.to_string()).unwrap_or_else(|_| "?".into());
            let rel = f.enclosed_name().ok_or_else(|| format!("caminho inseguro: {name}"))?;
            if f.is_symlink() {
                return Err(format!("link simbólico recusado: {name}"));
            }
            let ext = rel.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            if matches!(ext.as_deref(), Some("livewallpaper")) {
                return Err(format!("pacote aninhado recusado: {name}"));
            }
            let out = staging.join(&rel);
            if f.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let mut file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            let copied = std::io::copy(&mut (&mut f).take(left + 1), &mut file).map_err(|e| format!("{name}: {e}"))?;
            if copied > left {
                return Err("limite de bytes descompactados excedido".into());
            }
            left -= copied;
        }
        Ok(())
    })();

    match result {
        Ok(()) => std::fs::rename(&staging, dest).or_else(|e| {
            // Outro processo pode ter promovido a mesma pasta no meio tempo.
            if dest.is_dir() { Ok(()) } else { Err(format!("promoção falhou: {e}")) }
        }).inspect(|_| {
            let _ = std::fs::remove_dir_all(&staging);
        }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// Resolve o `entry` do manifest dentro de `cache_dir`; `None` se tentar escapar da pasta
/// (caminho absoluto, `..`, link simbólico) ou se o arquivo não existir.
fn resolve_package_entry(cache_dir: &Path, entry: &str) -> Option<PathBuf> {
    let rel = Path::new(entry);
    if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return None;
    }
    let root = cache_dir.canonicalize().ok()?;
    let file = root.join(rel).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then(|| cache_dir.join(rel))
}

#[cfg(test)]
mod package_tests {
    use super::*;

    #[test]
    fn entry_cannot_escape_package_dir() {
        let base = std::env::temp_dir().join("isnotaengine_pkg_test");
        let dir = base.join("pkg");
        std::fs::create_dir_all(dir.join("content")).unwrap();
        std::fs::write(dir.join("content/shader.hlsl"), "x").unwrap();
        std::fs::write(base.join("fora.hlsl"), "x").unwrap();

        assert!(resolve_package_entry(&dir, "content/shader.hlsl").is_some());
        assert!(resolve_package_entry(&dir, "../fora.hlsl").is_none());
        assert!(resolve_package_entry(&dir, "content/../../fora.hlsl").is_none());
        assert!(resolve_package_entry(&dir, &base.join("fora.hlsl").to_string_lossy()).is_none());
        assert!(resolve_package_entry(&dir, "content/nao_existe.hlsl").is_none());
    }

    fn zip_with(files: &[(&str, &[u8])]) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (n, d) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(d).unwrap();
        }
        zip::ZipArchive::new(std::io::Cursor::new(w.finish().unwrap().into_inner())).unwrap()
    }

    #[test]
    fn manifest_valido_e_ausente() {
        let mut ok = zip_with(&[("manifest.json", br#"{"type":"web","entry":"content/a.html"}"#)]);
        assert_eq!(read_package_manifest(&mut ok), Ok(("web".into(), "content/a.html".into())));
        let mut sem = zip_with(&[("content/shader.hlsl", b"x")]);
        assert_eq!(read_package_manifest(&mut sem), Ok(("shader".into(), String::new())));
    }

    #[test]
    fn manifest_invalido_ou_grande_e_recusado() {
        let mut ruim = zip_with(&[("manifest.json", b"{nao e json")]);
        assert!(read_package_manifest(&mut ruim).is_err());
        let grande = vec![b' '; MAX_MANIFEST_BYTES as usize + 1];
        let mut g = zip_with(&[("manifest.json", &grande)]);
        assert!(read_package_manifest(&mut g).unwrap_err().contains("grande demais"));
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inae_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn extracao_ok_promove_staging() {
        let d = tmp("ext_ok");
        let mut z = zip_with(&[("content/a.txt", b"oi"), ("manifest.json", b"{}")]);
        let dest = d.join("pkg-1");
        extract_package(&mut z, &dest, 1024).unwrap();
        assert_eq!(std::fs::read(dest.join("content/a.txt")).unwrap(), b"oi");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "staging não removido");
    }

    #[test]
    fn extracao_recusa_traversal_aninhado_e_excesso_sem_deixar_rastro() {
        let d = tmp("ext_bad");
        for (nome, arq, limite) in [
            ("traversal", zip_with(&[("../fora.txt", b"x")]), 1024u64),
            ("aninhado", zip_with(&[("content/outro.LiveWallpaper", b"x")]), 1024),
            ("excesso", zip_with(&[("a.bin", &[0u8; 100])]), 10),
        ] {
            let mut z = arq;
            let dest = d.join(nome);
            assert!(extract_package(&mut z, &dest, limite).is_err(), "{nome}");
            assert!(!dest.exists(), "{nome} deixou pacote parcial");
        }
        assert!(!d.join("fora.txt").exists());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "staging sobrou");
    }

    #[test]
    fn cache_name_diferencia_homonimos() {
        let d = tmp("ext_name");
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.livewallpaper"), "1").unwrap();
        std::fs::write(b.join("x.livewallpaper"), "1").unwrap();
        let na = package_cache_name(&a.join("x.livewallpaper")).unwrap();
        let nb = package_cache_name(&b.join("x.livewallpaper")).unwrap();
        assert_ne!(na, nb);
        assert!(na.starts_with("x-"));
    }
}
