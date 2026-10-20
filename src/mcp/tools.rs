//! Ferramentas do servidor MCP: definições (JSON Schema) e execução.
//!
//! Textos voltados ao modelo ficam em inglês; os guias (`get_guide`) saem como estão em `docs/`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

use super::psl::{self, Compiled, Failure, Lang};

const SHADER_GUIDE: &str = include_str!("../../docs/shader-contract.md");
const WEB_GUIDE: &str = include_str!("../../docs/web-wallpaper-guide.md");
const WORKFLOW_GUIDE: &str = include_str!("../../docs/mcp-workflow.md");

const MAX_PREVIEW_PIXELS: u64 = 1920 * 1080;
const MAX_FRAMES: usize = 6;

#[derive(Default)]
pub struct State {
    session: Option<psl::Session>,
}

pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
    /// As mesmas imagens de `content`, em PNG, para quem não fala MCP (o agente do Studio).
    pub pngs: Vec<Vec<u8>>,
    /// Arquivo gravado por esta chamada (wallpaper salvo).
    pub saved: Option<PathBuf>,
}

impl ToolResult {
    fn text(s: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": s.into()})], is_error: false, pngs: Vec::new(), saved: None }
    }
    fn error(s: impl Into<String>) -> Self {
        Self { is_error: true, ..Self::text(s) }
    }
    fn image(mut self, png: &[u8]) -> Self {
        self.content.push(json!({"type": "image", "data": crate::media::base64(png), "mimeType": "image/png"}));
        self.pngs.push(png.to_vec());
        self
    }
    fn saved(mut self, path: PathBuf) -> Self {
        self.saved = Some(path);
        self
    }
    pub fn to_json(&self) -> Value {
        json!({"content": self.content, "isError": self.is_error})
    }
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

pub fn definitions() -> Value {
    let code = json!({"type": "string", "description": "Shader source code (max 256 KiB)."});
    let language = json!({
        "type": "string", "enum": ["hlsl", "glsl"],
        "description": "\"hlsl\" (native, follows the contract from get_guide) or \"glsl\" (Shadertoy-style mainImage, converted automatically). Omit to auto-detect."
    });
    json!([
        tool(
            "get_guide",
            "Read the authoring guide BEFORE writing anything. topic \"shader\": the HLSL contract (inputs, uniforms, limits) and the Shadertoy/GLSL import rules. \"web\": how to write an HTML wallpaper (no network, window.engine bridge). \"workflow\": the recommended create-and-iterate loop, and static images.",
            json!({"topic": {"type": "string", "enum": ["shader", "web", "workflow"]}}),
            &["topic"],
        ),
        tool(
            "psl_compile",
            "Compile a shader without rendering. Returns compiler errors with line numbers (for GLSL they refer to your GLSL source) or OK plus warnings and the declared `// param:` parameters.",
            json!({"code": code, "language": language}),
            &["code"],
        ),
        tool(
            "psl_render",
            "Compile and render preview frames of a shader at the given times, returned as images, plus objective metrics (GPU ms, brightness, blown/black pixels, motion between frames) and hints. Output is deterministic (fixed clock, silent audio, mouse centered). Look at the images and the hints, fix problems, and call again. Frames are limited to 1920x1080 and 6 per call; the default 800x450 keeps it cheap.",
            json!({
                "code": code, "language": language,
                "times": {"type": "array", "items": {"type": "number"}, "description": "Times in seconds, 1-6 values. Default [1, 6, 12]. Use spread-out times to judge animation."},
                "width": {"type": "integer", "description": "Default 800."},
                "height": {"type": "integer", "description": "Default 450."},
                "params": {"type": "object", "additionalProperties": {"type": "number"}, "description": "Overrides for `// param:` values, by name."}
            }),
            &["code"],
        ),
        tool(
            "render_still",
            "Render one frame at high resolution (up to 7680x4320) and save it as a PNG static-image wallpaper. Returns the saved path and a small preview. Apply it with apply_wallpaper. Use psl_render first to get the look right.",
            json!({
                "code": code, "language": language,
                "name": {"type": "string", "description": "Wallpaper name (used for the file name)."},
                "time": {"type": "number", "description": "Which instant to freeze, in seconds. Default 6."},
                "width": {"type": "integer", "description": "Default 3840."},
                "height": {"type": "integer", "description": "Default 2160."},
                "params": {"type": "object", "additionalProperties": {"type": "number"}}
            }),
            &["code", "name"],
        ),
        tool(
            "render_page_still",
            "Render an HTML/SVG page ONCE at high resolution (default 3840x2160) and save the screenshot as a PNG static-image wallpaper (about 0% GPU). Use it when the user wants an image: an illustration, blueprint, poster, typography or any picture with no motion. Draw the artwork as inline SVG or CSS with no animation, check it with web_preview first, then call this. Returns the saved path.",
            json!({
                "html": {"type": "string", "description": "A complete single-file HTML document (inline SVG/CSS/JS, no network)."},
                "name": {"type": "string", "description": "Wallpaper name (used for the file name)."},
                "width": {"type": "integer", "description": "Default 3840."},
                "height": {"type": "integer", "description": "Default 2160."},
                "wait_ms": {"type": "integer", "description": "How long to wait after load before the screenshot, 0-15000. Default 2500."}
            }),
            &["html", "name"],
        ),
        tool(
            "web_preview",
            "Render an HTML wallpaper exactly like the desktop does (same WebView2 and the same no-network security policy) and return screenshots. Use it for every web wallpaper before saving. Call get_guide with topic \"web\" first.",
            json!({
                "html": {"type": "string", "description": "A complete single-file HTML document (CSS and JS inline, no network)."},
                "width": {"type": "integer", "description": "Default 800. Try another aspect ratio too."},
                "height": {"type": "integer", "description": "Default 450."},
                "capture_after_ms": {"type": "array", "items": {"type": "integer"}, "description": "Milliseconds after load for each screenshot, 1-4 values. Default [1500, 5000]."}
            }),
            &["html"],
        ),
        tool(
            "save_wallpaper",
            "Save a shader (HLSL or GLSL, which must compile) or an HTML page into the user's imported-wallpapers folder. Never overwrites an existing wallpaper with different content. Returns the path to give to apply_wallpaper.",
            json!({
                "kind": {"type": "string", "enum": ["shader", "web"]},
                "name": {"type": "string"},
                "code": {"type": "string", "description": "Shader source or the HTML document."},
                "language": language
            }),
            &["kind", "name", "code"],
        ),
        tool(
            "apply_wallpaper",
            "Apply a wallpaper on the desktop through the running IsNotAEngine app. `wallpaper` is a catalog id (see list_wallpapers) or a file path returned by save_wallpaper / render_still. Without `monitor`, applies to all monitors.",
            json!({"wallpaper": {"type": "string"}, "monitor": {"type": "integer", "description": "Monitor index, 0-based."}}),
            &["wallpaper"],
        ),
        tool(
            "list_wallpapers",
            "List the built-in catalog and the imported wallpapers, with ids usable in apply_wallpaper. Requires the IsNotAEngine app to be running.",
            json!({}),
            &[],
        ),
    ])
}

pub fn call(state: &mut State, name: &str, args: &Value) -> ToolResult {
    let result = match name {
        "get_guide" => get_guide(args),
        "psl_compile" => psl_compile(args),
        "psl_render" => psl_render(state, args),
        "render_still" => render_still(state, args),
        "web_preview" => web_preview(args),
        "render_page_still" => render_page_still(args),
        "save_wallpaper" => save_wallpaper(args),
        "apply_wallpaper" => apply_wallpaper(args),
        "list_wallpapers" => list_wallpapers(),
        _ => Err(format!("unknown tool: {name}")),
    };
    result.unwrap_or_else(ToolResult::error)
}

// ---------------------------------------------------------------- argumentos

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing or invalid \"{key}\" (string expected)"))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn opt_u32(args: &Value, key: &str, default: u32) -> Result<u32, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("\"{key}\" must be a non-negative integer")),
    }
}

