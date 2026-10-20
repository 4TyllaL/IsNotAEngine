//! "Criar com IA" dentro do Studio: o usuário descreve o wallpaper e um modelo (Claude, Gemini ou
//! OpenRouter, com a chave do próprio usuário) escreve, renderiza, olha o resultado e corrige,
//! usando as mesmas ferramentas do servidor MCP.
//!
//! Privacidade: o pedido e os quadros renderizados vão para o provedor escolhido. A chave fica no
//! Gerenciador de Credenciais do Windows. O modelo não aplica nada: o usuário aperta "Aplicar".

pub mod agent;
pub mod http;
pub mod keys;
pub mod provider;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use agent::{Backend, Draft, End, Event, Limits, ToolOutput, Toolbox};
use provider::{Config, Kind, Msg, Part, ToolDef, Turn};

const SHADER_GUIDE: &str = include_str!("../../docs/shader-contract.md");
const WEB_GUIDE: &str = include_str!("../../docs/web-wallpaper-guide.md");
const WORKFLOW_GUIDE: &str = include_str!("../../docs/mcp-workflow.md");

/// Ferramentas que o modelo recebe aqui. Sem `apply_wallpaper`: quem aplica é o usuário.
const ALLOWED_TOOLS: [&str; 6] = ["psl_compile", "psl_render", "render_still", "web_preview", "render_page_still", "save_wallpaper"];

const INTRO: &str = "\
You are the wallpaper artist built into IsNotAEngine, a Windows animated-wallpaper engine. The user describes the wallpaper they want, usually in Portuguese. Reply in the user's language, in one or two short sentences per message: say what you are about to try or what you changed. Never paste code in the chat.

FIRST decide what the user wants. If they ask for an image, picture, illustration, drawing, poster, blueprint, typography or any scene with no mention of motion, make a STILL IMAGE: draw it as static inline SVG (or CSS) in an HTML page, check it with web_preview, then save it with render_page_still, which writes a PNG (a static image costs about 0% GPU). Do NOT save such a request as a live web page. Only make an animated shader (psl_render + save_wallpaper) or a live HTML page (web_preview + save_wallpaper) when the user wants movement, interaction, a clock, audio reaction or similar. If it is unclear, prefer the still image and say so in one sentence.

