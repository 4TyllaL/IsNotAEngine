//! Configuração persistente do motor IsNotAEngine (TOML em %LOCALAPPDATA%).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresentMode {
    /// DirectComposition com swap chain de composição (padrão recomendado no Win11 24H2+).
    Composition,
    /// Swap chain tradicional ancorada diretamente ao HWND.
    Hwnd,
}

fn default_speed() -> f32 {
    1.0
}

fn default_tint() -> String {
    "auto".to_string()
}

fn default_true() -> bool {
    true
}

fn default_interval_minutes() -> u32 {
    10
}

fn default_transition_ms() -> u32 {
    1000
}

/// Rotação automática de wallpapers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistConfig {
    pub enabled: bool,
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u32,
    #[serde(default)]
    pub shuffle: bool,
    /// Duração total do fade (escurece e clareia). 0 = troca direta.
    #[serde(default = "default_transition_ms")]
    pub transition_ms: u32,
    /// Ids do catálogo ou caminhos. Vazio = todos os shaders do catálogo.
    #[serde(default)]
    pub items: Vec<String>,
    /// Filas personalizadas por monitor (índice do monitor -> lista de ids/caminhos).
    /// Quando presente e não vazia para um monitor, sobrepõe `items` naquele monitor.
    #[serde(default)]
    pub monitor_items: HashMap<usize, Vec<String>>,
}

impl Default for PlaylistConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: default_interval_minutes(),
            shuffle: false,
            transition_ms: default_transition_ms(),
            items: Vec::new(),
            monitor_items: HashMap::new(),
        }
    }
}

impl PlaylistConfig {
    /// Limita valores vindos da interface (ou de um `config.toml` editado à mão).
    pub fn sanitized(mut self) -> Self {
        self.interval_minutes = self.interval_minutes.clamp(1, 24 * 60);
        self.transition_ms = self.transition_ms.min(5000);
        self.items.truncate(256);
        self.monitor_items.retain(|&id, list| {
            list.truncate(256);
            id < 16 && !list.is_empty()
        });
        self
    }
}

/// Regra da agenda por horário: a partir de `start` ("HH:MM"), aplica `wallpaper`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleSlot {
    /// Horário de início no formato "HH:MM" (00:00 a 23:59).
    pub start: String,
    /// ID do catálogo ou caminho do wallpaper.
    pub wallpaper: String,
    /// Monitor alvo (`None` = todos os monitores).
    #[serde(default)]
    pub monitor: Option<usize>,
}

/// Agenda automática de wallpapers por horário do dia.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub slots: Vec<ScheduleSlot>,
}

impl ScheduleConfig {
    pub fn sanitized(mut self) -> Self {
        self.slots.truncate(48);
        self.slots.retain_mut(|s| {
            s.wallpaper = s.wallpaper.trim().chars().take(1024).collect();
            s.monitor = s.monitor.filter(|&m| m < 16);
            if let Some(mins) = crate::playlist::parse_hhmm(&s.start) {
                s.start = format!("{:02}:{:02}", mins / 60, mins % 60);
                !s.wallpaper.is_empty()
            } else {
                false
            }
        });
        self
    }
}

fn default_screensaver_minutes() -> u32 {
    5
}

/// Proteção de tela viva no Windows (entra em tela cheia após tempo ocioso).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreensaverConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Tempo sem uso de mouse/teclado para ativar (1 a 240 minutos).
    #[serde(default = "default_screensaver_minutes")]
    pub idle_minutes: u32,
    /// Wallpaper opcional exclusivo para a proteção de tela (vazio = mantém o de cada monitor).
    #[serde(default)]
    pub wallpaper: String,
    /// Bloqueia a sessão do Windows (`LockWorkStation`) ao sair da proteção de tela, exigindo PIN/senha.
    #[serde(default)]
    pub lock_on_dismiss: bool,
}

impl Default for ScreensaverConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_minutes: default_screensaver_minutes(),
            wallpaper: String::new(),
            lock_on_dismiss: false,
        }
    }
}

impl ScreensaverConfig {
    pub fn sanitized(mut self) -> Self {
        self.idle_minutes = self.idle_minutes.clamp(1, 240);
        self.wallpaper = self.wallpaper.trim().chars().take(1024).collect();
        self
    }
}

