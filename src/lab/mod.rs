//! Shader Lab: ferramenta de desenvolvimento para iterar shaders fora do desktop.
//!
//! `IsNotAEngine.exe --lab <render|seq|watch> <arquivo.hlsl> [opções]`
//!
//! Compila sem cache (tempo e warnings reais), renderiza offscreen pelo mesmo caminho das
//! prévias do Studio (bloom incluído) com constantes determinísticas e grava cada execução
//! em `<out>/<shader>/<data-hora>/`: PNGs, cópia da fonte e `report.json`.
//! Não toca em WorkerW, bandeja, Studio nem na configuração do motor.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::render::offscreen::{OffscreenRenderer, preview_constants};
use crate::render::params::{self, ParamSpec};
use crate::compiler_worker::WorkerError;
use crate::render::pipeline::{ShaderConstants, ShaderPipeline};

// Limites para fontes não confiáveis.
const MAX_SOURCE_BYTES: u64 = 256 * 1024;
const MAX_DIM: u32 = 7680;
const MAX_PIXELS: u64 = 7680 * 4320;
const MAX_FRAMES: usize = 1200;
const COMPILE_TIMEOUT: Duration = Duration::from_secs(30);

const USAGE: &str = "\
uso: IsNotAEngine.exe --lab <comando> <arquivo.hlsl> [opções]

comandos:
  render   frames nos instantes de --times
  seq      sequência de --frames frames a --fps a partir de --start
  watch    observa o arquivo; a cada salvamento recompila e roda `render`

opções:
  --size WxH         resolução (padrão 1920x1080)
  --times a,b,c      instantes em segundos para render/watch (padrão 0,5,15,30)
  --start S          início da sequência (padrão 0)
  --fps N            taxa da sequência (padrão 30)
  --frames N         quantidade de frames da sequência (padrão 60, máx. 1200)
  --param nome=v     sobrescreve um parâmetro `// param:` (repetível)
  --audio            áudio sintético das prévias (padrão: silêncio)
  --tint #rrggbb     cor de destaque do Studio (padrão: automático)
  --out DIR          pasta das execuções (padrão lab/runs)

códigos de saída: 0 ok, 1 uso, 2 compilação, 3 timeout, 4 renderização";

#[derive(Debug, Clone)]
struct Options {
    command: String,
    file: PathBuf,
    width: u32,
    height: u32,
    times: Vec<f32>,
    start: f32,
    fps: f32,
    frames: usize,
    overrides: Vec<(String, f32)>,
    audio: bool,
    tint: [f32; 4],
    out: PathBuf,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut it = args.iter();
    let command = it.next().ok_or("faltou o comando")?.clone();
    if !["render", "seq", "watch"].contains(&command.as_str()) {
        return Err(format!("comando desconhecido: {command}"));
    }
    let file = PathBuf::from(it.next().ok_or("faltou o arquivo .hlsl")?);
    let mut o = Options {
        command,
        file,
        width: 1920,
        height: 1080,
        times: vec![0.0, 5.0, 15.0, 30.0],
        start: 0.0,
        fps: 30.0,
        frames: 60,
        overrides: Vec::new(),
        audio: false,
        tint: [0.0; 4],
        out: PathBuf::from("lab/runs"),
    };
    let num = |v: Option<&String>, name: &str| -> Result<f32, String> {
        v.and_then(|s| s.parse::<f32>().ok())
            .filter(|f| f.is_finite())
            .ok_or(format!("valor inválido para {name}"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--size" => {
                let v = it.next().ok_or("faltou WxH")?;
                let (w, h) = v.split_once('x').ok_or("use --size WxH")?;
                o.width = w.parse().map_err(|_| "largura inválida")?;
                o.height = h.parse().map_err(|_| "altura inválida")?;
            }
            "--times" => {
                let v = it.next().ok_or("faltou a lista de instantes")?;
                o.times = v
                    .split(',')
                    .map(|s| s.trim().parse::<f32>().ok().filter(|f| f.is_finite() && *f >= 0.0))
                    .collect::<Option<Vec<_>>>()
                    .ok_or("instantes inválidos")?;
            }
            "--start" => o.start = num(it.next(), "--start")?.max(0.0),
            "--fps" => o.fps = num(it.next(), "--fps")?,
            "--frames" => o.frames = num(it.next(), "--frames")? as usize,
            "--param" => {
                let v = it.next().ok_or("faltou nome=valor")?;
                let (k, val) = v.split_once('=').ok_or("use --param nome=valor")?;
                o.overrides.push((k.to_string(), num(Some(&val.to_string()), k)?));
            }
            "--audio" => o.audio = true,
            "--tint" => {
                let v = it.next().ok_or("faltou a cor")?;
                o.tint = crate::engine::parse_hex_color(v);
                if o.tint[3] == 0.0 {
                    return Err("--tint espera #rrggbb".into());
                }
            }
            "--out" => o.out = PathBuf::from(it.next().ok_or("faltou a pasta")?),
            other => return Err(format!("opção desconhecida: {other}")),
        }
    }
    if o.width < 16 || o.height < 16 || o.width > MAX_DIM || o.height > MAX_DIM
        || o.width as u64 * o.height as u64 > MAX_PIXELS
    {
        return Err(format!("resolução fora dos limites (16..{MAX_DIM}, até 8K)"));
    }
    if !(1.0..=240.0).contains(&o.fps) {
        return Err("--fps deve estar entre 1 e 240".into());
    }
    if o.frames == 0 || o.frames > MAX_FRAMES || o.times.is_empty() || o.times.len() > MAX_FRAMES {
        return Err(format!("quantidade de frames deve estar entre 1 e {MAX_FRAMES}"));
    }
    Ok(o)
}