fn overrides(args: &Value) -> Vec<(String, f32)> {
    args.get("params")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n as f32))).collect())
        .unwrap_or_default()
}

fn check_size(w: u32, h: u32, max_pixels: u64) -> Result<(), String> {
    if !(16..=7680).contains(&w) || !(16..=4320).contains(&h) {
        return Err(format!("size {w}x{h} is out of range (16..7680 x 16..4320)"));
    }
    if u64::from(w) * u64::from(h) > max_pixels {
        return Err(format!("{w}x{h} has more than {max_pixels} pixels"));
    }
    Ok(())
}

fn compile_args(args: &Value) -> Result<Result<Compiled, Failure>, String> {
    let code = str_arg(args, "code")?;
    let lang = Lang::parse(opt_str(args, "language"), code)?;
    Ok(psl::compile(code, lang))
}

fn compile_error(f: &Failure) -> ToolResult {
    ToolResult::error(format!("COMPILE ERROR\n{}", f.message()))
}

fn session(state: &mut State) -> Result<&mut psl::Session, String> {
    if state.session.is_none() {
        state.session = Some(psl::Session::new()?);
    }
    Ok(state.session.as_mut().expect("sessão criada acima"))
}

pub(crate) fn wallpapers_dir() -> PathBuf {
    crate::config::EngineConfig::config_path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("wallpapers")
}

