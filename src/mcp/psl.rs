//! Compilação e renderização de shaders para o servidor MCP: a mesma máquina do Lab (PSL),
//! em forma de biblioteca. Não toca no desktop, na bandeja nem na configuração do motor.

use std::time::{Duration, Instant};

use crate::lab::image_stats;
use crate::render::offscreen::{OffscreenRenderer, preview_constants};
use crate::render::params::{self, ParamSpec};
use crate::compiler_worker::WorkerError;
use crate::render::pipeline::{CompiledShader, ShaderPipeline};
use crate::wallpaper::catalog::with_common;

/// Mesmos limites do contrato de shader e do Lab.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_PIXELS: u64 = 7680 * 4320;
const COMPILE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Hlsl,
    /// GLSL no formato do Shadertoy (`mainImage`).
    Glsl,
}

impl Lang {
    /// `None` = detectar: `mainImage` sem `ps_main`/`f_main` é Shadertoy.
    pub fn parse(name: Option<&str>, code: &str) -> Result<Self, String> {
        match name.map(str::to_ascii_lowercase).as_deref() {
            Some("hlsl") => Ok(Lang::Hlsl),
            Some("glsl" | "shadertoy") => Ok(Lang::Glsl),
            Some(other) => Err(format!("unknown language \"{other}\": use \"hlsl\" or \"glsl\"")),
            None => Ok(if code.contains("mainImage") && !code.contains("ps_main") && !code.contains("f_main") {
                Lang::Glsl
            } else {
                Lang::Hlsl
            }),
        }
    }
}

pub enum Failure {
    /// Recusado antes de compilar (tamanho, recurso sem suporte na conversão).
    Rejected(String),
    Compile(String),
    Timeout,
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::Rejected(m) | Failure::Compile(m) => m.clone(),
            Failure::Timeout => format!(
                "compilation exceeded {}s (usually a huge unrolled loop): reduce loop counts",
                COMPILE_TIMEOUT.as_secs()
            ),
        }
    }
}

pub struct Compiled {
    /// HLSL final (o próprio código, ou o resultado da conversão do GLSL).
    pub hlsl: String,
    pub bytes: CompiledShader,
    /// Avisos da conversão GLSL.
    pub notes: Vec<String>,
    pub params: Vec<ParamSpec>,
    pub compile_ms: f64,
}