Work like this: write the shader (GLSL in Shadertoy style, `mainImage`, is usually the best way) or an HTML page, render it with psl_render or web_preview, LOOK at the images and read the hints, fix what is wrong, and render again. Usually 3 to 6 rounds are enough. When it looks good, save it with save_wallpaper (kind \"shader\" or \"web\"); use render_still when the user asks for a static image or for 4K. After saving, tell the user in one sentence what you made. You cannot apply wallpapers: the user presses Apply.

Rules: never save something you have not rendered and looked at. Keep every shader loop small and bounded (at most ~64 iterations) because it runs on the user's GPU, and aim for 4 ms or less at 1080p. Web pages have no network. Dark, calm backgrounds keep desktop icons readable. If the request is impossible here (it needs textures, buffers, or internet data), say so and offer the closest alternative. The guides below are authoritative.";

pub fn system_prompt() -> String {
    format!("{INTRO}\n\n# Shader guide\n\n{SHADER_GUIDE}\n\n# Web wallpaper guide\n\n{WEB_GUIDE}\n\n# Workflow\n\n{WORKFLOW_GUIDE}")
}

/// O modelo precisa saber quantas rodadas tem: com um limite baixo ele gastava tudo testando e nunca chegava a salvar.
fn budget_note(max_steps: usize) -> String {
    let plan = if max_steps <= 3 {
        "Do not explore: write the best version you can in your first reply, render it once, and save it."
    } else {
        "Save no later than your second-to-last round; if rounds are running out, save the best version you have already rendered instead of polishing it further."
    };
    format!("\n\n# Budget\n\nThis run allows at most {max_steps} rounds (a round is one model reply, which may contain several tool calls). {plan} A wallpaper that is saved beats a perfect one that is not. Keep the code compact: a page or shader above roughly 20,000 characters is too big (every round re-sends it, and writing it takes minutes), so draw repeated detail with loops and data tables instead of spelling it out, and add detail in later rounds only if there is budget left.")
}

/// Argumentos de `save_wallpaper` para guardar o melhor rascunho quando a geração acabou sem o modelo salvar.
fn autosave_args(draft: &Draft, stamp: u64) -> Value {
    let mut args = json!({"kind": draft.kind, "name": format!("ia-{stamp}"), "code": draft.code});
    if draft.kind == "shader" {
        args["language"] = json!(draft.language);
    }
    args
}

/// As ferramentas do MCP, sem a parte de protocolo, entregues ao agente.
struct McpToolbox {
    state: crate::mcp::tools::State,
    /// Último código que funcionou e ainda não foi salvo (ver [`Draft`]).
    draft: Option<Draft>,
}

impl McpToolbox {
    fn new() -> Self {
        Self { state: Default::default(), draft: None }
    }

    /// Atualiza o rascunho conforme a ferramenta: renderizar/compilar com sucesso guarda o código;
    /// salvar o wallpaper o limpa (já está a salvo na pasta de wallpapers); erros não mexem.
    fn track(&mut self, name: &str, args: &Value, is_error: bool) {
        if is_error {
            return;
        }
        match name {
            "psl_render" | "psl_compile" => {
                if let Some(code) = args.get("code").and_then(Value::as_str) {
                    let lang = crate::mcp::psl::Lang::parse(args.get("language").and_then(Value::as_str), code).ok();
                    let language = if lang == Some(crate::mcp::psl::Lang::Glsl) { "glsl" } else { "hlsl" };
                    self.draft = Some(Draft { kind: "shader", language, code: code.to_string() });
                }
            }
            "web_preview" => {
                if let Some(html) = args.get("html").and_then(Value::as_str) {
                    self.draft = Some(Draft { kind: "web", language: "html", code: html.to_string() });
                }
            }
            "save_wallpaper" | "render_page_still" => self.draft = None,
            _ => {}
        }
    }
}

/// Quantos rascunhos ficam em disco (os mais antigos são apagados).
const MAX_DRAFTS: usize = 20;

/// Grava o rascunho em `dir` (`rascunho-<carimbo>.<ext>`) e apaga os mais antigos. Devolve o caminho.
fn save_draft(dir: &std::path::Path, draft: &Draft, stamp: u64) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("rascunho-{stamp:012}.{}", draft.language));
    std::fs::write(&path, &draft.code).map_err(|e| e.to_string())?;
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rascunho-"))).collect())
        .unwrap_or_default();
    files.sort();
    let excess = files.len().saturating_sub(MAX_DRAFTS);
    for old in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// Rascunhos que estão em disco, do mais novo para o mais antigo, para a lista "Rascunhos da IA" do Studio.
/// Cada item: `name` (nome do arquivo), `path`, `kind` (`web` ou `shader`), `ext`, `size` e `stamp` (segundos).
fn list_drafts(dir: &std::path::Path) -> Vec<Value> {
    let mut items: Vec<(u64, Value)> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_prefix("rascunho-")?;
            let (stamp, ext) = stem.split_once('.')?;
            let stamp: u64 = stamp.parse().ok()?;
            let kind = match ext {
                "html" => "web",
                "hlsl" | "glsl" => "shader",
                _ => return None,
            };
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            Some((stamp, json!({"name": name, "path": e.path().to_string_lossy(), "kind": kind, "ext": ext, "size": size, "stamp": stamp})))
        })
        .collect();
    items.sort_by_key(|(stamp, _)| std::cmp::Reverse(*stamp));
    items.into_iter().map(|(_, v)| v).collect()
}

/// Apaga um rascunho pelo NOME do arquivo (nunca por caminho): só `rascunho-*` dentro de `dir`.
fn remove_draft(dir: &std::path::Path, name: &str) -> bool {
    let plain = !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != "..";
    plain && name.starts_with("rascunho-") && std::fs::remove_file(dir.join(name)).is_ok()
}

/// Página onde o usuário cria a chave de cada provedor. Endereços fixos: o Studio só diz QUAL provedor.
pub fn key_page(kind: Kind) -> &'static str {
    match kind {
        Kind::Claude => "https://console.anthropic.com/settings/keys",
        Kind::Gemini => "https://aistudio.google.com/apikey",
        Kind::OpenRouter => "https://openrouter.ai/keys",
    }
}