fn describe_params(c: &Compiled) -> String {
    if c.params.is_empty() {
        return String::new();
    }
    let list: Vec<String> = c.params.iter().map(|p| format!("{} [{}..{}] default {}", p.name, p.min, p.max, p.default)).collect();
    format!("Declared parameters: {}\n", list.join("; "))
}

// ---------------------------------------------------------------- ferramentas

fn get_guide(args: &Value) -> Result<ToolResult, String> {
    Ok(ToolResult::text(match str_arg(args, "topic")? {
        "shader" => SHADER_GUIDE,
        "web" => WEB_GUIDE,
        "workflow" => WORKFLOW_GUIDE,
        other => return Err(format!("unknown topic \"{other}\": use shader, web or workflow")),
    }))
}

fn psl_compile(args: &Value) -> Result<ToolResult, String> {
    Ok(match compile_args(args)? {
        Err(f) => compile_error(&f),
        Ok(c) => {
            let mut out = format!("COMPILED OK in {:.0} ms.\n", c.compile_ms);
            out.push_str(&describe_params(&c));
            for n in &c.notes {
                out.push_str(&format!("Note: {n}\n"));
            }
            let warnings = c.bytes.messages.trim();
            if !warnings.is_empty() {
                out.push_str(&format!("Compiler warnings:\n{warnings}\n"));
            }
            ToolResult::text(out)
        }
    })
}

fn psl_render(state: &mut State, args: &Value) -> Result<ToolResult, String> {
    let (w, h) = (opt_u32(args, "width", 800)?, opt_u32(args, "height", 450)?);
    check_size(w, h, MAX_PREVIEW_PIXELS)?;
    let times: Vec<f32> = match args.get("times") {
        None | Some(Value::Null) => vec![1.0, 6.0, 12.0],
        Some(v) => v
            .as_array()
            .ok_or("\"times\" must be an array of numbers")?
            .iter()
            .map(|t| t.as_f64().filter(|t| (0.0..=3600.0).contains(t)).map(|t| t as f32).ok_or("each time must be a number between 0 and 3600"))
            .collect::<Result<_, _>>()?,
    };
    if times.is_empty() || times.len() > MAX_FRAMES {
        return Err(format!("\"times\" needs 1 to {MAX_FRAMES} values"));
    }
    let compiled = match compile_args(args)? {
        Ok(c) => c,
        Err(f) => return Ok(compile_error(&f)),
    };
    let frames = session(state)?.render(&compiled, &times, w, h, &overrides(args))?;

    let mut out = format!("COMPILED OK in {:.0} ms. Rendered {} frame(s) at {w}x{h}.\n", compiled.compile_ms, frames.len());
    out.push_str(&describe_params(&compiled));
    for n in &compiled.notes {
        out.push_str(&format!("Note: {n}\n"));
    }
    let warnings = compiled.bytes.messages.trim();
    if !warnings.is_empty() {
        out.push_str(&format!("Compiler warnings:\n{warnings}\n"));
    }
    for f in &frames {
        out.push_str(&format!(
            "t={:.1}s  gpu {}  luma {:.2}±{:.2}  black {:.0}%  blown {:.0}%{}\n",
            f.t,
            f.gpu_ms.map_or("n/a".into(), |g| format!("{g:.2} ms")),
            f.luma_mean,
            f.luma_std,
            f.crushed * 100.0,
            f.clipped * 100.0,
            f.delta_prev.map_or(String::new(), |d| format!("  motion {d:.3}")),
        ));
    }
    let hints = psl::hints(&frames, w, h);
    if hints.is_empty() {
        out.push_str("No issues detected by the metrics. Judge the images.\n");
    } else {
        out.push_str("Hints:\n");
        for hint in hints {
            out.push_str(&format!("- {hint}\n"));
        }
    }
    let mut result = ToolResult::text(out);
    for f in &frames {
        let png = crate::render::offscreen::encode_png(&f.rgba, w, h).ok_or("could not encode a frame")?;
        result = result.image(&png);
    }
    Ok(result)
}

