//! Vídeos de fábrica: wallpapers em vídeo que acompanham o app (não ficam dentro do executável).
//!
//! Onde ficam: a pasta `videos` ao lado do executável (instalador e pacote MSIX colocam lá) e, no
//! desenvolvimento, `assets/wallpapers/videos` relativo ao diretório atual. Cada vídeo pode ter um
//! `<nome>.json` ao lado com `title`, `category` e `description`; sem ele, o título vem do nome do
//! arquivo e a categoria é "Ambient".
//!
//! Spec estável: `video:<nome>` (sem extensão). A configuração guarda isso, não o caminho, então o
//! wallpaper continua valendo quando o app muda de pasta (outra versão, instalação, pacote).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "video:";
const EXTENSIONS: [&str; 6] = ["mp4", "webm", "mov", "mkv", "wmv", "avi"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FactoryVideo {
    /// `video:<nome>`: o que vai para a configuração e para o motor.
    pub id: String,
    pub title: String,
    pub category: String,
    pub description: String,
    /// Caminho do arquivo (para a prévia do Studio).
    pub file: String,
}

#[derive(Debug, Default, Deserialize)]
struct Meta {
    title: Option<String>,
    category: Option<String>,
    description: Option<String>,
}

/// Pastas onde procurar, em ordem de prioridade.
pub fn dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        v.push(exe_dir.join("videos"));
    }
    v.push(PathBuf::from("assets/wallpapers/videos"));
    v
}

/// Nome aceito num spec: o nome do arquivo sem extensão. Pode ter espaços, acentos e parênteses ("A Chuva (1)"),
/// mas nunca separadores de caminho, `:`, caracteres de controle nem começar com ponto: o spec não vira caminho.
fn valid_name(name: &str) -> bool {
    let bad = |c: char| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
    !name.is_empty()
        && name.chars().count() <= 80
        && !name.starts_with('.')
        && name.trim() == name
        && !name.chars().any(bad)
}

fn is_video(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn pretty(name: &str) -> String {
    // "A Ruina (1)" (cópia do navegador) vira "A Ruina".
    let name = match name.rfind(" (") {
        Some(i) if name.ends_with(')') && name[i + 2..name.len() - 1].chars().all(|c| c.is_ascii_digit()) => &name[..i],
        _ => name,
    };
    let words: Vec<String> = name
        .split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect();
    words.join(" ")
}

/// Vídeos de fábrica encontrados em `dirs` (o primeiro com um nome vence), em ordem alfabética.
pub fn list_in(dirs: &[PathBuf]) -> Vec<FactoryVideo> {
    let mut out: Vec<FactoryVideo> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && is_video(p)).collect();
        files.sort();
        for path in files {
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).filter(|n| valid_name(n)) else { continue };
            let id = format!("{PREFIX}{name}");
            if out.iter().any(|v| v.id == id) {
                continue;
            }
            let meta: Meta = std::fs::read_to_string(path.with_extension("json"))
                .ok()
                // O Bloco de Notas pode gravar com BOM, que o JSON não aceita.
                .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
                .unwrap_or_default();
            let clean = |s: Option<String>, max: usize| s.map(|t| t.chars().filter(|c| !c.is_control()).take(max).collect::<String>()).filter(|t| !t.trim().is_empty());
            out.push(FactoryVideo {
                title: clean(meta.title, 60).unwrap_or_else(|| pretty(name)),
                category: clean(meta.category, 30).unwrap_or_else(|| "Ambient".into()),
                description: clean(meta.description, 300).unwrap_or_else(|| "Vídeo em loop.".into()),
                file: path.to_string_lossy().into_owned(),
                id,
            });
        }
    }
    out.sort_by_key(|v| v.title.to_lowercase());
    out
}

pub fn list() -> Vec<FactoryVideo> {
    list_in(&dirs())
}

/// Arquivo de um spec `video:<nome>` nas `dirs`, se existir.
pub fn resolve_in(spec: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let name = spec.strip_prefix(PREFIX).filter(|n| valid_name(n))?;
    dirs.iter().find_map(|d| EXTENSIONS.iter().map(|ext| d.join(format!("{name}.{ext}"))).find(|p| p.is_file()))
}

pub fn resolve(spec: &str) -> Option<PathBuf> {
    resolve_in(spec, &dirs())
}

/// Título de exibição de um spec `video:<nome>`.
pub fn title(spec: &str) -> Option<String> {
    list().into_iter().find(|v| v.id == spec).map(|v| v.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inae_factory_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn videos_are_found_with_metadata_and_resolved_by_stable_id() {
        let a = temp("a");
        let b = temp("b");
        std::fs::write(a.join("tokyo_alley.mp4"), b"x").unwrap();
        std::fs::write(a.join("tokyo_alley.json"), "\u{feff}{\"title\":\"Beco em Tóquio\",\"description\":\"Chuva e neon.\"}").unwrap(); // com BOM
        std::fs::write(a.join("notas.txt"), b"x").unwrap();
        std::fs::write(a.join(".oculto.mp4"), b"x").unwrap();
        std::fs::write(a.join("A Ruina (1).mp4"), b"x").unwrap();
        std::fs::write(b.join("tokyo_alley.webm"), b"x").unwrap(); // a primeira pasta vence
        std::fs::write(b.join("winter-cabin.webm"), b"x").unwrap();
        let dirs = vec![a.clone(), b.clone()];

        let list = list_in(&dirs);
        let ids: Vec<&str> = list.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["video:A Ruina (1)", "video:tokyo_alley", "video:winter-cabin"]);
        assert_eq!(list[0].title, "A Ruina", "o sufixo (1) de cópia some do título");
        assert_eq!((list[1].title.as_str(), list[1].category.as_str()), ("Beco em Tóquio", "Ambient"));
        assert_eq!(list[2].title, "Winter Cabin");
        assert!(list[1].file.ends_with("tokyo_alley.mp4"));
        assert_eq!(resolve_in("video:A Ruina (1)", &dirs), Some(a.join("A Ruina (1).mp4")));

        assert_eq!(resolve_in("video:tokyo_alley", &dirs), Some(a.join("tokyo_alley.mp4")));
        assert_eq!(resolve_in("video:winter-cabin", &dirs), Some(b.join("winter-cabin.webm")));
        for bad in ["video:", "video:../x", "video:a/b", "video:..", "video:.oculto", "video: tokyo_alley", "tokyo_alley", "video:nao_existe", r"video:C:\x", "video:a\\b"] {
            assert_eq!(resolve_in(bad, &dirs), None, "{bad}");
        }
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }
}
