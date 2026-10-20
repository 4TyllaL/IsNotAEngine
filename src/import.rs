//! Importação de wallpapers do usuário: cópia para a pasta local sem sobrescrever
//! arquivos homônimos e rótulos exibidos no Studio.

use std::io;
use std::path::{Path, PathBuf};

/// Copia `src` para `dest_dir` e devolve o caminho final.
///
/// Se já existir um arquivo de mesmo nome com conteúdo idêntico, ele é reaproveitado;
/// se o conteúdo for diferente, a cópia ganha um sufixo (`nome-2.ext`) em vez de
/// sobrescrever o wallpaper anterior. Importar um arquivo que já está em `dest_dir`
/// devolve o próprio caminho.
pub fn import_into(src: &Path, dest_dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let name = src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "caminho sem nome de arquivo"))?;
    let stem = Path::new(name).file_stem().unwrap_or(name).to_string_lossy().to_string();
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_string());

    let src_canon = src.canonicalize()?;
    let mut n = 1u32;
    loop {
        let file = match (&ext, n) {
            (_, 1) => name.to_string_lossy().to_string(),
            (Some(e), n) => format!("{stem}-{n}.{e}"),
            (None, n) => format!("{stem}-{n}"),
        };
        let target = dest_dir.join(file);
        if !target.exists() {
            std::fs::copy(src, &target)?;
            return Ok(target);
        }
        if target.canonicalize()? == src_canon || same_content(src, &target)? {
            return Ok(target);
        }
        n += 1;
    }
}

fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(std::fs::metadata(a)?.len() == std::fs::metadata(b)?.len() && std::fs::read(a)? == std::fs::read(b)?)
}

/// Rótulo do tipo de wallpaper mostrado na galeria do Studio.
pub fn kind_label(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "mp4" | "webm" | "wmv" | "mov" | "avi" | "mkv" => "Vídeo HW",
        "html" | "htm" => "Web HTML5",
        "livewallpaper" => "LiveWallpaper",
        "png" | "jpg" | "jpeg" | "bmp" | "webp" | "tif" | "tiff" => "Imagem",
        _ => "Shader",
    }
}

/// Importar isto é trazer uma **página com código** (HTML/JS): arquivo `.html`, pasta com `index.html` ou
/// pacote `.livewallpaper`. O Studio pede confirmação antes. Shaders, vídeos e imagens não entram aqui.
pub fn runs_page_code(path: &Path) -> bool {
    if path.is_dir() {
        return true;
    }
    matches!(path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str(), "html" | "htm" | "livewallpaper")
}

/// Um wallpaper do usuário na pasta de importados (para a galeria do Studio).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ImportedItem {
    pub path: String,
    pub title: String,
    pub kind: &'static str,
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

/// Os arquivos de `dir` que o motor sabe exibir, em ordem alfabética (até 500).
pub fn list_imported(dir: &Path) -> Vec<ImportedItem> {
    let mut items: Vec<ImportedItem> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() {
                return None;
            }
            let kind = imported_kind(&path.extension()?.to_str()?.to_ascii_lowercase())?;
            Some(ImportedItem { path: path.to_string_lossy().into_owned(), title: path.file_stem()?.to_string_lossy().into_owned(), kind })
        })
        .collect();
    items.sort_by_cached_key(|i| i.title.to_lowercase());
    items.truncate(500);
    items
}

/// Caminho canônico de `file` se (e só se) estiver ESTRITAMENTE dentro de `dir` (nunca a própria pasta,
/// nunca fora dela, nem por `..` ou atalho).
pub fn resolve_inside(dir: &Path, file: &str) -> Result<PathBuf, String> {
    let root = dir.canonicalize().map_err(|_| "a pasta de wallpapers não existe".to_string())?;
    let path = Path::new(file).canonicalize().map_err(|_| "o arquivo não existe mais".to_string())?;
    if path == root || !path.starts_with(&root) {
        return Err("só dá para remover wallpapers da pasta de importados do app".into());
    }
    Ok(path)
}

/// Manda para a Lixeira do Windows (dá para restaurar de lá). Nunca apaga definitivamente.
pub fn recycle(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW, SHFileOperationW};
    use windows::core::PCWSTR;
    // `canonicalize` devolve `\\?\C:\...`; SHFileOperation não entende esse prefixo (erro 124).
    let shown = path.as_os_str().to_string_lossy();
    let plain = shown.strip_prefix(r"\\?\").filter(|rest| !rest.starts_with("UNC\\")).map(Path::new).unwrap_or(path);
    let mut from: Vec<u16> = plain.as_os_str().encode_wide().collect();
    from.extend([0, 0]); // a lista termina com dois nulos
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16,
        ..Default::default()
    };
    // SAFETY: `from` vive durante a chamada e termina com dois nulos, como a API exige.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(io::Error::other(format!("a Lixeira recusou (código {code})")));
    }
    Ok(())
}