#[derive(Serialize)]
struct CompileReport {
    ok: bool,
    ms: f64,
    messages: String,
}

#[derive(Serialize)]
struct FrameReport {
    file: String,
    t: f32,
    gpu_ms: Option<f64>,
    cpu_ms: f64,
    /// Luminância média e desvio (0..1).
    luma_mean: f32,
    luma_std: f32,
    /// Fração de pixels quase pretos (< 2/255) e estourados (> 253/255 em algum canal).
    crushed: f32,
    clipped: f32,
    /// Diferença absoluta média em relação ao frame anterior desta execução (0..1).
    delta_prev: Option<f32>,
}

#[derive(Serialize)]
struct Summary {
    gpu_ms_mean: Option<f64>,
    gpu_ms_max: Option<f64>,
    cpu_ms_mean: f64,
    working_set_mb: Option<f64>,
    peak_working_set_mb: Option<f64>,
}

#[derive(Serialize)]
struct Report {
    shader: String,
    source_hash: String,
    command: String,
    width: u32,
    height: u32,
    audio: bool,
    params: Vec<(String, f32)>,
    param_problems: Vec<String>,
    notes: Vec<String>,
    compile: CompileReport,
    frames: Vec<FrameReport>,
    summary: Option<Summary>,
}

/// Ponto de entrada do modo `--lab`. Devolve o código de saída do processo.
pub fn run(args: &[String]) -> i32 {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("erro: {e}\n\n{USAGE}");
            return 1;
        }
    };
    let mut renderer = match OffscreenRenderer::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("erro: não foi possível criar o dispositivo D3D11: {e}");
            return 4;
        }
    };
    if opts.command != "watch" {
        return run_once(&opts, &mut renderer);
    }

    println!("observando {} (Ctrl+C para sair)", opts.file.display());
    let mut last = None;
    loop {
        let modified = std::fs::metadata(&opts.file).and_then(|m| m.modified()).ok();
        if modified.is_some() && modified != last {
            last = modified;
            let code = run_once(&Options { command: "render".into(), ..opts.clone() }, &mut renderer);
            if code == 3 {
                // A thread do compilador ficou presa; não há como cancelá-la com segurança.
                return code;
            }
            println!("aguardando alterações...");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn run_once(opts: &Options, renderer: &mut OffscreenRenderer) -> i32 {
    let stem = opts.file.file_stem().and_then(|s| s.to_str()).unwrap_or("shader").to_string();
    let source = match read_source(&opts.file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("erro: {e}");
            return 1;
        }
    };
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut h);
        format!("{:016x}", h.finish())
    };

    let dir = opts.out.join(&stem).join(run_stamp());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("erro: não foi possível criar {}: {e}", dir.display());
        return 4;
    }
    let _ = std::fs::write(dir.join(format!("{stem}.hlsl")), &source);

    let (specs, param_problems) = params::parse(&source);
    let mut notes = Vec::new();
    let values = resolve_params(&specs, &opts.overrides, &mut notes);
    if source.contains("cbuffer") {
        notes.push("o shader declara o próprio cbuffer: common.hlsli não é incluído e o contrato não é garantido".into());
    }

    let mut report = Report {
        shader: opts.file.display().to_string(),
        source_hash: hash,
        command: opts.command.clone(),
        width: opts.width,
        height: opts.height,
        audio: opts.audio,
        params: specs.iter().zip(values).map(|(s, v)| (s.name.clone(), v)).collect(),
        param_problems,
        notes,
        compile: CompileReport { ok: false, ms: 0.0, messages: String::new() },
        frames: Vec::new(),
        summary: None,
    };

    // Compilação num processo filho com prazo: no timeout o filho é encerrado de verdade.
    let code = crate::wallpaper::catalog::with_common(&source);
    let name = format!("{stem}.hlsl");
    let started = Instant::now();
    let compiled = crate::compiler_worker::compile(&code, &name, COMPILE_TIMEOUT);
    report.compile.ms = started.elapsed().as_secs_f64() * 1000.0;

    let exit = match compiled {
        Err(WorkerError::Timeout) => {
            report.compile.messages = format!("tempo de compilação excedeu {}s", COMPILE_TIMEOUT.as_secs());
            3
        }
        Err(e) => {
            report.compile.messages = format!("o processo de compilação falhou: {e:?}");
            4
        }
        Ok(Err(msg)) => {
            report.compile.messages = msg;
            2
        }
        Ok(Ok(bytes)) => {
            report.compile.ok = true;
            report.compile.messages = bytes.messages.clone();
            match ShaderPipeline::from_bytecode(renderer.device(), &bytes.vs, &bytes.ps) {
                Ok(pipeline) => {
                    renderer.insert(&stem, pipeline, &source);
                    render_frames(opts, renderer, &stem, &dir, values, &mut report)
                }
                Err(e) => {
                    report.compile.messages.push_str(&format!("\ncriação do pipeline falhou: {e}"));
                    4
                }
            }
        }
    };

    print_report(&report, &dir);
    if let Ok(json) = serde_json::to_string_pretty(&report) {
        let _ = std::fs::write(dir.join("report.json"), json);
    }
    exit
}