/// Preferências do "Criar com IA". A chave de API nunca fica aqui (Gerenciador de Credenciais).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiConfig {
    /// `claude`, `gemini` ou `openrouter`.
    #[serde(default)]
    pub provider: String,
    /// Último modelo usado em cada provedor.
    #[serde(default)]
    pub models: HashMap<String, String>,
    /// OpenRouter: se o modelo escolhido lê imagens.
    #[serde(default = "default_true")]
    pub openrouter_vision: bool,
    /// Limite de rodadas (chamadas ao modelo) de uma geração.
    #[serde(default = "default_ai_steps")]
    pub max_steps: u32,
    /// Limite de tokens (lidos + escritos) de uma geração. É um limite de USO, não de dinheiro.
    #[serde(default = "default_ai_tokens")]
    pub max_total_tokens: u64,
}

fn default_ai_steps() -> u32 {
    24
}

fn default_ai_tokens() -> u64 {
    1_000_000
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "claude".into(),
            models: HashMap::new(),
            openrouter_vision: true,
            max_steps: default_ai_steps(),
            max_total_tokens: default_ai_tokens(),
        }
    }
}

/// Título digitado pelo usuário: sem caracteres de controle, espaços nas pontas, no máximo 60 caracteres.
/// Vazio (ou só espaços) = sem nome próprio.
pub fn clean_title(raw: &str) -> String {
    let t: String = raw.chars().filter(|c| !c.is_control()).collect();
    t.trim().chars().take(60).collect()
}

impl AiConfig {
    pub const STEPS: std::ops::RangeInclusive<u32> = 1..=100;
    pub const TOKENS: std::ops::RangeInclusive<u64> = 10_000..=10_000_000;

    /// Limites dentro das faixas aceitas (valores do arquivo ou do Studio fora delas são ajustados).
    pub fn sanitized(mut self) -> Self {
        self.max_steps = self.max_steps.clamp(*Self::STEPS.start(), *Self::STEPS.end());
        self.max_total_tokens = self.max_total_tokens.clamp(*Self::TOKENS.start(), *Self::TOKENS.end());
        self
    }

    pub fn limits(&self) -> crate::ai::agent::Limits {
        let c = self.clone().sanitized();
        crate::ai::agent::Limits { max_steps: c.max_steps as usize, max_total_tokens: c.max_total_tokens, ..Default::default() }
    }
}

/// Modo desempenho: consumo mínimo do motor (ver `performance.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PerformanceMode {
    Off,
    /// Ativa sozinho enquanto há um app em tela cheia (jogo).
    #[default]
    Auto,
    Always,
}

impl PerformanceMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub present_mode: PresentMode,
    pub target_fps: u32,
    pub pause_when_covered: bool,
    pub pause_on_battery: bool,
    pub open_studio: bool,
    /// Atribuições pelo índice ATUAL do monitor (usado em tempo de execução e pelo Studio). O disco
    /// guarda a versão por identidade em `monitor_wallpapers_by_key`; este mapa é reconstruído por
    /// [`EngineConfig::resolve_monitors`] a cada enumeração e fica só como cópia legada.
    #[serde(default)]
    pub monitor_wallpapers: HashMap<usize, String>,
    /// Atribuições pela chave persistente do monitor (`desktop::monitors::stable_keys`). Telas
    /// desconectadas continuam aqui, então voltam com a configuração anterior.
    #[serde(default)]
    pub monitor_wallpapers_by_key: HashMap<String, String>,
    /// Já migramos as atribuições por índice para chaves; depois disso, tela sem chave conhecida
    /// começa sem atribuição em vez de herdar a de outro índice.
    #[serde(default)]
    pub monitors_migrated: bool,
    /// Valores de parâmetros `// param:` escolhidos pelo usuário: spec do wallpaper -> nome -> valor.
    /// Os valores são limitados à faixa do shader na hora de usar (`params::resolve`).
    #[serde(default)]
    pub shader_params: HashMap<String, HashMap<String, f32>>,
    /// Nome de exibição escolhido pelo usuário para wallpapers criados/importados (chave: caminho do arquivo).
    /// O arquivo não é renomeado: só o título no Studio e no menu da bandeja.
    #[serde(default)]
    pub wallpaper_titles: HashMap<String, String>,
    /// Chave de cada monitor conectado, na ordem dos índices atuais (não persistido).
    #[serde(skip)]
    monitor_keys: Vec<String>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_tint")]
    pub tint: String,
    #[serde(default = "default_true")]
    pub mouse_enabled: bool,
    #[serde(default = "default_true")]
    pub parallax_enabled: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub playlist: PlaylistConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub screensaver: ScreensaverConfig,
    /// Grava um quadro do wallpaper como wallpaper do Windows (Recorte, etc.) e restaura ao sair.
    #[serde(default = "default_true")]
    pub mirror_system_wallpaper: bool,
    /// Também usa o quadro como imagem da tela de bloqueio. Desligado por padrão: o Windows
    /// não permite voltar ao modo anterior (ex.: Spotlight) por API.
    #[serde(default)]
    pub mirror_lock_screen: bool,
    #[serde(default)]
    pub ai: AiConfig,
    /// Tour de boas-vindas do Studio já visto (ou pulado).
    #[serde(default)]
    pub tour_done: bool,
    /// Mostra uma janela de console com o log (desligado por padrão).
    #[serde(default)]
    pub show_console: bool,
    #[serde(default)]
    pub performance_mode: PerformanceMode,
}

