//! Pipeline de shaders e buffers de constantes da GPU.

use tracing::{debug, error};
use windows::{
    Win32::Foundation::E_FAIL, Win32::Graphics::Direct3D::Fxc::*, Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*, core::*,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ShaderConstants {
    pub resolution: [f32; 2],
    pub time: f32,
    pub speed: f32,
    pub tint: [f32; 4],
    pub mouse: [f32; 2],
    pub mouse_click: [f32; 2],
    pub mouse_enabled: f32,
    pub parallax_enabled: f32,
    /// [ângulo YIQ do matiz de referência, 1.0 = rotação global ligada] (diretiva `// retint:`)
    pub tint_ref: [f32; 2],
    /// Relógio em tempo real: [hora (0..23), minuto (0..59), segundo (0..59), segundo_fracionário (segundo + ms/1000)]
    pub clock_time: [f32; 4],
    /// Data em tempo real: [ano, mês (1..12), dia (1..31), dia_da_semana (0..6)]
    pub clock_date: [f32; 4],
    /// Áudio reativo WASAPI: [graves/bass, médios/mid, agudos/treble, volume global RMS] (0.0 .. 1.0)
    pub audio_reactive: [f32; 4],
    /// Espectro de frequências FFT WASAPI (16 bandas logarítmicas normalizadas 0.0 .. 1.0)
    pub audio_spectrum: [f32; 16],
    /// Parâmetros de usuário declarados com `// param:` (ver docs/shader-contract.md).
    pub params: [f32; 8],
    /// Transição da playlist: x = quanto escurecer o quadro (0 = normal, 1 = preto). Zero = sem efeito.
    pub fade: [f32; 4],
}

/// Prazo de uma compilação de shader do usuário.
const COMPILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct ShaderPipeline {
    pub vs: ID3D11VertexShader,
    pub ps: ID3D11PixelShader,
    pub constants_buffer: ID3D11Buffer,
}

impl ShaderPipeline {
    pub fn compile(device: &ID3D11Device, code: &str, name: &str) -> Result<Self> {
        debug!(name = %name, "Compilando pipeline HLSL");
        let (vs_code, ps_entry) = stage_sources(code);
        let (vs_key, ps_key) = unsafe {
            (
                hash_shader_input(&vs_code, b"vs_main", b"vs_5_0"),
                hash_shader_input(code, ps_entry.as_bytes(), b"ps_5_0"),
            )
        };
        let (vs_bytes, ps_bytes) = match (cache_lookup(vs_key), cache_lookup(ps_key)) {
            (Some(vs), Some(ps)) => (vs, ps),
            _ => {
                // Cache frio: compila num processo filho com prazo (docs/compiler-worker.md).
                let compiled = crate::compiler_worker::compile(code, name, COMPILE_TIMEOUT)
                    .map_err(|e| {
                        error!(shader = %name, error = ?e, "Processo de compilação falhou");
                        Error::new(E_FAIL, format!("compilador indisponível para {name}: {e:?}"))
                    })?
                    .map_err(|msg| {
                        error!(shader = %name, error = %msg, "Falha na compilação do shader HLSL");
                        Error::new(E_FAIL, format!("falha ao compilar shader {name}: {msg}"))
                    })?;
                cache_store(vs_key, &compiled.vs);
                cache_store(ps_key, &compiled.ps);
                (compiled.vs, compiled.ps)
            }
        };
        Self::from_bytecode(device, &vs_bytes, &ps_bytes)
    }

    pub fn from_bytecode(device: &ID3D11Device, vs_bytes: &[u8], ps_bytes: &[u8]) -> Result<Self> {
        unsafe {
            let mut vs = None;
            let mut ps = None;
            device.CreateVertexShader(vs_bytes, None, Some(&mut vs))?;
            device.CreatePixelShader(ps_bytes, None, Some(&mut ps))?;

            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<ShaderConstants>() as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut constants_buffer = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut constants_buffer))?;

            Ok(Self {
                vs: vs.ok_or_else(|| Error::new(E_FAIL, "sem vertex shader"))?,
                ps: ps.ok_or_else(|| Error::new(E_FAIL, "sem pixel shader"))?,
                constants_buffer: constants_buffer.ok_or_else(|| Error::new(E_FAIL, "sem constant buffer"))?,
            })
        }
    }

    pub fn update_constants(
        &self,
        context: &ID3D11DeviceContext,
        constants: ShaderConstants,
    ) -> Result<()> {
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(
                &self.constants_buffer,
                0,
                D3D11_MAP_WRITE_DISCARD,
                0,
                Some(&mut mapped),
            )?;
            (mapped.pData as *mut ShaderConstants).write(constants);
            context.Unmap(&self.constants_buffer, 0);
            Ok(())
        }
    }
}

/// Código do vertex shader (injeta o triângulo fullscreen se faltar `vs_main`) e a
/// entrada do pixel shader (`ps_main`, ou `f_main` quando só ela existe).
fn stage_sources(code: &str) -> (String, PCSTR) {
    let vs_code = if code.contains("vs_main") {
        code.to_string()
    } else {
        format!(
            "float4 vs_main(uint id : SV_VertexID) : SV_Position {{
             float2 uv = float2((id << 1) & 2, id & 2);
             return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
             }}

{}",
            code
        )
    };
    let ps_entry = if code.contains("f_main") && !code.contains("ps_main") { s!("f_main") } else { s!("ps_main") };
    (vs_code, ps_entry)
}