fn read_source(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!("fonte maior que {} KiB", MAX_SOURCE_BYTES / 1024));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn resolve_params(specs: &[ParamSpec], overrides: &[(String, f32)], notes: &mut Vec<String>) -> [f32; 8] {
    let mut values = params::defaults(specs);
    for (name, v) in overrides {
        match specs.iter().position(|s| &s.name == name) {
            Some(i) => {
                values[i] = specs[i].clamp(*v);
                if values[i] != *v {
                    notes.push(format!("--param {name}={v} limitado a {}", values[i]));
                }
            }
            None => notes.push(format!("--param {name}: parâmetro não declarado, ignorado")),
        }
    }
    values
}

fn render_frames(
    opts: &Options,
    renderer: &mut OffscreenRenderer,
    key: &str,
    dir: &Path,
    values: [f32; 8],
    report: &mut Report,
) -> i32 {
    let jobs: Vec<(String, f32)> = if opts.command == "seq" {
        (0..opts.frames).map(|i| (format!("f{i:04}.png"), opts.start + i as f32 / opts.fps)).collect()
    } else {
        opts.times.iter().map(|&t| (format!("t{t:07.3}.png"), t)).collect()
    };

    let mut prev: Option<Vec<u8>> = None;
    for (file, t) in jobs {
        let constants = lab_constants(opts, t, values);
        let started = Instant::now();
        let (pixels, gpu_ms) = match renderer.render_timed(key, constants) {
            Ok(r) => r,
            Err(e) => {
                report.compile.messages.push_str(&format!("\nrenderização falhou em t={t}: {e}"));
                return 4;
            }
        };
        let cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        let stats = image_stats(&pixels, prev.as_deref());
        if let Err(e) = write_png(&dir.join(&file), &pixels, opts.width, opts.height) {
            report.compile.messages.push_str(&format!("\nfalha ao gravar {file}: {e}"));
            return 4;
        }
        report.frames.push(FrameReport {
            file,
            t,
            gpu_ms,
            cpu_ms,
            luma_mean: stats.0,
            luma_std: stats.1,
            crushed: stats.2,
            clipped: stats.3,
            delta_prev: stats.4,
        });
        prev = Some(pixels);
    }

    let gpu: Vec<f64> = report.frames.iter().filter_map(|f| f.gpu_ms).collect();
    let n = report.frames.len().max(1) as f64;
    let (ws, peak) = memory_mb();
    report.summary = Some(Summary {
        gpu_ms_mean: (!gpu.is_empty()).then(|| gpu.iter().sum::<f64>() / gpu.len() as f64),
        gpu_ms_max: gpu.iter().cloned().reduce(f64::max),
        cpu_ms_mean: report.frames.iter().map(|f| f.cpu_ms).sum::<f64>() / n,
        working_set_mb: ws,
        peak_working_set_mb: peak,
    });
    0
}

/// Constantes determinísticas: relógio fixo (2026-01-01 12:00:00), mouse no centro,
/// parallax desligado e silêncio, a menos que `--audio` peça o áudio sintético.
fn lab_constants(opts: &Options, t: f32, values: [f32; 8]) -> ShaderConstants {
    let mut c = preview_constants(opts.width, opts.height, t, opts.tint);
    c.clock_time = [12.0, 0.0, 0.0, 0.0];
    c.clock_date = [2026.0, 1.0, 1.0, 4.0];
    if !opts.audio {
        c.audio_reactive = [0.0; 4];
        c.audio_spectrum = [0.0; 16];
    }
    c.params = values;
    c
}