impl Default for EngineConfig {
    fn default() -> Self {
        let mut monitor_wallpapers = HashMap::new();
        monitor_wallpapers.insert(0, "matrix".to_string());
        monitor_wallpapers.insert(1, "coding".to_string());
        monitor_wallpapers.insert(2, "matrix".to_string());

        Self {
            present_mode: PresentMode::Composition,
            target_fps: 30,
            pause_when_covered: true,
            pause_on_battery: false,
            open_studio: true,
            monitor_wallpapers,
            monitor_wallpapers_by_key: HashMap::new(),
            monitors_migrated: false,
            shader_params: HashMap::new(),
            wallpaper_titles: HashMap::new(),
            monitor_keys: Vec::new(),
            speed: 1.0,
            tint: default_tint(),
            mouse_enabled: true,
            parallax_enabled: true,
            autostart: false,
            playlist: PlaylistConfig::default(),
            schedule: ScheduleConfig::default(),
            screensaver: ScreensaverConfig::default(),
            mirror_system_wallpaper: true,
            mirror_lock_screen: false,
            ai: AiConfig::default(),
            tour_done: false,
            show_console: false,
            performance_mode: PerformanceMode::Auto,
        }
    }
}

impl EngineConfig {
    /// Carrega a configuração do disco (ou gera a padrão) e aplica argumentos de linha de comando.
    pub fn load_and_merge() -> Self {
        let mut config = Self::load().unwrap_or_default();
        config.apply_args(std::env::args().skip(1));
        let _ = config.save();
        info!(
            mode = ?config.present_mode,
            target_fps = config.target_fps,
            pause_when_covered = config.pause_when_covered,
            pause_on_battery = config.pause_on_battery,
            "Configuração consolidada carregada"
        );
        config
    }

    /// Atalho para carregar do disco e mesclar com argumentos CLI.
    pub fn from_args() -> Self {
        Self::load_and_merge()
    }


