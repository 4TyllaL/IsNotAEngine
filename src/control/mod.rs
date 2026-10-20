//! Canal de controle local: named pipe `\\.\pipe\IsNotAEngine.Control.v1`.
//!
//! Protocolo: uma mensagem JSON por linha (UTF-8, `\n`). Docks, widgets e scripts do
//! próprio usuário usam o pipe para ler o estado e enviar comandos. Detalhes em
//! `docs/CONTROL_PIPE.md`. Este módulo só define o protocolo (sem Win32, testável);
//! o transporte fica em `pipe.rs`.

mod pipe;

pub use pipe::{ControlServer, PIPE_NAME};

use serde::{Deserialize, Serialize};

use crate::engine::EngineCommand;

/// Versão do protocolo (também no nome do pipe). Só muda em quebra de compatibilidade.
pub const PROTOCOL_VERSION: u32 = 1;

/// Tamanho máximo de uma linha de requisição; maior que isso derruba a conexão.
pub const MAX_LINE: usize = 4096;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Estado atual (resposta única).
    State,
    /// Catálogo embutido mais os wallpapers importados; cada `id` serve em `set_wallpaper`.
    ListWallpapers,
    /// Muda um ajuste. Nomes e faixas em `docs/CONTROL_PIPE.md`; valor fora da faixa é recusado.
    SetSetting {
        name: String,
        value: serde_json::Value,
    },
    /// Responde `ok` e passa a empurrar `{"event":"state",...}` a cada mudança.
    /// A conexão vira só de eventos: use outra para enviar comandos.
    Subscribe,
    Pause,
    Resume,
    TogglePause,
    NextWallpaper,
    OpenStudio,
    /// `monitor` ausente aplica a todos os monitores.
    SetWallpaper {
        monitor: Option<usize>,
        wallpaper: String,
    },
}

impl Request {
    /// Comando de engine equivalente; `Ok(None)` para pedidos que só leem estado e `Err` com o
    /// motivo quando o pedido é válido como JSON mas o valor não é aceito.
    pub fn into_command(self) -> Result<Option<EngineCommand>, String> {
        Ok(Some(match self {
            Request::State | Request::Subscribe | Request::ListWallpapers => return Ok(None),
            Request::SetSetting { name, value } => return setting_command(&name, &value).map(Some),
            Request::Pause => EngineCommand::SetPaused(true),
            Request::Resume => EngineCommand::SetPaused(false),
            Request::TogglePause => EngineCommand::TogglePause,
            Request::NextWallpaper => EngineCommand::NextWallpaper,
            Request::OpenStudio => EngineCommand::OpenStudio,
            Request::SetWallpaper { monitor, wallpaper } => {
                EngineCommand::ControlSetWallpaper(monitor, wallpaper)
            }
        }))
    }
}

/// Faixas aceitas. O Studio não valida: um FPS 0 dividiria por zero no timer do loop.
const FPS_RANGE: std::ops::RangeInclusive<u64> = 5..=240;
const SPEED_RANGE: std::ops::RangeInclusive<f64> = 0.1..=3.0;
const PLAYLIST_MINUTES: std::ops::RangeInclusive<u64> = 1..=24 * 60;