pub(crate) fn image_stats(px: &[u8], prev: Option<&[u8]>) -> (f32, f32, f32, f32, Option<f32>) {
    let n = (px.len() / 4).max(1) as f64;
    let (mut sum, mut sq, mut crushed, mut clipped) = (0f64, 0f64, 0u64, 0u64);
    for p in px.as_chunks::<4>().0 {
        let l = (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
        sum += l;
        sq += l * l;
        if p[0] < 2 && p[1] < 2 && p[2] < 2 {
            crushed += 1;
        }
        if p[0] > 253 || p[1] > 253 || p[2] > 253 {
            clipped += 1;
        }
    }
    let mean = sum / n;
    let std = (sq / n - mean * mean).max(0.0).sqrt();
    let delta = prev.filter(|p| p.len() == px.len()).map(|p| {
        let d: u64 = p
            .as_chunks::<4>().0
            .iter()
            .zip(px.as_chunks::<4>().0)
            .map(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i]) as u64).sum::<u64>())
            .sum();
        (d as f64 / (n * 3.0 * 255.0)) as f32
    });
    (mean as f32, std as f32, (crushed as f64 / n) as f32, (clipped as f64 / n) as f32, delta)
}

fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    // O shader escreve valores de exibição num alvo UNORM: marcamos o PNG como sRGB.
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    // Alfa do shader não é usado pelo desktop: grava opaco para o PNG bater com a tela.
    let opaque: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    writer.write_image_data(&opaque).map_err(|e| e.to_string())
}

fn memory_mb() -> (Option<f64>, Option<f64>) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb).is_ok() };
    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    if ok { (Some(mb(c.WorkingSetSize)), Some(mb(c.PeakWorkingSetSize))) } else { (None, None) }
}

fn run_stamp() -> String {
    let (t, d) = crate::engine::system_clock();
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}-{ms:03}", d[0], d[1], d[2], t[0], t[1], t[2] as u32)
}

fn print_report(r: &Report, dir: &Path) {
    let status = if r.compile.ok { "ok" } else { "FALHOU" };
    println!("── {} [{}] compilação {status} em {:.1} ms", r.shader, r.source_hash, r.compile.ms);
    for line in r.compile.messages.lines().filter(|l| !l.trim().is_empty()) {
        println!("   {line}");
    }
    for p in r.param_problems.iter().chain(&r.notes) {
        println!("   aviso: {p}");
    }
    if !r.params.is_empty() {
        let list: Vec<String> = r.params.iter().map(|(n, v)| format!("{n}={v}")).collect();
        println!("   params: {}", list.join(" "));
    }
    for f in &r.frames {
        let gpu = f.gpu_ms.map(|g| format!("{g:6.2} ms")).unwrap_or_else(|| "   n/d   ".into());
        let delta = f.delta_prev.map(|d| format!(" Δ{d:.4}")).unwrap_or_default();
        println!(
            "   {} t={:<7.3} gpu {gpu}  luma {:.3}±{:.3} preto {:.1}% estouro {:.1}%{delta}",
            f.file, f.t, f.luma_mean, f.luma_std, f.crushed * 100.0, f.clipped * 100.0
        );
    }
    if let Some(s) = &r.summary {
        let fmt = |v: Option<f64>| v.map(|v| format!("{v:.2}")).unwrap_or_else(|| "n/d".into());
        println!(
            "   gpu médio {} ms (máx {}) · cpu médio {:.1} ms/frame (inclui readback e PNG) · memória {} MB (pico {})",
            fmt(s.gpu_ms_mean), fmt(s.gpu_ms_max), s.cpu_ms_mean, fmt(s.working_set_mb), fmt(s.peak_working_set_mb)
        );
    }
    println!("   saída: {}", dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_options_and_limits() {
        let o = parse_args(&args("seq a.hlsl --size 640x360 --frames 10 --fps 24 --param k=2")).unwrap();
        assert_eq!((o.width, o.height, o.frames), (640, 360, 10));
        assert_eq!(o.overrides, vec![("k".to_string(), 2.0)]);
        assert!(parse_args(&args("render a.hlsl --size 99999x10")).is_err());
        assert!(parse_args(&args("render a.hlsl --frames 5000")).is_err());
        assert!(parse_args(&args("explode a.hlsl")).is_err());
    }

    #[test]
    fn overrides_are_clamped() {
        let specs = params::parse("// param: k 0 1 0.5").0;
        let mut notes = Vec::new();
        let v = resolve_params(&specs, &[("k".into(), 9.0), ("x".into(), 1.0)], &mut notes);
        assert_eq!(v[0], 1.0);
        assert_eq!(notes.len(), 2);
    }
}