/// Bytecode compilado sem cache, com as mensagens do compilador (warnings).
#[derive(Debug)]
pub struct CompiledShader {
    pub vs: Vec<u8>,
    pub ps: Vec<u8>,
    pub messages: String,
}

/// Compila VS e PS ignorando os caches (para medir tempo e coletar warnings). Não usa o
/// dispositivo, então pode rodar numa thread separada. O erro é o texto do compilador.
pub fn compile_uncached(code: &str, name: &str) -> std::result::Result<CompiledShader, String> {
    let (vs_code, ps_entry) = stage_sources(code);
    let (vs, m1) = compile_hlsl(&vs_code, name, s!("vs_main"), s!("vs_5_0")).map_err(|e| e.message())?;
    let (ps, m2) = compile_hlsl(code, name, ps_entry, s!("ps_5_0")).map_err(|e| e.message())?;
    // O VS injetado repete o código inteiro, então seus warnings duplicam os do PS.
    let messages = if m2.trim().is_empty() { m1 } else { m2 };
    Ok(CompiledShader { vs: blob_bytes(&vs).to_vec(), ps: blob_bytes(&ps).to_vec(), messages })
}

static SHADER_BYTECODE_CACHE: std::sync::Mutex<Option<std::collections::HashMap<u64, Vec<u8>>>> =
    std::sync::Mutex::new(None);

fn hash_shader_input(code: &str, entry: &[u8], target: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    entry.hash(&mut hasher);
    target.hash(&mut hasher);
    hasher.finish()
}

fn cache_file(key: u64) -> std::path::PathBuf {
    crate::config::EngineConfig::config_path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("shader_cache")
        .join(format!("{key:016x}.bin"))
}

/// Bytecode em cache (memória, depois disco).
fn cache_lookup(key: u64) -> Option<Vec<u8>> {
    if let Ok(guard) = SHADER_BYTECODE_CACHE.lock()
        && let Some(ref map) = *guard
        && let Some(cached) = map.get(&key)
    {
        return Some(cached.clone());
    }
    let bytes = std::fs::read(cache_file(key)).ok().filter(|b| !b.is_empty())?;
    if let Ok(mut guard) = SHADER_BYTECODE_CACHE.lock() {
        guard.get_or_insert_with(std::collections::HashMap::new).insert(key, bytes.clone());
    }
    Some(bytes)
}

fn cache_store(key: u64, bytes: &[u8]) {
    let file = cache_file(key);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, bytes);
    if let Ok(mut guard) = SHADER_BYTECODE_CACHE.lock() {
        guard.get_or_insert_with(std::collections::HashMap::new).insert(key, bytes.to_vec());
    }
}

/// Compila (com cache) um HLSL fixo e confiável do próprio app, no processo.
pub(crate) fn get_or_compile_bytecode(code: &str, name: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    let key = hash_shader_input(code, unsafe { entry.as_bytes() }, unsafe { target.as_bytes() });
    if let Some(bytes) = cache_lookup(key) {
        return Ok(bytes);
    }
    let (blob, _) = compile_hlsl(code, name, entry, target)?;
    let bytes = blob_bytes(&blob).to_vec();
    cache_store(key, &bytes);
    Ok(bytes)
}

fn compile_hlsl(code: &str, name: &str, entry: PCSTR, target: PCSTR) -> Result<(ID3DBlob, String)> {
    let mut blob = None;
    let mut errors = None;
    let source_name = std::ffi::CString::new(name).unwrap_or_default();

    let result = unsafe {
        D3DCompile(
            code.as_ptr() as *const _,
            code.len(),
            PCSTR(source_name.as_ptr() as *const _),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };
    let msg = errors
        .map(|b| String::from_utf8_lossy(blob_bytes(&b)).trim_end_matches('\0').to_string())
        .unwrap_or_default();
    if let Err(e) = result {
        error!(shader = %name, error = %msg, "Falha na compilação do shader HLSL");
        return Err(Error::new(
            e.code(),
            format!("falha ao compilar shader {name}: {msg}"),
        ));
    }
    let blob = blob.ok_or_else(|| Error::new(E_FAIL, "D3DCompile não retornou código"))?;
    Ok((blob, msg))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_shaders_compile() {
        use crate::wallpaper::catalog::{CATALOG, Kind, with_common};

        let mut sources: Vec<(String, String)> = CATALOG
            .iter()
            .filter(|e| e.kind == Kind::Shader)
            .map(|e| (e.id.to_string(), with_common(e.source)))
            .collect();
        // O fallback interno também precisa compilar (o catálogo pode não ter nenhum shader).
        sources.push(("fallback".to_string(), with_common(crate::wallpaper::manager::FALLBACK_SOURCE)));

        for (name, code) in sources {
            for (entry, target) in [(s!("vs_main"), s!("vs_5_0")), (s!("ps_main"), s!("ps_5_0"))] {
                let res = compile_hlsl(&code, &name, entry, target);
                assert!(res.is_ok(), "Shader {name} falhou na compilação: {:?}", res.err());
            }
        }
    }
}