/// Valida `value` para o ajuste `name` e monta o comando da engine.
fn setting_command(name: &str, value: &serde_json::Value) -> Result<EngineCommand, String> {
    let flag = || {
        value
            .as_bool()
            .ok_or_else(|| format!("`{name}` espera true ou false"))
    };
    let patch = |patch: PlaylistPatch| Ok(EngineCommand::PatchPlaylist(patch));
    match name {
        "fps" => match value.as_u64().filter(|v| FPS_RANGE.contains(v)) {
            Some(fps) => Ok(EngineCommand::SetFps(fps as u32)),
            None => Err(format!(
                "`fps` espera um inteiro de {} a {}",
                FPS_RANGE.start(),
                FPS_RANGE.end()
            )),
        },
        "speed" => match value.as_f64().filter(|v| SPEED_RANGE.contains(v)) {
            Some(speed) => Ok(EngineCommand::SetSpeed(speed as f32)),
            None => Err(format!(
                "`speed` espera um número de {} a {}",
                SPEED_RANGE.start(),
                SPEED_RANGE.end()
            )),
        },
        "pause_covered" => Ok(EngineCommand::SetPauseCovered(flag()?)),
        "pause_battery" => Ok(EngineCommand::SetPauseBattery(flag()?)),
        "mouse" => Ok(EngineCommand::SetMouseEnabled(flag()?)),
        "parallax" => Ok(EngineCommand::SetParallaxEnabled(flag()?)),
        "playlist_enabled" => patch(PlaylistPatch {
            enabled: Some(flag()?),
            ..Default::default()
        }),
        "playlist_shuffle" => patch(PlaylistPatch {
            shuffle: Some(flag()?),
            ..Default::default()
        }),
        "playlist_minutes" => match value.as_u64().filter(|v| PLAYLIST_MINUTES.contains(v)) {
            Some(minutes) => patch(PlaylistPatch {
                minutes: Some(minutes as u32),
                ..Default::default()
            }),
            None => Err(format!(
                "`playlist_minutes` espera um inteiro de {} a {}",
                PLAYLIST_MINUTES.start(),
                PLAYLIST_MINUTES.end()
            )),
        },
        _ => Err(format!("ajuste desconhecido: `{name}`")),
    }
}

/// Mudança parcial da playlist: só os campos presentes são alterados.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PlaylistPatch {
    pub enabled: Option<bool>,
    pub minutes: Option<u32>,
    pub shuffle: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorState {
    pub id: usize,
    pub wallpaper: String,
}

/// Ajustes que o pipe lê e altera (`set_setting`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Settings {
    pub fps: u32,
    pub speed: f32,
    pub pause_covered: bool,
    pub pause_battery: bool,
    pub mouse: bool,
    pub parallax: bool,
    pub playlist_enabled: bool,
    pub playlist_minutes: u32,
    pub playlist_shuffle: bool,
}

/// Estado publicado aos clientes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateSnapshot {
    pub v: u32,
    /// Pausa manual (bandeja, Studio ou pipe).
    pub paused: bool,
    /// Pausa automática (desktop coberto ou bateria).
    pub auto_paused: bool,
    #[serde(flatten)]
    pub settings: Settings,
    pub monitors: Vec<MonitorState>,
}

impl StateSnapshot {
    pub fn new(
        paused: bool,
        auto_paused: bool,
        settings: Settings,
        wallpapers: &std::collections::HashMap<usize, String>,
    ) -> Self {
        let mut monitors: Vec<MonitorState> = wallpapers
            .iter()
            .map(|(id, w)| MonitorState {
                id: *id,
                wallpaper: w.clone(),
            })
            .collect();
        monitors.sort_by_key(|m| m.id);
        Self {
            v: PROTOCOL_VERSION,
            paused,
            auto_paused,
            settings,
            monitors,
        }
    }
}

/// Quantos wallpapers importados a lista devolve, no máximo (mantém a resposta pequena).
const MAX_IMPORTED: usize = 500;

#[derive(Debug, Serialize, PartialEq)]
pub struct WallpaperItem {
    /// Id do catálogo ou caminho completo do arquivo importado: vale em `set_wallpaper`.
    pub id: String,
    pub title: String,
    pub category: String,
    /// `shader`, `web`, `video` ou `livewallpaper`.
    pub kind: &'static str,
    pub audio: bool,
    /// `catalog` ou `imported`.
    pub source: &'static str,
}

fn imported_kind(ext: &str) -> Option<&'static str> {
    match ext {
        "hlsl" => Some("shader"),
        "html" | "htm" => Some("web"),
        "mp4" | "webm" | "wmv" | "mov" | "avi" | "mkv" => Some("video"),
        "livewallpaper" => Some("livewallpaper"),
        "png" | "jpg" | "jpeg" | "bmp" | "webp" | "tif" | "tiff" => Some("image"),
        _ => None,
    }
}