fn render_still(state: &mut State, args: &Value) -> Result<ToolResult, String> {
    let (w, h) = (opt_u32(args, "width", 3840)?, opt_u32(args, "height", 2160)?);
    check_size(w, h, psl::MAX_PIXELS)?;
    let time = args.get("time").and_then(Value::as_f64).unwrap_or(6.0).clamp(0.0, 3600.0) as f32;
    let stem = crate::shadertoy::file_stem(str_arg(args, "name")?);
    let compiled = match compile_args(args)? {
        Ok(c) => c,
        Err(f) => return Ok(compile_error(&f)),
    };
    let started = std::time::Instant::now();
    let session = session(state)?;
    let big = session.render(&compiled, &[time], w, h, &overrides(args))?.remove(0);
    let png = crate::render::offscreen::encode_png(&big.rgba, w, h).ok_or("could not encode the PNG")?;

    let dir = wallpapers_dir();
    let tmp = std::env::temp_dir().join("isnotaengine_mcp");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let tmp_file = tmp.join(format!("{stem}_{w}x{h}.png"));
    std::fs::write(&tmp_file, &png).map_err(|e| format!("could not write the image: {e}"))?;
    let saved = crate::import::import_into(&tmp_file, &dir).map_err(|e| format!("could not save into the wallpapers folder: {e}"))?;
    let _ = std::fs::remove_file(&tmp_file);

    let mut result = ToolResult::text(format!(
        "Saved a {w}x{h} static wallpaper ({:.1} MB, rendered at t={time:.1}s in {:.1}s).\nPath: {}\nApply it with apply_wallpaper using this path. Static images cost ~0% GPU.\n",
        png.len() as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64(),
        saved.display()
    ));
    result = result.saved(saved);
    // Miniatura para o modelo conferir o resultado sem gastar tokens com um 4K.
    let (pw, ph) = (640u32, (640.0 * f64::from(h) / f64::from(w)).round().max(16.0) as u32);
    if let Ok(mut small) = session.render(&compiled, &[time], pw, ph, &overrides(args)) {
        let f = small.remove(0);
        if let Some(png) = crate::render::offscreen::encode_png(&f.rgba, pw, ph) {
            result = result.image(&png);
        }
    }
    Ok(result)
}

/// Largura e altura gravadas no cabeçalho (IHDR) de um PNG.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let b = png.get(16..24)?;
    Some((u32::from_be_bytes(b[0..4].try_into().ok()?), u32::from_be_bytes(b[4..8].try_into().ok()?)))
}

fn render_page_still(args: &Value) -> Result<ToolResult, String> {
    render_page_still_in(args, &wallpapers_dir())
}