    /// Retorna o caminho de armazenamento do arquivo de configuração (%LOCALAPPDATA%\IsNotAEngine\config.toml).
    pub fn config_path() -> PathBuf {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let dir = Path::new(&local_app_data).join("IsNotAEngine");
            let _ = std::fs::create_dir_all(&dir);
            dir.join("config.toml")
        } else {
            PathBuf::from("config.toml")
        }
    }

    /// Lê a configuração de `config.toml`.
    pub fn load() -> Option<Self> {
        let path = Self::config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match toml::from_str::<Self>(&content) {
                    Ok(cfg) => {
                        info!(path = %path.display(), "Configuração carregada do disco");
                        return Some(cfg);
                    }
                    Err(e) => {
                        // O padrão será salvo por cima; guarda o original para recuperação manual.
                        let backup = path.with_extension("toml.invalid");
                        if !backup.exists() {
                            let _ = std::fs::copy(&path, &backup);
                        }
                        warn!(path = %path.display(), backup = %backup.display(), error = %e, "Erro ao decodificar config.toml; usando padrão");
                    }
                },
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Erro ao ler config.toml; usando padrão");
                }
            }
        }
        None
    }

    /// Salva a configuração atual em `config.toml` (grava num temporário e renomeia, para uma
    /// queda no meio não deixar o arquivo pela metade).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        let mut snapshot = self.clone();
        snapshot.sync_monitor_keys();
        if let Ok(content) = toml::to_string_pretty(&snapshot) {
            let tmp = path.with_extension("toml.tmp");
            std::fs::write(&tmp, content)?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(())
    }

    /// Padrões de fábrica, preservando o tour já visto e as preferências da IA.
    pub fn reset_to_defaults(&mut self) {
        let (tour_done, ai) = (self.tour_done, self.ai.clone());
        *self = Self::default();
        self.tour_done = tour_done;
        self.ai = ai;
    }

    /// Guarda um valor de parâmetro. `false` (e nada muda) se o spec, o nome ou o valor forem
    /// inválidos, ou se passaria dos limites (64 wallpapers, 8 parâmetros cada).
    pub fn set_shader_param(&mut self, spec: &str, name: &str, value: f32) -> bool {
        use crate::render::params::{MAX_PARAMS, valid_name};
        if spec.is_empty() || spec.len() > 1024 || !valid_name(name) || !value.is_finite() {
            return false;
        }
        if !self.shader_params.contains_key(spec) && self.shader_params.len() >= 64 {
            return false;
        }
        let entry = self.shader_params.entry(spec.to_string()).or_default();
        if !entry.contains_key(name) && entry.len() >= MAX_PARAMS {
            return false;
        }
        entry.insert(name.to_string(), value);
        true
    }

    /// Volta os parâmetros de um wallpaper aos padrões do shader.
    pub fn reset_shader_params(&mut self, spec: &str) {
        self.shader_params.remove(spec);
    }

    /// Os valores do usuário para um wallpaper, em pares (ordem estável).
    pub fn shader_param_overrides(&self, spec: &str) -> Vec<(String, f32)> {
        let mut v: Vec<_> = self.shader_params.get(spec).map(|m| m.iter().map(|(k, x)| (k.clone(), *x)).collect()).unwrap_or_default();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Liga as atribuições aos monitores conectados. `keys[i]` é a chave do monitor de índice `i`.
    ///
    /// Na primeira vez (`monitors_migrated == false`) as atribuições por índice do arquivo antigo
    /// viram atribuições por chave. Depois, a chave manda: trocar a ordem de enumeração não troca
    /// os wallpapers, e uma tela nova (sem chave conhecida) fica sem atribuição.
    pub fn resolve_monitors(&mut self, keys: &[String]) {
        // Primeiro guarda o que mudou por índice na enumeração ANTERIOR; sem isso, o mapa por chave
        // (desatualizado) sobrescreveria uma troca de wallpaper feita depois da última resolução.
        self.sync_monitor_keys();
        for (id, key) in keys.iter().enumerate() {
            if let Some(spec) = self.monitor_wallpapers_by_key.get(key) {
                self.monitor_wallpapers.insert(id, spec.clone());
            } else if !self.monitors_migrated {
                if let Some(spec) = self.monitor_wallpapers.get(&id) {
                    self.monitor_wallpapers_by_key.insert(key.clone(), spec.clone());
                }
            } else {
                self.monitor_wallpapers.remove(&id);
            }
        }
        self.monitor_wallpapers.retain(|id, _| *id < keys.len());
        if !keys.is_empty() {
            self.monitors_migrated = true;
        }
        self.monitor_keys = keys.to_vec();
    }

    /// Copia as atribuições por índice para o mapa por chave (chamado antes de salvar).
    fn sync_monitor_keys(&mut self) {
        for (id, key) in self.monitor_keys.iter().enumerate() {
            if let Some(spec) = self.monitor_wallpapers.get(&id) {
                self.monitor_wallpapers_by_key.insert(key.clone(), spec.clone());
            }
        }
    }

    /// Aplica argumentos CLI sobre a configuração existente.
    pub fn apply_args<I, S>(&mut self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for raw in args {
            let arg = raw.as_ref();
            match arg.split_once('=') {
                Some(("--mode", "hwnd")) => self.present_mode = PresentMode::Hwnd,
                Some(("--mode", "dcomp")) => self.present_mode = PresentMode::Composition,
                Some(("--fps", n)) => {
                    if let Ok(fps) = n.parse::<u32>() {
                        self.target_fps = fps.clamp(1, 240);
                    }
                }
                Some(("--pause-covered", val)) => {
                    self.pause_when_covered = val != "false" && val != "0";
                }
                Some(("--pause-battery", val)) => {
                    self.pause_on_battery = val == "true" || val == "1";
                }
                _ => match arg {
                    "--no-pause-covered" => self.pause_when_covered = false,
                    "--studio" | "--open-studio" => self.open_studio = true,
                    "--no-studio" => self.open_studio = false,
                    _ => {}
                },
            }
        }
    }

    #[cfg(test)]
    pub fn from_iter<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut config = Self::default();
        config.apply_args(args);
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parsing_defaults() {
        let config = EngineConfig::from_iter(Vec::<String>::new());
        assert_eq!(config.target_fps, 30);
        assert_eq!(config.present_mode, PresentMode::Composition);
        assert!(config.pause_when_covered);
        assert!(!config.pause_on_battery);
        assert!(config.open_studio);
    }

    #[test]
    fn test_config_parsing_overrides() {
        let args = vec![
            "--mode=hwnd",
            "--fps=60",
            "--pause-battery=true",
            "--no-pause-covered",
            "--no-studio",
        ];
        let config = EngineConfig::from_iter(args);
        assert_eq!(config.target_fps, 60);
        assert_eq!(config.present_mode, PresentMode::Hwnd);
        assert!(!config.pause_when_covered);
        assert!(config.pause_on_battery);
        assert!(!config.open_studio);
    }

    #[test]
    fn playlist_values_are_clamped() {
        let p = PlaylistConfig { interval_minutes: 0, transition_ms: 99_999, ..Default::default() }.sanitized();
        assert_eq!((p.interval_minutes, p.transition_ms), (1, 5000));
    }

    #[test]
    fn old_config_without_playlist_still_loads() {
        let cfg: EngineConfig = toml::from_str(
            "present_mode = \"composition\"
target_fps = 30
pause_when_covered = true
pause_on_battery = false
open_studio = true
",
        )
        .expect("config antiga deve carregar");
        assert!(!cfg.playlist.enabled);
        assert_eq!(cfg.playlist.interval_minutes, 10);
    }

    #[test]
    fn test_config_toml_roundtrip() {
        let config = EngineConfig::default();
        let serialized = toml::to_string_pretty(&config).expect("deve serializar");
        let deserialized: EngineConfig = toml::from_str(&serialized).expect("deve deserializar");
        assert_eq!(config.target_fps, deserialized.target_fps);
        assert_eq!(config.present_mode, deserialized.present_mode);
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn legacy_index_assignments_migrate_to_keys_once() {
        let mut c = EngineConfig { monitor_wallpapers: HashMap::from([(0, "a".to_string()), (1, "b".to_string())]), ..Default::default() };
        c.resolve_monitors(&keys(&["X", "Y"]));
        assert_eq!(c.monitor_wallpapers_by_key["X"], "a");
        assert_eq!(c.monitor_wallpapers_by_key["Y"], "b");
        assert!(c.monitors_migrated);
    }

    #[test]
    fn reordering_screens_keeps_each_screens_wallpaper() {
        let mut c = EngineConfig { monitor_wallpapers: HashMap::from([(0, "a".to_string()), (1, "b".to_string())]), ..Default::default() };
        c.resolve_monitors(&keys(&["X", "Y"]));
        c.resolve_monitors(&keys(&["Y", "X"]));
        assert_eq!(c.monitor_wallpapers[&0], "b");
        assert_eq!(c.monitor_wallpapers[&1], "a");
    }

    #[test]
    fn unplugged_screen_keeps_its_config_and_new_screen_starts_empty() {
        let mut c = EngineConfig { monitor_wallpapers: HashMap::from([(0, "a".to_string()), (1, "b".to_string())]), ..Default::default() };
        c.resolve_monitors(&keys(&["X", "Y"]));
        c.resolve_monitors(&keys(&["X"]));
        assert_eq!(c.monitor_wallpapers_by_key["Y"], "b", "desconectar não apaga");
        assert!(!c.monitor_wallpapers.contains_key(&1));
        c.resolve_monitors(&keys(&["X", "Z"]));
        assert!(!c.monitor_wallpapers.contains_key(&1), "tela nova não herda o índice 1");
        c.resolve_monitors(&keys(&["Y", "X"]));
        assert_eq!(c.monitor_wallpapers[&0], "b", "Y voltou com a configuração anterior");
    }

    #[test]
    fn wallpaper_changed_after_a_resolve_survives_the_next_resolve() {
        // Regressão: trocar o wallpaper e depois um evento de display (nova resolução) revertia a troca.
        let mut c = EngineConfig::default();
        c.resolve_monitors(&keys(&["X", "Y"]));
        c.monitor_wallpapers.insert(1, "outro_wallpaper".to_string());
        c.resolve_monitors(&keys(&["X", "Y"]));
        assert_eq!(c.monitor_wallpapers[&1], "outro_wallpaper");
        // E com a ordem trocada, a troca acompanha a tela.
        c.resolve_monitors(&keys(&["Y", "X"]));
        assert_eq!(c.monitor_wallpapers[&0], "outro_wallpaper");
    }

    #[test]
    fn changes_by_index_are_saved_under_the_screen_key() {
        let mut c = EngineConfig::default();
        c.resolve_monitors(&keys(&["X", "Y"]));
        c.monitor_wallpapers.insert(1, "novo".to_string());
        c.sync_monitor_keys();
        assert_eq!(c.monitor_wallpapers_by_key["Y"], "novo");
    }

    #[test]
    fn keys_survive_toml_roundtrip_and_old_files_load() {
        let mut c = EngineConfig::default();
        c.monitor_wallpapers_by_key.insert(r"hw:\\?\display#sam707d#7&2f#{e6f0}".to_string(), "matrix".to_string());
        c.monitors_migrated = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: EngineConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.monitor_wallpapers_by_key, c.monitor_wallpapers_by_key);
        assert!(back.monitors_migrated);
        let old: EngineConfig = toml::from_str(
            "present_mode = \"composition\"\ntarget_fps = 30\npause_when_covered = true\npause_on_battery = false\nopen_studio = true\n[monitor_wallpapers]\n0 = \"matrix\"\n",
        )
        .unwrap();
        assert!(!old.monitors_migrated && old.monitor_wallpapers_by_key.is_empty());
    }

    #[test]
    fn old_files_with_remote_site_grants_still_load() {
        // `web_grants` existia quando wallpapers por URL eram permitidos; o campo é ignorado agora.
        let old: EngineConfig = toml::from_str("present_mode = \"composition\"\ntarget_fps = 30\npause_when_covered = true\npause_on_battery = false\nopen_studio = true\n[web_grants.\"https://a.com\"]\naudio = true\n").unwrap();
        assert_eq!(old.target_fps, 30);
    }

    #[test]
    fn reset_to_defaults_keeps_tour_and_ai_prefs_only() {
        let mut c = EngineConfig { tour_done: true, speed: 2.5, autostart: true, ..Default::default() };
        c.ai.provider = "gemini".into();
        c.reset_to_defaults();
        let d = EngineConfig::default();
        assert!(c.tour_done && c.ai.provider == "gemini");
        assert_eq!((c.speed, c.autostart), (d.speed, d.autostart));
    }

    #[test]
    fn shader_params_validate_cap_and_roundtrip() {
        let mut c = EngineConfig::default();
        assert!(c.set_shader_param("aurora", "intensity", 1.2));
        assert!(!c.set_shader_param("", "x", 1.0));
        assert!(!c.set_shader_param("aurora", "nome inválido!", 1.0));
        assert!(!c.set_shader_param("aurora", "x", f32::NAN));
        for i in 0..8 {
            c.set_shader_param("aurora", &format!("p{i}"), 1.0);
        }
        assert!(!c.set_shader_param("aurora", "extra", 1.0), "no máximo 8 por wallpaper");
        for i in 0..70 {
            c.set_shader_param(&format!("w{i}"), "a", 1.0);
        }
        assert!(c.shader_params.len() <= 64);
        let back: EngineConfig = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back.shader_params["aurora"]["intensity"], 1.2);
        c.reset_shader_params("aurora");
        assert!(c.shader_param_overrides("aurora").is_empty());
    }

    #[test]
    fn ai_limits_have_safe_defaults_clamp_and_load_from_old_files() {
        let c = AiConfig::default();
        assert_eq!((c.max_steps, c.max_total_tokens), (24, 1_000_000));
        let wild = AiConfig { max_steps: 100_000, max_total_tokens: 1, ..AiConfig::default() }.sanitized();
        assert_eq!((wild.max_steps, wild.max_total_tokens), (100, 10_000));
        let l = AiConfig { max_steps: 0, ..AiConfig::default() }.limits();
        assert_eq!(l.max_steps, 1, "nunca zero rodadas");
        // arquivo antigo (sem os campos novos) carrega com os padrões
        let old: AiConfig = toml::from_str("provider = \"claude\"\n").unwrap();
        assert_eq!((old.max_steps, old.max_total_tokens), (24, 1_000_000));
    }

    #[test]
    fn performance_mode_defaults_to_auto_for_old_files_and_roundtrips() {
        let old: EngineConfig = toml::from_str("present_mode = \"composition\"
target_fps = 30
pause_when_covered = true
pause_on_battery = false
open_studio = true
").unwrap();
        assert_eq!(old.performance_mode, PerformanceMode::Auto);
        let c = EngineConfig { performance_mode: PerformanceMode::Always, ..Default::default() };
        let back: EngineConfig = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back.performance_mode, PerformanceMode::Always);
        for m in [PerformanceMode::Off, PerformanceMode::Auto, PerformanceMode::Always] {
            assert_eq!(PerformanceMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(PerformanceMode::parse("x"), None);
    }

    #[test]
    fn typed_titles_are_trimmed_cleaned_and_capped() {
        assert_eq!(clean_title("  Meu céu \n\t"), "Meu céu");
        assert_eq!(clean_title("   "), "");
        assert_eq!(clean_title(&"x".repeat(200)).chars().count(), 60);
        let c = EngineConfig::default();
        assert!(c.wallpaper_titles.is_empty());
    }

    #[test]
    fn schedule_screensaver_and_monitor_playlists_sanitize_and_roundtrip() {
        let mut c = EngineConfig::default();
        c.playlist.monitor_items.insert(0, vec!["tokyo_rain".into(), "winter_cabin".into()]);
        c.playlist.monitor_items.insert(99, vec!["ignored".into()]);
        c.playlist = c.playlist.sanitized();
        assert!(c.playlist.monitor_items.contains_key(&0));
        assert!(!c.playlist.monitor_items.contains_key(&99));

        c.schedule = ScheduleConfig {
            enabled: true,
            slots: vec![
                ScheduleSlot { start: "7:05".into(), wallpaper: "coding".into(), monitor: None },
                ScheduleSlot { start: "99:99".into(), wallpaper: "invalid".into(), monitor: None },
                ScheduleSlot { start: "19:30".into(), wallpaper: "tokyo_rain".into(), monitor: Some(1) },
            ],
        }
        .sanitized();
        assert_eq!(c.schedule.slots.len(), 2);
        assert_eq!(c.schedule.slots[0].start, "07:05");
        assert_eq!(c.schedule.slots[1].monitor, Some(1));

        c.screensaver = ScreensaverConfig {
            enabled: true,
            idle_minutes: 0,
            wallpaper: "  winter_cabin  ".into(),
            lock_on_dismiss: true,
        }
        .sanitized();
        assert_eq!(c.screensaver.idle_minutes, 1);
        assert_eq!(c.screensaver.wallpaper, "winter_cabin");
        assert!(c.screensaver.lock_on_dismiss);

        let toml_str = toml::to_string_pretty(&c).expect("deve serializar");
        let back: EngineConfig = toml::from_str(&toml_str).expect("deve deserializar");
        assert_eq!(back.schedule, c.schedule);
        assert_eq!(back.screensaver, c.screensaver);
        assert_eq!(back.playlist.monitor_items, c.playlist.monitor_items);
    }
}