/// Limpa as mensagens do compilador: tira o caminho do arquivo e escreve `line N, col A-B: ...`,
/// subtraindo `offset` (linhas do prelúdio que a conversão do Shadertoy adiciona) para que o
/// número aponte para o código do autor.
pub fn clean_messages(msg: &str, offset: u32) -> String {
    const MARK: &str = "shader.hlsl(";
    msg.lines()
        .map(|line| {
            let Some(at) = line.rfind(MARK) else { return line.to_string() };
            let rest = &line[at + MARK.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = &rest[digits.len()..];
            let (Ok(n), Some(cols)) = (digits.parse::<u32>(), after.strip_prefix(',')) else {
                return line.to_string();
            };
            let Some((cols, tail)) = cols.split_once(')') else { return line.to_string() };
            format!("line {}, col {cols}{tail}", n.saturating_sub(offset).max(1))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn compile(code: &str, lang: Lang) -> Result<Compiled, Failure> {
    if code.len() > MAX_SOURCE_BYTES {
        return Err(Failure::Rejected(format!(
            "source is {} KiB; the limit is {} KiB",
            code.len() / 1024,
            MAX_SOURCE_BYTES / 1024
        )));
    }
    let (hlsl, notes, offset) = match lang {
        Lang::Hlsl => (code.to_string(), Vec::new(), 0),
        Lang::Glsl => {
            let c = crate::shadertoy::convert(code, "mcp").map_err(Failure::Rejected)?;
            (c.hlsl, c.warnings, c.body_line.saturating_sub(1))
        }
    };

    // Num processo filho com prazo: o FXC pode demorar muito em loops desenrolados e, no
    // timeout, o filho é encerrado de verdade (docs/compiler-worker.md).
    let full = with_common(&hlsl);
    let started = Instant::now();
    let compile_ms = || started.elapsed().as_secs_f64() * 1000.0;
    match crate::compiler_worker::compile(&full, "shader.hlsl", COMPILE_TIMEOUT) {
        Err(WorkerError::Timeout) => Err(Failure::Timeout),
        Err(WorkerError::Busy) => Err(Failure::Rejected("the compiler is busy; try again in a moment".into())),
        Err(WorkerError::Rejected(m)) => Err(Failure::Rejected(m)),
        Err(WorkerError::Crashed(m)) => Err(Failure::Rejected(format!("the compiler process failed: {m}"))),
        Ok(Err(msg)) => Err(Failure::Compile(clean_messages(&msg, offset))),
        Ok(Ok(mut bytes)) => {
            bytes.messages = clean_messages(&bytes.messages, offset);
            let (params, _problems) = params::parse(&hlsl);
            Ok(Compiled { hlsl, bytes, notes, params, compile_ms: compile_ms() })
        }
    }
}

pub struct Frame {
    pub t: f32,
    pub rgba: Vec<u8>,
    pub gpu_ms: Option<f64>,
    pub luma_mean: f32,
    pub luma_std: f32,
    pub crushed: f32,
    pub clipped: f32,
    /// Diferença média em relação ao quadro anterior da mesma chamada.
    pub delta_prev: Option<f32>,
}

pub struct Session {
    renderer: OffscreenRenderer,
}

impl Session {
    pub fn new() -> Result<Self, String> {
        OffscreenRenderer::new()
            .map(|renderer| Self { renderer })
            .map_err(|e| format!("could not create a Direct3D 11 device: {e}"))
    }

    /// Renderiza `times` em `w`x`h`. Relógio, mouse e áudio são fixos (determinístico), como no Lab.
    pub fn render(
        &mut self,
        c: &Compiled,
        times: &[f32],
        w: u32,
        h: u32,
        overrides: &[(String, f32)],
    ) -> Result<Vec<Frame>, String> {
        let pipeline = ShaderPipeline::from_bytecode(self.renderer.device(), &c.bytes.vs, &c.bytes.ps)
            .map_err(|e| format!("could not create the pipeline: {e}"))?;
        self.renderer.insert("mcp", pipeline, &c.hlsl);

        let mut values = params::defaults(&c.params);
        for (name, v) in overrides {
            if let Some(i) = c.params.iter().position(|s| &s.name == name) {
                values[i] = c.params[i].clamp(*v);
            }
        }

        let mut frames = Vec::with_capacity(times.len());
        let mut prev: Option<Vec<u8>> = None;
        for &t in times {
            let mut k = preview_constants(w, h, t, [0.0; 4]);
            k.clock_time = [12.0, 0.0, 0.0, 0.0];
            k.clock_date = [2026.0, 1.0, 1.0, 4.0];
            k.audio_reactive = [0.0; 4];
            k.audio_spectrum = [0.0; 16];
            k.params = values;
            let (rgba, gpu_ms) = self
                .renderer
                .render_timed("mcp", k)
                .map_err(|e| format!("render failed at t={t}: {e}"))?;
            let (luma_mean, luma_std, crushed, clipped, delta_prev) = image_stats(&rgba, prev.as_deref());
            frames.push(Frame { t, rgba: rgba.clone(), gpu_ms, luma_mean, luma_std, crushed, clipped, delta_prev });
            prev = Some(rgba);
        }
        Ok(frames)
    }
}

/// Observações objetivas sobre os quadros, para a IA se corrigir sem depender só de olhar a imagem.
pub fn hints(frames: &[Frame], w: u32, h: u32) -> Vec<String> {
    let mut out = Vec::new();
    let mean = frames.iter().map(|f| f.luma_mean).sum::<f32>() / frames.len().max(1) as f32;
    let std = frames.iter().map(|f| f.luma_std).sum::<f32>() / frames.len().max(1) as f32;
    let clipped = frames.iter().map(|f| f.clipped).fold(0.0, f32::max);
    let crushed = frames.iter().map(|f| f.crushed).fold(0.0, f32::max);
    if mean < 0.06 {
        out.push(format!("Very dark (mean luma {mean:.2}); a desktop wallpaper this dark loses its detail. Raise exposure or glow."));
    } else if mean > 0.80 {
        out.push(format!("Very bright (mean luma {mean:.2}); desktop icons and text will be hard to read."));
    }
    if clipped > 0.15 {
        out.push(format!("{:.0}% of the pixels are blown out (a channel at 255): compress highlights (e.g. col / (1.0 + col)).", clipped * 100.0));
    }
    if crushed > 0.60 {
        out.push(format!("{:.0}% of the pixels are pure black: most of the screen is empty.", crushed * 100.0));
    }
    if std < 0.02 && !frames.is_empty() {
        out.push(format!("Almost flat image (luma std {std:.3}): there is little visible structure."));
    }
    let moved = frames.iter().filter_map(|f| f.delta_prev).fold(0.0f32, f32::max);
    if frames.len() > 1 && moved < 0.003 {
        out.push("The frames barely differ from each other: the animation is static or too slow for the requested times.".into());
    }
    let gpu = frames.iter().filter_map(|f| f.gpu_ms).fold(0.0f64, f64::max);
    if gpu > 0.0 {
        let est_1080p = gpu * (1920.0 * 1080.0) / (f64::from(w) * f64::from(h)).max(1.0);
        if est_1080p > 4.0 {
            out.push(format!(
                "Expensive: about {est_1080p:.1} ms of GPU at 1080p (the budget is 4 ms, and it runs on every monitor). Reduce loop counts or octaves."
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection() {
        assert_eq!(Lang::parse(None, "void mainImage(out vec4 c, in vec2 p){}").unwrap(), Lang::Glsl);
        assert_eq!(Lang::parse(None, "float4 ps_main(float4 p:SV_Position):SV_Target{return 0;}").unwrap(), Lang::Hlsl);
        assert_eq!(Lang::parse(Some("GLSL"), "").unwrap(), Lang::Glsl);
        assert_eq!(Lang::parse(Some("shadertoy"), "").unwrap(), Lang::Glsl);
        assert!(Lang::parse(Some("wgsl"), "").is_err());
    }

    #[test]
    fn error_lines_point_to_the_authors_glsl() {
        let msg = "C:\\x\\shader.hlsl(120,5): error X3000: syntax error\nfoo\nshader.hlsl(130,9): warning X1: y";
        let out = clean_messages(msg, 100);
        assert!(out.contains("line 20, col 5: error X3000: syntax error"), "{out}");
        assert!(out.contains("line 30, col 9: warning X1: y"), "{out}");
        assert!(!out.contains("C:\\x"), "caminho removido: {out}");
        assert_eq!(out.lines().nth(1), Some("foo"));
        // sem deslocamento só limpa o caminho; faixa de colunas e texto depois do ')' são mantidos
        assert_eq!(
            clean_messages("falha ao compilar shader shader.hlsl: C:\\a\\shader.hlsl(2,17-25): error X3004: undeclared identifier 'z'", 0),
            "line 2, col 17-25: error X3004: undeclared identifier 'z'"
        );
        // nunca vira linha 0 nem estoura
        assert!(clean_messages("shader.hlsl(3,1): error", 100).starts_with("line 1, col 1"));
        assert_eq!(clean_messages("sem posição aqui", 5), "sem posição aqui");
    }

    fn frames(luma: f32, std: f32, delta: Option<f32>) -> Vec<Frame> {
        vec![
            Frame { t: 0.0, rgba: vec![], gpu_ms: Some(0.5), luma_mean: luma, luma_std: std, crushed: 0.0, clipped: 0.0, delta_prev: None },
            Frame { t: 1.0, rgba: vec![], gpu_ms: Some(0.5), luma_mean: luma, luma_std: std, crushed: 0.0, clipped: 0.0, delta_prev: delta },
        ]
    }

    #[test]
    fn hints_flag_dark_flat_static_and_expensive() {
        let dark = hints(&frames(0.02, 0.01, Some(0.0)), 800, 450);
        assert!(dark.iter().any(|h| h.contains("Very dark")));
        assert!(dark.iter().any(|h| h.contains("flat")));
        assert!(dark.iter().any(|h| h.contains("barely differ")));
        assert!(hints(&frames(0.3, 0.2, Some(0.05)), 800, 450).is_empty(), "um quadro saudável não gera aviso");

        let mut slow = frames(0.3, 0.2, Some(0.05));
        slow[0].gpu_ms = Some(3.0); // a 800x450 ~ 17 ms estimados a 1080p
        assert!(hints(&slow, 800, 450).iter().any(|h| h.contains("Expensive")));
    }

    #[test]
    fn compile_rejects_oversized_and_reports_errors_with_lines() {
        assert!(matches!(compile(&"a".repeat(MAX_SOURCE_BYTES + 1), Lang::Hlsl), Err(Failure::Rejected(_))));
        match compile("float4 ps_main(float4 p : SV_Position) : SV_Target { return foo; }", Lang::Hlsl) {
            Err(Failure::Compile(m)) => assert!(m.contains("foo"), "{m}"),
            _ => panic!("deveria falhar a compilação"),
        }
        let ok = compile("float4 ps_main(float4 p : SV_Position) : SV_Target { return float4(frag_uv(p), 0, 1); }", Lang::Hlsl);
        assert!(ok.is_ok());
    }

    #[test]
    fn glsl_errors_are_reported_against_the_glsl_lines() {
        // erro na linha 3 do GLSL do autor
        let glsl = "// um\n// dois\nvoid mainImage(out vec4 O, in vec2 U){ O = vec4(naoexiste, 0., 0., 1.); }\n";
        match compile(glsl, Lang::Glsl) {
            Err(Failure::Compile(m)) => assert!(m.contains("line 3,") && m.contains("naoexiste"), "{m}"),
            Err(f) => panic!("{}", f.message()),
            Ok(_) => panic!("deveria falhar"),
        }
    }

    #[test]
    fn renders_deterministic_frames_with_stats() {
        let c = compile("float4 ps_main(float4 p : SV_Position) : SV_Target { float2 uv = frag_uv(p); return float4(uv, 0.5 + 0.5 * sin(T()), 1); }", Lang::Hlsl)
            .ok()
            .expect("compila");
        let mut s = Session::new().expect("GPU");
        let a = s.render(&c, &[1.0, 3.0], 64, 36, &[]).expect("render");
        let b = s.render(&c, &[1.0, 3.0], 64, 36, &[]).expect("render");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].rgba.len(), 64 * 36 * 4);
        assert_eq!(a[0].rgba, b[0].rgba, "mesma entrada, mesma imagem");
        assert!(a[1].delta_prev.unwrap() > 0.0, "o tempo muda o quadro");
    }
}