/// Remove da pasta de cache de pacotes as extrações de `<nome>.livewallpaper` (`<nome>-<16 hex>`).
pub fn clean_package_cache(cache_dir: &Path, stem: &str) -> usize {
    let prefix = format!("{stem}-");
    let mut n = 0;
    for e in std::fs::read_dir(cache_dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let is_ours = name.strip_prefix(&prefix).is_some_and(|rest| rest.len() == 16 && rest.chars().all(|c| c.is_ascii_hexdigit()));
        if is_ours && e.path().is_dir() && std::fs::remove_dir_all(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// Dados que o app gera e que o usuário pode limpar (Ajustes › Dados).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKind {
    ShaderCache,
    PackageCache,
    AiDrafts,
    ConfigBackups,
}

impl DataKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "shader_cache" => Self::ShaderCache,
            "package_cache" => Self::PackageCache,
            "ai_drafts" => Self::AiDrafts,
            "config_backups" => Self::ConfigBackups,
            _ => return None,
        })
    }

    pub const ALL: [(&'static str, DataKind); 4] = [
        ("shader_cache", Self::ShaderCache),
        ("package_cache", Self::PackageCache),
        ("ai_drafts", Self::AiDrafts),
        ("config_backups", Self::ConfigBackups),
    ];

    /// Itens (arquivos ou pastas) deste tipo dentro da pasta de dados `base` (a do `config.toml`).
    pub fn items(self, base: &Path) -> Vec<PathBuf> {
        let list = |dir: PathBuf, keep: &dyn Fn(&str) -> bool| -> Vec<PathBuf> {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| keep(&e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect()
        };
        match self {
            Self::ShaderCache => list(base.join("shader_cache"), &|n| n.ends_with(".bin")),
            Self::PackageCache => list(base.join("cache"), &|_| true),
            Self::AiDrafts => list(base.join("ai_drafts"), &|n| n.starts_with("rascunho-")),
            Self::ConfigBackups => list(base.to_path_buf(), &|n| n.starts_with("config.toml.") && !n.ends_with(".tmp")),
        }
    }

    /// Caches e rascunhos são descartáveis (apagados); os backups da configuração vão para a Lixeira.
    pub fn recoverable(self) -> bool {
        matches!(self, Self::ConfigBackups)
    }
}

fn size_of(path: &Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size_of(&e.path())).sum(),
        Err(_) => 0,
    }
}

/// (quantidade de itens, bytes) de cada tipo de dado limpável.
pub fn data_usage(base: &Path) -> Vec<(&'static str, usize, u64)> {
    DataKind::ALL
        .iter()
        .map(|(name, kind)| {
            let items = kind.items(base);
            (*name, items.len(), items.iter().map(|p| size_of(p)).sum())
        })
        .collect()
}

/// Limpa um tipo de dado com `remove` (injetável nos testes). Devolve (itens removidos, bytes liberados).
pub fn clean_data(base: &Path, kind: DataKind, remove: &dyn Fn(&Path) -> io::Result<()>) -> (usize, u64) {
    let (mut n, mut bytes) = (0, 0);
    for p in kind.items(base) {
        let size = size_of(&p);
        if remove(&p).is_ok() {
            n += 1;
            bytes += size;
        }
    }
    (n, bytes)
}