/// Uma página HTML/SVG vira uma imagem estática (PNG) na pasta de wallpapers: custa ~0% de GPU, ao contrário
/// de uma página animada. A página roda no mesmo WebView2 do desktop, sem rede.
pub(crate) fn render_page_still_in(args: &Value, dir: &std::path::Path) -> Result<ToolResult, String> {
    let html = str_arg(args, "html")?;
    let (w, h) = (opt_u32(args, "width", 3840)?, opt_u32(args, "height", 2160)?);
    check_size(w, h, 3840 * 2160)?;
    let wait = opt_u32(args, "wait_ms", 2500)?.min(15_000);
    let stem = crate::shadertoy::file_stem(str_arg(args, "name")?);
    let started = std::time::Instant::now();
    let png = super::web::preview(html, w, h, &[wait])?.into_iter().next().ok_or("the screenshot failed")?;
    let (pw, ph) = png_size(&png).unwrap_or((w, h));

    let tmp = std::env::temp_dir().join("isnotaengine_mcp");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let tmp_file = tmp.join(format!("{stem}_{pw}x{ph}.png"));
    std::fs::write(&tmp_file, &png).map_err(|e| format!("could not write the image: {e}"))?;
    let saved = crate::import::import_into(&tmp_file, dir).map_err(|e| format!("could not save into the wallpapers folder: {e}"))?;
    let _ = std::fs::remove_file(&tmp_file);
    let note = if (pw, ph) != (w, h) { format!("\nNote: the screenshot is {pw}x{ph}, not the requested {w}x{h} (the display scale changes it)." ) } else { String::new() };
    Ok(ToolResult::text(format!(
        "Saved a {pw}x{ph} static image wallpaper ({:.1} MB, rendered in {:.1}s).\nPath: {}\nStatic images cost ~0% GPU.{note}\n",
        png.len() as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64(),
        saved.display()
    ))
    .saved(saved))
}

fn web_preview(args: &Value) -> Result<ToolResult, String> {
    let html = str_arg(args, "html")?;
    let (w, h) = (opt_u32(args, "width", 800)?, opt_u32(args, "height", 450)?);
    check_size(w, h, MAX_PREVIEW_PIXELS)?;
    let waits: Vec<u32> = match args.get("capture_after_ms") {
        None | Some(Value::Null) => vec![1500, 5000],
        Some(v) => v
            .as_array()
            .ok_or("\"capture_after_ms\" must be an array of integers")?
            .iter()
            .map(|n| n.as_u64().filter(|n| *n <= 60_000).map(|n| n as u32).ok_or("each capture time must be an integer up to 60000"))
            .collect::<Result<_, _>>()?,
    };
    if waits.is_empty() || waits.len() > 4 {
        return Err("\"capture_after_ms\" needs 1 to 4 values".into());
    }
    let shots = super::web::preview(html, w, h, &waits)?;
    let mut result = ToolResult::text(format!(
        "Rendered {} screenshot(s) at {w}x{h} with the desktop's WebView2 and no-network policy (after {:?} ms).\nIf the page is blank, look for a JavaScript error: blocked network access (fetch, CDNs, Google Fonts) is the usual cause.\n",
        shots.len(),
        waits
    ));
    for png in &shots {
        result = result.image(png);
    }
    Ok(result)
}

fn save_wallpaper(args: &Value) -> Result<ToolResult, String> {
    save_wallpaper_in(args, &wallpapers_dir())
}