/// Catálogo embutido seguido dos arquivos de `imported_dir` (ordem alfabética).
pub fn list_wallpapers(imported_dir: &std::path::Path) -> Vec<WallpaperItem> {
    use crate::wallpaper::catalog::{CATALOG, Kind};
    let mut items: Vec<WallpaperItem> = CATALOG
        .iter()
        .map(|e| WallpaperItem {
            id: e.id.to_owned(),
            title: e.title.to_owned(),
            category: e.category.to_owned(),
            kind: if e.kind == Kind::Shader {
                "shader"
            } else {
                "web"
            },
            audio: e.audio,
            source: "catalog",
        })
        .collect();

    let mut imported: Vec<WallpaperItem> = std::fs::read_dir(imported_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            Some(WallpaperItem {
                id: path.to_string_lossy().into_owned(),
                title: path.file_stem()?.to_string_lossy().into_owned(),
                category: "Importado".to_owned(),
                kind: imported_kind(&ext)?,
                audio: false,
                source: "imported",
            })
        })
        .collect();
    imported.sort_by_cached_key(|i| i.title.to_lowercase());
    imported.truncate(MAX_IMPORTED);
    items.extend(imported);
    items
}

pub fn response_wallpapers(items: &[WallpaperItem]) -> String {
    let json = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
    format!(
        "{{\"ok\":true,\"wallpapers\":{json}}}
"
    )
}

pub fn response_ok(state_json: &str) -> String {
    format!("{{\"ok\":true,\"state\":{state_json}}}\n")
}

pub fn response_err(message: &str) -> String {
    let msg = serde_json::to_string(message).unwrap_or_else(|_| "\"erro\"".into());
    format!("{{\"ok\":false,\"error\":{msg}}}\n")
}