/// Remoção definitiva de um arquivo ou pasta de cache.
pub fn delete_path(p: &Path) -> io::Result<()> {
    if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_different_content_does_not_overwrite() {
        let base = std::env::temp_dir().join("isnotaengine_import_test");
        let _ = std::fs::remove_dir_all(&base);
        let (a, b, dest) = (base.join("a"), base.join("b"), base.join("dest"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("wp.hlsl"), "um").unwrap();
        std::fs::write(b.join("wp.hlsl"), "dois").unwrap();

        let p1 = import_into(&a.join("wp.hlsl"), &dest).unwrap();
        let p2 = import_into(&b.join("wp.hlsl"), &dest).unwrap();
        assert_eq!(p1.file_name().unwrap(), "wp.hlsl");
        assert_eq!(p2.file_name().unwrap(), "wp-2.hlsl");
        assert_eq!(std::fs::read_to_string(&p1).unwrap(), "um");
        assert_eq!(std::fs::read_to_string(&p2).unwrap(), "dois");

        // Reimportar o mesmo conteúdo e importar de dentro da pasta de destino reaproveitam o arquivo.
        assert_eq!(import_into(&a.join("wp.hlsl"), &dest).unwrap(), p1);
        assert_eq!(import_into(&p2, &dest).unwrap(), p2);
    }

    #[test]
    fn labels_by_extension() {
        assert_eq!(kind_label(Path::new("x.MP4")), "Vídeo HW");
        assert_eq!(kind_label(Path::new("x.html")), "Web HTML5");
        assert_eq!(kind_label(Path::new("x.livewallpaper")), "LiveWallpaper");
        assert_eq!(kind_label(Path::new("x.hlsl")), "Shader");
        assert_eq!(kind_label(Path::new("x.PNG")), "Imagem");
        assert_eq!(kind_label(Path::new("foto.webp")), "Imagem");
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inae_lib_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_what_the_engine_can_show_sorted_and_skips_the_rest() {
        let d = tmp("list");
        for f in ["b.hlsl", "A.html", "c.mp4", "d.png", "e.livewallpaper", "nota.txt", "f.zip"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        std::fs::create_dir_all(d.join("pasta")).unwrap();
        let got: Vec<(String, &str)> = list_imported(&d).into_iter().map(|i| (i.title, i.kind)).collect();
        assert_eq!(got, [("A".into(), "web"), ("b".into(), "shader"), ("c".into(), "video"), ("d".into(), "image"), ("e".into(), "livewallpaper")]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn removal_is_confined_to_the_imported_folder() {
        let base = tmp("confine");
        let dir = base.join("wallpapers");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.hlsl"), "x").unwrap();
        std::fs::write(base.join("config.toml"), "x").unwrap();
        assert!(resolve_inside(&dir, &dir.join("ok.hlsl").to_string_lossy()).is_ok());
        assert!(resolve_inside(&dir, &base.join("config.toml").to_string_lossy()).is_err(), "fora da pasta");
        assert!(resolve_inside(&dir, &dir.join("..").join("config.toml").to_string_lossy()).is_err(), "por ..");
        assert!(resolve_inside(&dir, &dir.to_string_lossy()).is_err(), "nunca a própria pasta");
        assert!(resolve_inside(&dir, &dir.join("nao_existe.hlsl").to_string_lossy()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn package_cache_cleanup_only_touches_that_packages_folders() {
        let c = tmp("pkgcache");
        for d in ["meu-0123456789abcdef", "meu-fedcba9876543210", "meu-extra-0123456789abcdef", "outro-0123456789abcdef", "meu-curto"] {
            std::fs::create_dir_all(c.join(d)).unwrap();
        }
        assert_eq!(clean_package_cache(&c, "meu"), 2);
        assert!(c.join("meu-extra-0123456789abcdef").exists() && c.join("outro-0123456789abcdef").exists() && c.join("meu-curto").exists());
        let _ = std::fs::remove_dir_all(&c);
    }

    #[test]
    fn data_cleanup_counts_removes_only_its_kind_and_reports_bytes() {
        let base = tmp("data");
        std::fs::create_dir_all(base.join("shader_cache")).unwrap();
        std::fs::write(base.join("shader_cache/1.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(base.join("shader_cache/2.bin"), vec![0u8; 50]).unwrap();
        std::fs::write(base.join("shader_cache/leia-me.txt"), "x").unwrap();
        std::fs::create_dir_all(base.join("ai_drafts")).unwrap();
        std::fs::write(base.join("ai_drafts/rascunho-1.glsl"), "x").unwrap();
        std::fs::write(base.join("config.toml"), "importante").unwrap();
        std::fs::write(base.join("config.toml.bak-1"), "b").unwrap();
        std::fs::write(base.join("config.toml.invalid"), "i").unwrap();
        std::fs::write(base.join("config.toml.tmp"), "t").unwrap();

        let usage: std::collections::HashMap<_, _> = data_usage(&base).into_iter().map(|(n, c, b)| (n, (c, b))).collect();
        assert_eq!(usage["shader_cache"], (2, 150));
        assert_eq!(usage["config_backups"].0, 2, "o .tmp e o config.toml não contam");

        let (n, bytes) = clean_data(&base, DataKind::ShaderCache, &delete_path);
        assert_eq!((n, bytes), (2, 150));
        assert!(base.join("shader_cache/leia-me.txt").exists(), "só os .bin");
        assert!(base.join("ai_drafts/rascunho-1.glsl").exists(), "outros tipos intactos");
        // backups: o removedor injetado decide como (na prática, a Lixeira); o config.toml nunca entra
        let removed = std::cell::RefCell::new(Vec::new());
        let (n, _) = clean_data(&base, DataKind::ConfigBackups, &|p: &Path| {
            removed.borrow_mut().push(p.file_name().unwrap().to_string_lossy().to_string());
            Ok(())
        });
        assert_eq!(n, 2);
        let mut r = removed.into_inner();
        r.sort();
        assert_eq!(r, ["config.toml.bak-1", "config.toml.invalid"]);
        assert!(base.join("config.toml").exists());
        assert!(DataKind::parse("../x").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[ignore = "manda um arquivo temporário à Lixeira de verdade; rode à mão: cargo test recycle_moves -- --ignored"]
    fn recycle_moves_a_real_file_to_the_recycle_bin() {
        let d = tmp("recycle");
        let f = d.join("remover-teste-inae.hlsl");
        std::fs::write(&f, "// teste").unwrap();
        // resolve_inside entrega o caminho canônico (com prefixo \\?\): é o caso real do Studio.
        let canon = f.canonicalize().unwrap();
        recycle(&canon).expect("lixeira");
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_pages_and_packages_need_the_code_warning() {
        for yes in ["a.html", "a.HTM", "pacote.livewallpaper"] {
            assert!(runs_page_code(Path::new(yes)), "{yes}");
        }
        for no in ["a.hlsl", "v.mp4", "i.png", "nada"] {
            assert!(!runs_page_code(Path::new(no)), "{no}");
        }
        assert!(runs_page_code(&std::env::temp_dir()), "pasta");
    }
}