/// Abre a página de chaves no navegador padrão.
pub fn open_key_page(kind: Kind) {
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    use windows::core::{PCWSTR, w};
    let url: Vec<u16> = key_page(kind).encode_utf16().chain(Some(0)).collect();
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(url.as_ptr()), None, None, SW_SHOWNORMAL);
    }
}

impl Toolbox for McpToolbox {
    fn defs(&self) -> Vec<ToolDef> {
        crate::mcp::tools::definitions()
            .as_array()
            .map(|list| {
                list.iter()
                    .filter(|d| d["name"].as_str().is_some_and(|n| ALLOWED_TOOLS.contains(&n)))
                    .map(|d| ToolDef {
                        name: d["name"].as_str().unwrap_or_default().to_string(),
                        description: d["description"].as_str().unwrap_or_default().to_string(),
                        schema: d["inputSchema"].clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolOutput {
        if !ALLOWED_TOOLS.contains(&name) {
            return ToolOutput { parts: vec![Part::Text(format!("unknown tool: {name}")) ], is_error: true, saved: None };
        }
        // Argumentos inválidos (JSON quebrado no OpenRouter) chegam como null: viram objeto vazio e a ferramenta explica.
        let args = if args.is_object() { args.clone() } else { json!({}) };
        let r = crate::mcp::tools::call(&mut self.state, name, &args);
        self.track(name, &args, r.is_error);
        let mut parts: Vec<Part> = r
            .content
            .iter()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str().map(|t| Part::Text(t.to_string())))
            .collect();
        parts.extend(r.pngs.iter().cloned().map(Part::Image));
        ToolOutput { parts, is_error: r.is_error, saved: r.saved }
    }

    fn draft(&self) -> Option<Draft> {
        self.draft.clone()
    }
}

struct HttpBackend {
    cfg: Config,
    cancel: Arc<AtomicBool>,
}

impl Backend for HttpBackend {
    fn complete(&self, system: &str, history: &[Msg], tools: &[ToolDef]) -> Result<Turn, String> {
        let req = provider::build_request(&self.cfg, system, history, tools);
        let reply = http::post_json(&req, &http::DEFAULT_RETRY, &self.cancel)?;
        provider::parse_response(self.cfg.kind, &reply)
    }
}

fn data_uri(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", crate::media::base64(png))
}

fn event_json(e: Event) -> Value {
    match e {
        Event::Thinking => json!({"type": "thinking"}),
        Event::Said(text) => json!({"type": "said", "text": text}),
        Event::Tool { name, summary } => json!({"type": "tool", "name": name, "summary": summary}),
        Event::ToolDone { name, ok, text, images } => {
            json!({"type": "tool_done", "name": name, "ok": ok, "text": text, "images": images.iter().map(|i| data_uri(i)).collect::<Vec<_>>()})
        }
        Event::Saved(path) => json!({"type": "saved", "path": path.to_string_lossy(), "title": path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()}),
    }
}

fn end_json(end: &End) -> (&'static str, String) {
    match end {
        End::Completed => ("completed", String::new()),
        End::Cancelled => ("cancelled", String::new()),
        End::StepLimit => ("step_limit", "Stopped at the round limit you set. Say what to adjust (or \"continue\") and it picks up from here.".into()),
        End::TokenLimit => ("token_limit", "Stopped at the token limit you set. Say what to adjust (or \"continue\") and it picks up from here.".into()),
        End::Refused => ("refused", "The model declined this request.".into()),
        End::Failed(m) => ("failed", m.clone()),
    }
}

/// Envia um evento ao Studio. Quem chama decide como (normalmente `StudioHandle::post_script`).
pub type Emit = Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Hub {
    history: Arc<Mutex<Vec<Msg>>>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    /// Onde guardar rascunhos não salvos; `None` = pasta de dados do app (`ai_drafts`).
    drafts_dir: Option<std::path::PathBuf>,
    /// Onde guardar o que a geração salva sozinha; `None` = a pasta de wallpapers do usuário.
    wallpapers_dir: Option<std::path::PathBuf>,
}

pub struct Request {
    pub kind: Kind,
    pub model: String,
    pub vision: bool,
    pub prompt: String,
    pub limits: Limits,
}

impl Hub {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Esquece a conversa (próximo pedido começa do zero).
    pub fn reset(&self) {
        if !self.is_running() {
            self.history.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    /// Quais provedores já têm chave guardada.
    pub fn key_status() -> Value {
        Kind::ALL.iter().map(|k| (k.id().to_string(), json!(keys::load(k.id()).is_some()))).collect::<serde_json::Map<_, _>>().into()
    }

    /// Valida e dispara a geração numa thread. Erros de validação voltam já; o resto vai por `emit`.
    pub fn start(&self, req: Request, emit: Emit) -> Result<(), String> {
        let model = req.model.trim().to_string();
        if req.prompt.trim().is_empty() {
            return Err("Describe the wallpaper you want.".into());
        }
        if model.is_empty() {
            return Err("Choose a model (use \"Refresh models\" to list the ones your account can use).".into());
        }
        let api_key = keys::load(req.kind.id()).ok_or_else(|| format!("Save your {} API key first.", req.kind.title()))?;
        if self.running.swap(true, Ordering::SeqCst) {
            return Err("A generation is already running.".into());
        }
        self.cancel.store(false, Ordering::SeqCst);

        let cfg = Config { kind: req.kind, model, api_key, vision: req.vision, base_url: None };
        self.spawn(cfg, req.prompt, req.limits, emit)
    }

    /// Rascunhos guardados em disco (JSON para o Studio).
    pub fn drafts_json(&self) -> Vec<Value> {
        list_drafts(&self.drafts_dir())
    }

    /// Remove um rascunho pelo nome do arquivo. `false` se o nome não for de um rascunho ou não existir.
    pub fn remove_draft(&self, name: &str) -> bool {
        remove_draft(&self.drafts_dir(), name)
    }

    fn wallpapers_dir(&self) -> std::path::PathBuf {
        self.wallpapers_dir.clone().unwrap_or_else(crate::mcp::tools::wallpapers_dir)
    }

    fn drafts_dir(&self) -> std::path::PathBuf {
        self.drafts_dir.clone().unwrap_or_else(|| {
            crate::config::EngineConfig::config_path().parent().unwrap_or(std::path::Path::new(".")).join("ai_drafts")
        })
    }

    /// Dispara a geração numa thread (o hub já deve estar marcado como ocupado).
    fn spawn(&self, cfg: Config, prompt: String, limits: Limits, emit: Emit) -> Result<(), String> {
        let (hub, cancel) = (self.clone(), self.cancel.clone());
        let spawned = std::thread::Builder::new().name("ai-agent".into()).spawn({
            let emit = emit.clone();
            move || {
                emit(json!({"type": "started"}));
                let backend = HttpBackend { cfg, cancel: cancel.clone() };
                let mut tools = McpToolbox::new();
                let mut history = hub.history.lock().unwrap_or_else(|e| e.into_inner());
                let system = system_prompt() + &budget_note(limits.max_steps);
                let mut outcome = agent::run(&mut history, &prompt, &system, &backend, &mut tools, &cancel, limits, &mut |e| emit(event_json(e)));
                drop(history);
                let (end, message) = end_json(&outcome.end);
                // Terminou, bateu o limite ou o usuário cancelou com um resultado que renderizou mas sem salvar: guarda como
                // wallpaper, para não ficar só com um rascunho. Erro do provedor continua virando rascunho.
                if outcome.saved.is_empty()
                    && matches!(outcome.end, End::Completed | End::StepLimit | End::TokenLimit | End::Cancelled)
                    && let Some(d) = tools.draft()
                {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or_default();
                    if let Some(path) = crate::mcp::tools::save_wallpaper_in(&autosave_args(&d, stamp), &hub.wallpapers_dir()).ok().and_then(|r| r.saved) {
                        tools.draft = None;
                        outcome.saved.push(path.clone());
                        emit(json!({"type": "saved", "path": path.to_string_lossy(), "title": path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), "auto": true}));
                    }
                }
                // O que funcionou e não foi salvo não se perde: vai para o disco e para o Studio.
                let draft = tools.draft().map(|d| {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or_default();
                    let path = save_draft(&hub.drafts_dir(), &d, stamp).map(|p| p.to_string_lossy().to_string()).ok();
                    json!({"kind": d.kind, "language": d.language, "code": d.code, "path": path})
                });
                emit(json!({
                    "type": "done", "end": end, "message": message,
                    "saved": outcome.saved.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                    "usage": {"input": outcome.usage.input, "output": outcome.usage.output, "cached": outcome.usage.cached},
                    "steps": outcome.steps,
                    "limits": {"max_steps": limits.max_steps, "max_total_tokens": limits.max_total_tokens},
                    "draft": draft,
                }));
                hub.running.store(false, Ordering::SeqCst);
            }
        });
        if let Err(e) = spawned {
            self.running.store(false, Ordering::SeqCst);
            return Err(format!("could not start the generation thread: {e}"));
        }
        Ok(())
    }

    /// Busca os modelos que a conta do usuário pode usar (numa thread; resposta por `emit`).
    pub fn list_models(kind: Kind, emit: Emit) {
        std::thread::spawn(move || {
            let Some(key) = keys::load(kind.id()) else {
                emit(json!({"type": "models", "kind": kind.id(), "error": format!("Save your {} API key first.", kind.title())}));
                return;
            };
            let (url, headers) = provider::models_request(kind, &key);
            let msg = match http::get_json(&url, &headers, &AtomicBool::new(false)) {
                Ok(body) => match provider::api_error(&body) {
                    Some(e) => json!({"type": "models", "kind": kind.id(), "error": e}),
                    None => json!({"type": "models", "kind": kind.id(), "models": provider::parse_models(kind, &body).into_iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>()}),
                },
                Err(e) => json!({"type": "models", "kind": kind.id(), "error": e}),
            };
            emit(msg);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_carries_the_guides_and_the_rules() {
        let p = system_prompt();
        assert!(p.contains("wallpaper artist") && p.contains("Contrato de shader") && p.contains("Guia de wallpapers web"));
        assert!(p.contains("you cannot apply") || p.contains("You cannot apply"));
        assert!(p.len() < 40_000, "o prompt de sistema cabe folgado no contexto: {}", p.len());
    }

    #[test]
    fn the_model_gets_the_creative_tools_but_cannot_apply_or_list() {
        let t = McpToolbox::new();
        let names: Vec<String> = t.defs().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["psl_compile", "psl_render", "render_still", "render_page_still", "web_preview", "save_wallpaper"]);
        let mut t = t;
        let r = t.call("apply_wallpaper", &json!({"wallpaper": "matrix"}));
        assert!(r.is_error, "apply_wallpaper não está disponível ao modelo");
        assert!(t.call("psl_compile", &Value::Null).is_error, "argumentos nulos viram erro de ferramenta, não pânico");
    }

    #[test]
    fn toolbox_returns_images_and_text_from_a_real_render() {
        let mut t = McpToolbox::new();
        let r = t.call("psl_render", &json!({"code": "void mainImage(out vec4 O, in vec2 U){ O = vec4(U / iResolution.xy, 0.4, 1.); }", "times": [1], "width": 48, "height": 27}));
        assert!(!r.is_error);
        assert_eq!(r.parts.iter().filter(|p| matches!(p, Part::Image(_))).count(), 1);
        assert!(matches!(&r.parts[0], Part::Text(t) if t.starts_with("COMPILED OK")));
        assert!(r.saved.is_none());
    }

    #[test]
    fn start_validates_before_spawning_anything() {
        let hub = Hub::default();
        let emit: Emit = Arc::new(|_| {});
        let req = |model: &str, prompt: &str| Request { kind: Kind::Claude, model: model.into(), vision: true, prompt: prompt.into(), limits: Limits::default() };
        assert!(hub.start(req("claude-opus-5-5", "  "), emit.clone()).unwrap_err().contains("Describe"));
        assert!(hub.start(req("", "um oceano"), emit.clone()).unwrap_err().contains("Choose a model"));
        // sem chave guardada (o teste usa um provedor que a máquina de teste não deve ter configurado)
        if keys::load("claude").is_none() {
            assert!(hub.start(req("claude-opus-5-5", "um oceano"), emit).unwrap_err().contains("API key"));
        }
        assert!(!hub.is_running(), "validação que falha não deixa o hub 'ocupado'");
    }

    /// Servidor HTTP mínimo que responde, em ordem, como a API do Claude, e guarda os corpos recebidos.
    fn claude_mock(replies: Vec<Value>) -> (String, Arc<Mutex<Vec<Value>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let log = bodies.clone();
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut got = Vec::new();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    let Some(h) = text.find("\r\n\r\n") else {
                        if n == 0 { break } else { continue }
                    };
                    let len = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if got.len() >= h + 4 + len || n == 0 {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&got).to_string();
                let body = &text[text.find("\r\n\r\n").map_or(0, |h| h + 4)..];
                log.lock().unwrap().push(serde_json::from_str(body).unwrap_or(Value::Null));
                let out = reply.to_string();
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}", out.len()).as_bytes());
            }
        });
        (base, bodies)
    }

    #[test]
    fn the_whole_loop_runs_over_real_http_with_real_tools() {
        let glsl = "void mainImage(out vec4 O, in vec2 U){ vec2 uv = U / iResolution.xy; O = vec4(uv, 0.5 + 0.5 * sin(iTime), 1.); }";
        let (base, bodies) = claude_mock(vec![
            json!({"content": [
                {"type": "thinking", "thinking": "", "signature": "sg-1"},
                {"type": "text", "text": "Vou testar um degradê."},
                {"type": "tool_use", "id": "tu_1", "name": "psl_render", "input": {"code": glsl, "times": [1, 4], "width": 64, "height": 36}}
            ], "stop_reason": "tool_use", "usage": {"input_tokens": 1000, "output_tokens": 200}}),
            json!({"content": [{"type": "text", "text": "Ficou bom."}], "stop_reason": "end_turn", "usage": {"input_tokens": 1400, "output_tokens": 30, "cache_read_input_tokens": 900}}),
        ]);
        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink = events.clone();
        let emit: Emit = Arc::new(move |e| sink.lock().unwrap().push(e));

        let wall = temp_dir("whole_w");
        let hub = Hub { wallpapers_dir: Some(wall.clone()), drafts_dir: Some(temp_dir("whole_d")), ..Hub::default() };
        hub.running.store(true, Ordering::SeqCst);
        let cfg = Config { kind: Kind::Claude, model: "claude-opus-5-5".into(), api_key: "SEGREDO".into(), vision: true, base_url: Some(base) };
        hub.spawn(cfg, "um degradê animado".into(), Limits::default(), emit).expect("thread");

        let started = std::time::Instant::now();
        while !events.lock().unwrap().iter().any(|e| e["type"] == "done") {
            assert!(started.elapsed().as_secs() < 90, "a geração não terminou: {:?}", events.lock().unwrap().iter().map(|e| e["type"].clone()).collect::<Vec<_>>());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let events = events.lock().unwrap().clone();
        let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        // o modelo terminou sem salvar: o app guarda a melhor versão que renderizou (evento `saved` automático)
        assert_eq!(types, ["started", "thinking", "said", "tool", "tool_done", "thinking", "said", "saved", "done"]);
        assert_eq!(events[7]["auto"], true);
        assert!(std::path::Path::new(events[7]["path"].as_str().unwrap()).starts_with(&wall));
        let done = events.last().unwrap();
        assert!(done["draft"].is_null(), "salvo = sem rascunho pendente");
        let _ = std::fs::remove_dir_all(&wall);
        assert_eq!((done["end"].as_str(), done["steps"].as_u64()), (Some("completed"), Some(2)));
        assert_eq!(done["usage"], json!({"input": 2400, "output": 230, "cached": 900}));
        let tool_done = &events[4];
        assert_eq!(tool_done["ok"], true);
        assert_eq!(tool_done["images"].as_array().unwrap().len(), 2, "duas imagens renderizadas de verdade");
        assert!(tool_done["images"][0].as_str().unwrap().starts_with("data:image/png;base64,iVBOR"));
        assert!(!hub.is_running(), "o hub volta a ficar livre");

        // o que o provedor recebeu
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["model"], "claude-opus-5-5");
        assert!(bodies[0]["system"].as_str().unwrap().contains("wallpaper artist"));
        assert_eq!(bodies[0]["tools"].as_array().unwrap().len(), 6);
        assert_eq!(bodies[0]["messages"][0]["content"][0]["text"], "um degradê animado");
        // 2ª chamada: o raciocínio do 1º turno volta intacto e o resultado traz a imagem
        assert_eq!(bodies[1]["messages"][1]["content"][0]["signature"], "sg-1");
        let result = &bodies[1]["messages"][2]["content"][0];
        assert_eq!((result["type"].as_str(), result["tool_use_id"].as_str()), (Some("tool_result"), Some("tu_1")));
        assert!(result["content"][0]["text"].as_str().unwrap().starts_with("COMPILED OK"));
        assert_eq!(result["content"][1]["type"], "image");
        assert!(!bodies[1].to_string().contains("SEGREDO"), "a chave não vai no corpo");
    }

    #[test]
    fn events_are_json_with_inlined_images() {
        let e = event_json(Event::ToolDone { name: "psl_render".into(), ok: true, text: "COMPILED OK".into(), images: vec![vec![1, 2, 3]] });
        assert_eq!(e["images"][0], "data:image/png;base64,AQID");
        let s = event_json(Event::Saved(std::path::PathBuf::from(r"C:\w\oceano_noturno.hlsl")));
        assert_eq!(s["title"], "oceano_noturno");
        assert_eq!(end_json(&End::Completed).0, "completed");
        assert!(end_json(&End::Failed("boom".into())).1.contains("boom"));
    }

    const OK_GLSL: &str = "void mainImage(out vec4 O, in vec2 U){ vec2 uv = U / iResolution.xy; O = vec4(uv, 0.5 + 0.5 * sin(iTime), 1.); }";

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("inae_ai_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Roda uma geração contra o servidor simulado e devolve o evento `done` e todos os eventos.
    fn run_mock(replies: Vec<Value>, limits: Limits, drafts: &std::path::Path) -> (Value, Vec<Value>) {
        let (base, _) = claude_mock(replies);
        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink = events.clone();
        let emit: Emit = Arc::new(move |e| sink.lock().unwrap().push(e));
        let wall = drafts.join("wallpapers");
        let hub = Hub { drafts_dir: Some(drafts.to_path_buf()), wallpapers_dir: Some(wall), ..Hub::default() };
        hub.running.store(true, Ordering::SeqCst);
        let cfg = Config { kind: Kind::Claude, model: "claude-opus-5-5".into(), api_key: "SEGREDO".into(), vision: true, base_url: Some(base) };
        hub.spawn(cfg, "um degradê".into(), limits, emit).expect("thread");
        let started = std::time::Instant::now();
        while !events.lock().unwrap().iter().any(|e| e["type"] == "done") {
            assert!(started.elapsed().as_secs() < 90, "a geração não terminou");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let all = events.lock().unwrap().clone();
        (all.last().unwrap().clone(), all)
    }

    fn render_turn(code: &str) -> Value {
        json!({"content": [{"type": "tool_use", "id": "tu_1", "name": "psl_render", "input": {"code": code, "times": [1], "width": 48, "height": 27}}],
               "stop_reason": "tool_use", "usage": {"input_tokens": 500, "output_tokens": 100}})
    }

    #[test]
    fn provider_error_mid_run_keeps_the_work_as_a_draft() {
        let dir = temp_dir("fail");
        let (done, _) = run_mock(
            vec![render_turn(OK_GLSL), json!({"type": "error", "error": {"type": "invalid_request_error", "message": "modelo indisponível"}})],
            Limits::default(),
            &dir,
        );
        assert_eq!(done["end"], "failed");
        // o código que renderizou OK não se perde: volta no evento e fica num arquivo
        assert_eq!(done["draft"]["code"], OK_GLSL);
        assert_eq!((done["draft"]["kind"].as_str(), done["draft"]["language"].as_str()), (Some("shader"), Some("glsl")));
        let path = done["draft"]["path"].as_str().expect("caminho do rascunho");
        assert_eq!(std::fs::read_to_string(path).unwrap(), OK_GLSL);
        assert!(path.ends_with(".glsl"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_limits_stop_the_run_and_report_the_draft() {
        let dir = temp_dir("limit");
        // limite de rodadas: 1 passo (a renderização) e a conversa para
        let (done, _) = run_mock(vec![render_turn(OK_GLSL)], Limits { max_steps: 1, ..Limits::default() }, &dir);
        assert_eq!(done["end"], "step_limit");
        assert_eq!(done["limits"]["max_steps"], 1);
        assert!(done["message"].as_str().unwrap().contains("limit"));
        // bateu o limite com um shader que renderizou: ele é salvo como wallpaper, não fica só como rascunho
        assert_eq!(done["saved"].as_array().unwrap().len(), 1);
        assert!(done["draft"].is_null());
        assert!(std::path::Path::new(done["saved"][0].as_str().unwrap()).exists());
        // limite de tokens: 500 lidos + 100 escritos já passam de 400
        let (done, _) = run_mock(vec![render_turn(OK_GLSL)], Limits { max_total_tokens: 400, ..Limits::default() }, &dir);
        assert_eq!(done["end"], "token_limit");
        assert_eq!(done["limits"]["max_total_tokens"], 400);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_wallpaper_clears_the_draft_and_errors_do_not_replace_it() {
        let mut t = McpToolbox::new();
        assert!(t.draft().is_none());
        t.track("psl_render", &json!({"code": "A", "language": "hlsl"}), false);
        assert_eq!(t.draft().unwrap().code, "A");
        t.track("psl_render", &json!({"code": "B quebrado"}), true);
        assert_eq!(t.draft().unwrap().code, "A", "erro de compilação não apaga o último que funcionou");
        t.track("web_preview", &json!({"html": "<html></html>"}), false);
        assert_eq!((t.draft().unwrap().kind, t.draft().unwrap().language), ("web", "html"));
        t.track("save_wallpaper", &json!({"kind": "web"}), false);
        assert!(t.draft().is_none(), "salvo = nada pendente");
    }

    #[test]
    fn drafts_are_written_and_old_ones_pruned() {
        let dir = temp_dir("prune");
        let d = Draft { kind: "shader", language: "hlsl", code: "x".into() };
        for i in 0..(MAX_DRAFTS as u64 + 5) {
            save_draft(&dir, &d, i).unwrap();
        }
        let n = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(n, MAX_DRAFTS);
        assert!(!dir.join("rascunho-000000000000.hlsl").exists(), "o mais antigo saiu");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drafts_are_listed_newest_first_and_removed_only_by_plain_name() {
        let dir = temp_dir("list");
        save_draft(&dir, &Draft { kind: "shader", language: "hlsl", code: "a".into() }, 5).unwrap();
        save_draft(&dir, &Draft { kind: "web", language: "html", code: "<p>".into() }, 9).unwrap();
        std::fs::write(dir.join("outro.txt"), "x").unwrap();
        let list = list_drafts(&dir);
        assert_eq!(list.len(), 2, "só rascunhos");
        assert_eq!((list[0]["kind"].as_str(), list[0]["stamp"].as_u64()), (Some("web"), Some(9)));
        assert_eq!(list[1]["kind"], "shader");
        for bad in ["", "..", "../x", "rascunho-1.hlsl/../../x", "C:\\x", "outro.txt"] {
            assert!(!remove_draft(&dir, bad), "{bad}");
        }
        assert!(dir.join("outro.txt").exists());
        assert!(remove_draft(&dir, "rascunho-000000000009.html"));
        assert_eq!(list_drafts(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_pages_are_fixed_https_addresses_of_the_providers() {
        for (kind, host) in [(Kind::Claude, "console.anthropic.com"), (Kind::Gemini, "aistudio.google.com"), (Kind::OpenRouter, "openrouter.ai")] {
            let url = key_page(kind);
            assert!(url.starts_with(&format!("https://{host}/")), "{url}");
        }
    }

    #[test]
    fn the_model_is_told_its_budget_and_unsaved_work_is_saved_for_it() {
        assert!(budget_note(2).contains("at most 2 rounds") && budget_note(2).contains("first reply"));
        assert!(budget_note(24).contains("at most 24 rounds") && budget_note(24).contains("second-to-last"));
        let shader = Draft { kind: "shader", language: "glsl", code: "void mainImage(out vec4 O,in vec2 U){O=vec4(1);}".into() };
        let a = autosave_args(&shader, 7);
        assert_eq!((a["kind"].as_str(), a["language"].as_str(), a["name"].as_str()), (Some("shader"), Some("glsl"), Some("ia-7")));
        let page = Draft { kind: "web", language: "html", code: "<html></html>".into() };
        assert!(autosave_args(&page, 7).get("language").is_none(), "páginas não têm linguagem de shader");
    }
}