pub fn event_line(state_json: &str) -> String {
    format!("{{\"event\":\"state\",\"state\":{state_json}}}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parses_simple_commands() {
        let r: Request = serde_json::from_str(r#"{"cmd":"toggle_pause"}"#).unwrap();
        assert_eq!(r, Request::TogglePause);
        let r: Request = serde_json::from_str(r#"{"cmd":"subscribe"}"#).unwrap();
        assert_eq!(r, Request::Subscribe);
    }

    #[test]
    fn parses_set_wallpaper_with_and_without_monitor() {
        let r: Request =
            serde_json::from_str(r#"{"cmd":"set_wallpaper","monitor":1,"wallpaper":"matrix"}"#)
                .unwrap();
        assert_eq!(
            r,
            Request::SetWallpaper {
                monitor: Some(1),
                wallpaper: "matrix".into()
            }
        );
        let r: Request =
            serde_json::from_str(r#"{"cmd":"set_wallpaper","wallpaper":"coding"}"#).unwrap();
        assert_eq!(
            r,
            Request::SetWallpaper {
                monitor: None,
                wallpaper: "coding".into()
            }
        );
    }

    #[test]
    fn rejects_unknown_and_malformed() {
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"format_c"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"set_wallpaper"}"#).is_err());
        assert!(serde_json::from_str::<Request>("nao e json").is_err());
    }

    #[test]
    fn lists_catalog_then_sorted_imported_files() {
        let dir = std::env::temp_dir().join("isnotaengine_list_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["zeta.mp4", "Alfa.hlsl", "notes.txt", "page.HTML"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        std::fs::create_dir_all(dir.join("subpasta.mp4")).unwrap();

        let items = list_wallpapers(&dir);
        let catalog = crate::wallpaper::catalog::CATALOG.len();
        assert!(items[..catalog].iter().all(|i| i.source == "catalog"));
        let imported: Vec<(&str, &str)> = items[catalog..]
            .iter()
            .map(|i| (i.title.as_str(), i.kind))
            .collect();
        assert_eq!(
            imported,
            [("Alfa", "shader"), ("page", "web"), ("zeta", "video")]
        );
        assert!(items[catalog].id.ends_with("Alfa.hlsl"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_folder_lists_only_the_catalog() {
        let items = list_wallpapers(std::path::Path::new("Z:/nao/existe"));
        assert_eq!(items.len(), crate::wallpaper::catalog::CATALOG.len());
    }

    fn sample_settings() -> Settings {
        Settings {
            fps: 60,
            speed: 1.0,
            pause_covered: true,
            pause_battery: false,
            mouse: true,
            parallax: true,
            playlist_enabled: false,
            playlist_minutes: 10,
            playlist_shuffle: false,
        }
    }

    #[test]
    fn read_only_requests_have_no_command() {
        assert!(matches!(Request::ListWallpapers.into_command(), Ok(None)));
        assert!(matches!(Request::State.into_command(), Ok(None)));
        assert!(matches!(Request::Subscribe.into_command(), Ok(None)));
        assert!(matches!(Request::Pause.into_command(), Ok(Some(_))));
    }

    fn setting(json: &str) -> Result<Option<EngineCommand>, String> {
        serde_json::from_str::<Request>(json)
            .unwrap()
            .into_command()
    }

    #[test]
    fn settings_accept_values_in_range() {
        assert!(matches!(
            setting(r#"{"cmd":"set_setting","name":"fps","value":30}"#),
            Ok(Some(EngineCommand::SetFps(30)))
        ));
        assert!(
            matches!(setting(r#"{"cmd":"set_setting","name":"speed","value":1.5}"#), Ok(Some(EngineCommand::SetSpeed(s))) if s == 1.5)
        );
        assert!(matches!(
            setting(r#"{"cmd":"set_setting","name":"mouse","value":false}"#),
            Ok(Some(EngineCommand::SetMouseEnabled(false)))
        ));
        assert!(matches!(
            setting(r#"{"cmd":"set_setting","name":"playlist_minutes","value":15}"#),
            Ok(Some(EngineCommand::PatchPlaylist(PlaylistPatch {
                minutes: Some(15),
                enabled: None,
                shuffle: None
            })))
        ));
    }

    #[test]
    fn settings_refuse_bad_values_and_names() {
        for json in [
            r#"{"cmd":"set_setting","name":"fps","value":0}"#,
            r#"{"cmd":"set_setting","name":"fps","value":1000}"#,
            r#"{"cmd":"set_setting","name":"fps","value":"60"}"#,
            r#"{"cmd":"set_setting","name":"fps","value":30.5}"#,
            r#"{"cmd":"set_setting","name":"speed","value":0}"#,
            r#"{"cmd":"set_setting","name":"speed","value":99}"#,
            r#"{"cmd":"set_setting","name":"mouse","value":1}"#,
            r#"{"cmd":"set_setting","name":"playlist_minutes","value":0}"#,
            r#"{"cmd":"set_setting","name":"playlist_minutes","value":2000}"#,
            r#"{"cmd":"set_setting","name":"autostart","value":true}"#,
        ] {
            assert!(setting(json).is_err(), "{json}");
        }
    }

    #[test]
    fn state_orders_monitors_and_serializes() {
        let mut w = HashMap::new();
        w.insert(1, "coding".to_string());
        w.insert(0, "matrix".to_string());
        let s = StateSnapshot::new(true, false, sample_settings(), &w);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"v":1,"paused":true,"auto_paused":false,"fps":60,"speed":1.0,"pause_covered":true,"pause_battery":false,"mouse":true,"parallax":true,"playlist_enabled":false,"playlist_minutes":10,"playlist_shuffle":false,"monitors":[{"id":0,"wallpaper":"matrix"},{"id":1,"wallpaper":"coding"}]}"#
        );
    }

    #[test]
    fn error_response_escapes_message() {
        assert_eq!(
            response_err("a\"b"),
            "{\"ok\":false,\"error\":\"a\\\"b\"}\n"
        );
    }
}
