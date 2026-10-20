//! Shader Lab dentro do Studio: edita HLSL e vê o resultado em tempo real, sem sair do app.
//!
//! O editor (`lab.html`, em um iframe da aba "Lab") envia o código para `/lab/code`, que compila com o
//! mesmo caminho do desktop e guarda o pipeline; a prévia pede quadros em `/lab/frame`. Erros de
//! compilação voltam como texto, e o último shader válido continua na tela. "Salvar" grava um `.hlsl`
//! na pasta de wallpapers importados, onde o motor já faz hot-reload quando o arquivo muda.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::config::EngineConfig;
use crate::render::offscreen::{OffscreenRenderer, preview_constants};
use crate::wallpaper::catalog;

const KEY: &str = "lab:live";
/// Mesmo limite do contrato de shader (docs/shader-contract.md).
const MAX_SOURCE: usize = 256 * 1024;

thread_local! {
    static LAB: RefCell<Option<OffscreenRenderer>> = const { RefCell::new(None) };
    static COMPILED: RefCell<bool> = const { RefCell::new(false) };
}

/// Compila `code` para a prévia. `Err` traz o texto do compilador; o shader anterior segue válido.
pub fn set_code(code: &str) -> Result<(), String> {
    if code.len() > MAX_SOURCE {
        return Err(format!("fonte maior que {} KiB", MAX_SOURCE / 1024));
    }
    LAB.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(OffscreenRenderer::new().map_err(|e| format!("renderizador indisponível: {e}"))?);
        }
        let r = slot.as_mut().unwrap();
        // Compila sob outra chave: se falhar, a prévia anterior continua intacta.
        let probe = "lab:probe";
        r.forget(probe);
        r.render(probe, &catalog::with_common(code), preview_constants(16, 16, 0.0, [0.0; 4]))
            .map_err(|e| e.to_string())?;
        r.forget(probe);
        r.forget(KEY);
        r.render(KEY, &catalog::with_common(code), preview_constants(16, 16, 0.0, [0.0; 4])).map_err(|e| e.to_string())?;
        COMPILED.with(|c| *c.borrow_mut() = true);
        Ok(())
    })
}

/// Um quadro RGBA8 do shader atual; `None` se ainda não há shader válido.
pub fn frame(w: u32, h: u32, t: f32, tint: [f32; 4], overrides: &[(String, f32)]) -> Option<Vec<u8>> {
    if !COMPILED.with(|c| *c.borrow()) {
        return None;
    }
    LAB.with(|cell| {
        let mut slot = cell.borrow_mut();
        let r = slot.as_mut()?;
        r.render_with_params(KEY, "", preview_constants(w.clamp(16, 1920), h.clamp(16, 1080), t, tint), Some(overrides)).ok()
    })
}

/// Pasta onde os shaders do Lab são salvos (dentro dos wallpapers importados).
pub fn save_dir() -> PathBuf {
    let base = EngineConfig::config_path();
    base.parent().unwrap_or(Path::new(".")).join("wallpapers")
}

/// Lê um shader salvo (`.hlsl`) de dentro de `dir` para abrir no Lab. Recusa qualquer caminho fora
/// da pasta, outras extensões e arquivos maiores que o limite do contrato.
pub fn read_saved_in(dir: &Path, file: &str) -> Result<String, String> {
    let root = dir.canonicalize().map_err(|_| "pasta de wallpapers indisponível".to_string())?;
    let path = Path::new(file).canonicalize().map_err(|_| "arquivo não encontrado".to_string())?;
    if !path.starts_with(&root) || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("hlsl")) {
        return Err("só shaders .hlsl salvos pelo app podem ser abertos".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() as usize > MAX_SOURCE {
        return Err("fonte grande demais".into());
    }
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

pub fn read_saved(file: &str) -> Result<String, String> {
    read_saved_in(&save_dir(), file)
}

/// Nome de arquivo seguro: minúsculas, dígitos, `_` e `-`; nunca vazio.
pub fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    let s = s.trim_matches('_');
    if s.is_empty() { "sem_nome".into() } else { s.chars().take(48).collect() }
}

/// Grava o código como `lab_<nome>.hlsl` e devolve o caminho.
pub fn save(name: &str, code: &str) -> Result<PathBuf, String> {
    if code.len() > MAX_SOURCE {
        return Err("fonte grande demais".into());
    }
    let dir = save_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("lab_{}.hlsl", slug(name)));
    std::fs::write(&path, code).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_safe() {
        assert_eq!(slug("Minha Aurora!"), "minha_aurora");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("   "), "sem_nome");
        assert!(slug(&"a".repeat(200)).len() <= 48);
    }

    #[test]
    fn compile_error_keeps_previous_shader() {
        let ok = "float4 ps_main(float4 pos : SV_Position) : SV_Target { return float4(1,0,0,1); }";
        if set_code(ok).is_err() {
            return; // sem GPU neste ambiente
        }
        let before = frame(32, 32, 0.0, [0.0; 4], &[]).expect("quadro");
        let err = set_code("float4 ps_main(float4 pos : SV_Position) : SV_Target { return nao_existe; }").unwrap_err();
        assert!(!err.is_empty());
        let after = frame(32, 32, 0.0, [0.0; 4], &[]).expect("quadro depois do erro");
        assert_eq!(before, after, "o shader válido anterior deve continuar na tela");
    }

    #[test]
    fn open_saved_is_confined_to_the_wallpapers_folder() {
        let base = std::env::temp_dir().join(format!("inae_open_{}", std::process::id()));
        let dir = base.join("wallpapers");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.hlsl"), "// ok").unwrap();
        std::fs::write(dir.join("nota.txt"), "x").unwrap();
        std::fs::write(base.join("fora.hlsl"), "// fora").unwrap();
        assert_eq!(read_saved_in(&dir, &dir.join("ok.hlsl").to_string_lossy()).unwrap(), "// ok");
        assert!(read_saved_in(&dir, &dir.join("nota.txt").to_string_lossy()).is_err());
        assert!(read_saved_in(&dir, &base.join("fora.hlsl").to_string_lossy()).is_err());
        assert!(read_saved_in(&dir, &dir.join("..").join("fora.hlsl").to_string_lossy()).is_err());
        assert!(read_saved_in(&dir, "nao-existe.hlsl").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