/// `save_wallpaper` numa pasta escolhida: o Criar com IA usa para guardar o rascunho quando o modelo não salvou.
pub(crate) fn save_wallpaper_in(args: &Value, dir: &std::path::Path) -> Result<ToolResult, String> {
    let kind = str_arg(args, "kind")?;
    let code = str_arg(args, "code")?;
    let stem = crate::shadertoy::file_stem(str_arg(args, "name")?);
    let (body, ext, mut notes): (String, &str, Vec<String>) = match kind {
        "shader" => {
            let lang = Lang::parse(opt_str(args, "language"), code)?;
            match psl::compile(code, lang) {
                Ok(c) => (c.hlsl, "hlsl", c.notes),
                Err(f) => return Ok(compile_error(&f)),
            }
        }
        "web" => {
            if code.len() > psl::MAX_SOURCE_BYTES {
                return Err(format!("the page is larger than {} KiB", psl::MAX_SOURCE_BYTES / 1024));
            }
            if !code.to_ascii_lowercase().contains("<html") && !code.to_ascii_lowercase().contains("<body") {
                return Err("the HTML must be a complete document (with <html> or <body>)".into());
            }
            let mut notes = Vec::new();
            if code.contains("http://") || code.contains("https://") {
                notes.push("The page mentions http(s) URLs. Imported pages run with the network blocked: inline everything (use data: URIs).".to_string());
            }
            (code.to_string(), "html", notes)
        }
        other => return Err(format!("unknown kind \"{other}\": use shader or web")),
    };

    let tmp = std::env::temp_dir().join("isnotaengine_mcp");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let tmp_file = tmp.join(format!("{stem}.{ext}"));
    std::fs::write(&tmp_file, body).map_err(|e| format!("could not write the file: {e}"))?;
    let saved = crate::import::import_into(&tmp_file, dir).map_err(|e| format!("could not save: {e}"))?;
    let _ = std::fs::remove_file(&tmp_file);

    notes.insert(0, format!("Saved {kind} wallpaper: {}", saved.display()));
    notes.push("Apply it with apply_wallpaper using this path.".into());
    Ok(ToolResult::text(notes.join("\n")).saved(saved))
}

// ---------------------------------------------------------------- canal de controle do app

/// Uma requisição ao canal de controle da engine (named pipe local, mesmo usuário).
fn pipe_request(req: &Value) -> Result<Value, String> {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(crate::control::PIPE_NAME)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "The IsNotAEngine app is not running. Ask the user to start it, then try again.".to_string()
            } else {
                format!("could not reach the IsNotAEngine app: {e}")
            }
        })?;
    writeln!(pipe, "{req}").and_then(|_| pipe.flush()).map_err(|e| format!("could not send the command: {e}"))?;
    let mut line = String::new();
    BufReader::new(&pipe).read_line(&mut line).map_err(|e| format!("no reply from the app: {e}"))?;
    serde_json::from_str(line.trim()).map_err(|e| format!("invalid reply from the app: {e}"))
}

fn apply_wallpaper(args: &Value) -> Result<ToolResult, String> {
    let wallpaper = str_arg(args, "wallpaper")?;
    let mut req = json!({"cmd": "set_wallpaper", "wallpaper": wallpaper});
    if let Some(m) = args.get("monitor").filter(|m| !m.is_null()) {
        req["monitor"] = json!(m.as_u64().ok_or("\"monitor\" must be a non-negative integer")?);
    }
    let reply = pipe_request(&req)?;
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(ToolResult::text(format!("Applied \"{wallpaper}\"{}.", args.get("monitor").map_or(" on all monitors".into(), |m| format!(" on monitor {m}")))))
    } else {
        Err(format!("the app refused: {}", reply.get("error").and_then(Value::as_str).unwrap_or("unknown error")))
    }
}

fn list_wallpapers() -> Result<ToolResult, String> {
    let reply = pipe_request(&json!({"cmd": "list_wallpapers"}))?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(format!("the app refused: {}", reply.get("error").and_then(Value::as_str).unwrap_or("unknown error")));
    }
    let list = reply.get("wallpapers").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = format!("{} wallpapers (id | title | category | kind | source):\n", list.len());
    for w in &list {
        let s = |k: &str| w.get(k).and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("{} | {} | {} | {} | {}\n", s("id"), s("title"), s("category"), s("kind"), s("source")));
    }
    Ok(ToolResult::text(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(r: &ToolResult) -> String {
        r.content.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn every_tool_has_a_schema_and_a_handler() {
        let defs = definitions();
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 9);
        let mut state = State::default();
        for name in names {
            // argumentos vazios: nunca pânico, e nenhuma ferramenta pode ser "desconhecida"
            let r = call(&mut state, name, &json!({}));
            assert!(!text_of(&r).starts_with("unknown tool"), "{name} sem handler");
        }
        for d in defs.as_array().unwrap() {
            assert_eq!(d["inputSchema"]["type"], "object");
            assert!(d["description"].as_str().unwrap().len() > 40, "{} sem descrição útil", d["name"]);
        }
        assert!(text_of(&call(&mut state, "format_c", &json!({}))).starts_with("unknown tool"));
    }

    #[test]
    fn guides_are_served() {
        let mut s = State::default();
        for (topic, needle) in [("shader", "Contrato de shader"), ("web", "Guia de wallpapers web"), ("workflow", "psl_render")] {
            let r = call(&mut s, "get_guide", &json!({"topic": topic}));
            assert!(!r.is_error && text_of(&r).contains(needle), "{topic}");
        }
        assert!(call(&mut s, "get_guide", &json!({"topic": "x"})).is_error);
    }

    #[test]
    fn compile_reports_errors_as_tool_errors_with_text() {
        let mut s = State::default();
        let r = call(&mut s, "psl_compile", &json!({"code": "float4 ps_main(float4 p : SV_Position) : SV_Target { return nada; }"}));
        assert!(r.is_error && text_of(&r).starts_with("COMPILE ERROR") && text_of(&r).contains("nada"));
        let ok = call(&mut s, "psl_compile", &json!({"code": "// param: brilho 0 2 1\nfloat4 ps_main(float4 p : SV_Position) : SV_Target { return float4(P(0).xxx, 1); }"}));
        assert!(!ok.is_error, "{}", text_of(&ok));
        assert!(text_of(&ok).contains("brilho [0..2] default 1"), "{}", text_of(&ok));
    }

    #[test]
    fn render_returns_images_metrics_and_validates_limits() {
        let mut s = State::default();
        let code = "void mainImage(out vec4 O, in vec2 U){ vec2 uv = U / iResolution.xy; O = vec4(uv, 0.5 + 0.5 * sin(iTime), 1.); }";
        let r = call(&mut s, "psl_render", &json!({"code": code, "times": [1, 4], "width": 64, "height": 36}));
        assert!(!r.is_error, "{}", text_of(&r));
        assert_eq!(r.content.iter().filter(|c| c["type"] == "image").count(), 2);
        assert!(text_of(&r).contains("Rendered 2 frame(s) at 64x36") && text_of(&r).contains("motion"));
        let img = r.content.iter().find(|c| c["type"] == "image").unwrap();
        assert_eq!(img["mimeType"], "image/png");
        assert!(img["data"].as_str().unwrap().starts_with("iVBOR"), "PNG em base64");

        // limites
        assert!(call(&mut s, "psl_render", &json!({"code": code, "width": 4000, "height": 4000})).is_error);
        assert!(call(&mut s, "psl_render", &json!({"code": code, "times": []})).is_error);
        assert!(call(&mut s, "psl_render", &json!({"code": code, "times": [1, 2, 3, 4, 5, 6, 7]})).is_error);
        assert!(call(&mut s, "psl_render", &json!({"code": code, "times": [-1]})).is_error);
        assert!(call(&mut s, "psl_render", &json!({"code": code, "width": "grande"})).is_error);
    }

    #[test]
    fn save_validates_before_touching_the_disk() {
        let mut s = State::default();
        let bad = call(&mut s, "save_wallpaper", &json!({"kind": "shader", "name": "x", "code": "isso nao compila"}));
        assert!(bad.is_error && text_of(&bad).starts_with("COMPILE ERROR"));
        assert!(call(&mut s, "save_wallpaper", &json!({"kind": "web", "name": "x", "code": "so texto"})).is_error);
        assert!(call(&mut s, "save_wallpaper", &json!({"kind": "video", "name": "x", "code": "x"})).is_error);
        let big = "a".repeat(psl::MAX_SOURCE_BYTES + 1);
        assert!(call(&mut s, "save_wallpaper", &json!({"kind": "web", "name": "x", "code": format!("<html>{big}")})).is_error);
    }

    #[test]
    fn png_size_reads_the_header_and_rejects_garbage() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&3840u32.to_be_bytes());
        png.extend_from_slice(&2160u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((3840, 2160)));
        assert_eq!(png_size(b"nope"), None);
    }
}
