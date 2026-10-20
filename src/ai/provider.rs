//! Adaptadores dos provedores de IA (Claude, Gemini e OpenRouter): montam a requisição e leem a
//! resposta num formato neutro. Tudo aqui é função pura sobre JSON (testável sem rede); o HTTP
//! fica em `http.rs`.
//!
//! A resposta bruta do modelo (`Turn.raw`) é guardada e reenviada sem mudanças no histórico:
//! blocos de raciocínio do Claude e assinaturas de pensamento do Gemini precisam voltar intactos.

use serde_json::{Map, Value, json};

use crate::media::base64;

pub const MAX_OUTPUT_TOKENS: u32 = 32_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Claude,
    Gemini,
    OpenRouter,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Claude, Kind::Gemini, Kind::OpenRouter];

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn id(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Gemini => "gemini",
            Kind::OpenRouter => "openrouter",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Claude => "Claude (Anthropic)",
            Kind::Gemini => "Gemini (Google)",
            Kind::OpenRouter => "OpenRouter",
        }
    }

    /// Endereço oficial (esquema + host) da API.
    pub fn origin(self) -> &'static str {
        match self {
            Kind::Claude => "https://api.anthropic.com",
            Kind::Gemini => "https://generativelanguage.googleapis.com",
            Kind::OpenRouter => "https://openrouter.ai",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub kind: Kind,
    pub model: String,
    pub api_key: String,
    /// Se o modelo lê imagens. Sem visão, os quadros renderizados não são enviados (só as métricas).
    pub vision: bool,
    /// Troca o endereço do provedor (`http://127.0.0.1:porta`): testes e proxies. `None` = oficial.
    pub base_url: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Part {
    Text(String),
    /// PNG.
    Image(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Clone, Debug)]
pub struct ToolOut {
    pub call_id: String,
    pub name: String,
    pub parts: Vec<Part>,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
pub enum Msg {
    User(Vec<Part>),
    /// `raw` é o conteúdo do turno como o provedor devolveu (reenviado no histórico).
    Assistant { raw: Value },
    ToolResults(Vec<ToolOut>),
}

#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema do objeto de argumentos.
    pub schema: Value,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Finish {
    Done,
    ToolUse,
    MaxTokens,
    Refusal,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub text: String,
    pub calls: Vec<Call>,
    pub raw: Value,
    pub usage: Usage,
    pub finish: Finish,
}

pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

fn png_b64(png: &[u8]) -> String {
    base64(png)
}

/// Texto de uma lista de partes (imagens viram um marcador), para provedores só-texto.
fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.clone(),
            Part::Image(_) => "[image]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn images_of(outs: &[ToolOut]) -> Vec<&Vec<u8>> {
    outs.iter()
        .flat_map(|o| o.parts.iter())
        .filter_map(|p| if let Part::Image(i) = p { Some(i) } else { None })
        .collect()
}

// ---------------------------------------------------------------------------- requisição

pub fn build_request(cfg: &Config, system: &str, history: &[Msg], tools: &[ToolDef]) -> Request {
    let mut req = match cfg.kind {
        Kind::Claude => claude_request(cfg, system, history, tools),
        Kind::Gemini => gemini_request(cfg, system, history, tools),
        Kind::OpenRouter => openrouter_request(cfg, system, history, tools),
    };
    if let Some(base) = &cfg.base_url {
        req.url = req.url.replacen(cfg.kind.origin(), base.trim_end_matches('/'), 1);
    }
    req
}

fn claude_request(cfg: &Config, system: &str, history: &[Msg], tools: &[ToolDef]) -> Request {
    let image = |png: &[u8]| json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png_b64(png)}});
    let part = |p: &Part| match p {
        Part::Text(t) => json!({"type": "text", "text": t}),
        Part::Image(i) => image(i),
    };
    let messages: Vec<Value> = history
        .iter()
        .map(|m| match m {
            Msg::User(parts) => json!({"role": "user", "content": parts.iter().filter(|p| cfg.vision || matches!(p, Part::Text(_))).map(part).collect::<Vec<_>>()}),
            Msg::Assistant { raw } => json!({"role": "assistant", "content": raw}),
            Msg::ToolResults(outs) => json!({"role": "user", "content": outs.iter().map(|o| {
                let content: Vec<Value> = o.parts.iter().filter(|p| cfg.vision || matches!(p, Part::Text(_))).map(part).collect();
                json!({"type": "tool_result", "tool_use_id": o.call_id, "content": content, "is_error": o.is_error})
            }).collect::<Vec<_>>()}),
        })
        .collect();
    let tools: Vec<Value> = tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    Request {
        url: "https://api.anthropic.com/v1/messages".into(),
        headers: vec![
            ("x-api-key".into(), cfg.api_key.clone()),
            ("anthropic-version".into(), "2023-06-01".into()),
            ("content-type".into(), "application/json".into()),
        ],
        body: json!({
            "model": cfg.model,
            "max_tokens": MAX_OUTPUT_TOKENS,
            "system": system,
            // Cache automático do prefixo (sistema + ferramentas + conversa até aqui).
            "cache_control": {"type": "ephemeral"},
            "tools": tools,
            "messages": messages,
        }),
    }
}

/// O Gemini aceita só um subconjunto do JSON Schema: sem `additionalProperties`, `required` vazio
/// nem `default`.
fn gemini_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if matches!(k.as_str(), "additionalProperties" | "default" | "$schema") {
                    continue;
                }
                if k == "required" && v.as_array().is_some_and(Vec::is_empty) {
                    continue;
                }
                out.insert(k.clone(), gemini_schema(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(gemini_schema).collect()),
        other => other.clone(),
    }
}

fn gemini_request(cfg: &Config, system: &str, history: &[Msg], tools: &[ToolDef]) -> Request {
    let inline = |png: &[u8]| json!({"inlineData": {"mimeType": "image/png", "data": png_b64(png)}});
    let contents: Vec<Value> = history
        .iter()
        .map(|m| match m {
            Msg::User(parts) => json!({"role": "user", "parts": parts.iter().filter_map(|p| match p {
                Part::Text(t) => Some(json!({"text": t})),
                Part::Image(i) if cfg.vision => Some(inline(i)),
                Part::Image(_) => None,
            }).collect::<Vec<_>>()}),
            Msg::Assistant { raw } => raw.clone(),
            Msg::ToolResults(outs) => {
                let mut parts: Vec<Value> = outs
                    .iter()
                    .map(|o| {
                        let key = if o.is_error { "error" } else { "output" };
                        json!({"functionResponse": {"name": o.name, "response": {key: text_of(&o.parts)}}})
                    })
                    .collect();
                if cfg.vision {
                    parts.extend(images_of(outs).into_iter().map(|i| inline(i)));
                }
                json!({"role": "user", "parts": parts})
            }
        })
        .collect();
    let declarations: Vec<Value> = tools
        .iter()
        .map(|t| {
            let mut d = json!({"name": t.name, "description": t.description});
            let schema = gemini_schema(&t.schema);
            let has_props = schema.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty());
            if has_props {
                d["parameters"] = schema;
            }
            d
        })
        .collect();
    Request {
        url: format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", cfg.model),
        headers: vec![("x-goog-api-key".into(), cfg.api_key.clone()), ("content-type".into(), "application/json".into())],
        body: json!({
            "system_instruction": {"parts": [{"text": system}]},
            "contents": contents,
            "tools": [{"functionDeclarations": declarations}],
            "generationConfig": {"maxOutputTokens": MAX_OUTPUT_TOKENS},
        }),
    }
}

fn openrouter_request(cfg: &Config, system: &str, history: &[Msg], tools: &[ToolDef]) -> Request {
    let image_url = |png: &[u8]| json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", png_b64(png))}});
    let mut messages = vec![json!({"role": "system", "content": system})];
    for m in history {
        match m {
            Msg::User(parts) => {
                let content: Vec<Value> = parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(json!({"type": "text", "text": t})),
                        Part::Image(i) if cfg.vision => Some(image_url(i)),
                        Part::Image(_) => None,
                    })
                    .collect();
                messages.push(json!({"role": "user", "content": content}));
            }
            Msg::Assistant { raw } => messages.push(raw.clone()),
            Msg::ToolResults(outs) => {
                for o in outs {
                    messages.push(json!({"role": "tool", "tool_call_id": o.call_id, "content": text_of(&o.parts)}));
                }
                // Resultados de ferramenta não carregam imagens neste formato: seguem numa mensagem do usuário.
                let images = images_of(outs);
                if cfg.vision && !images.is_empty() {
                    let mut content = vec![json!({"type": "text", "text": "Rendered frames from the tool call(s) above, in order:"})];
                    content.extend(images.into_iter().map(|i| image_url(i)));
                    messages.push(json!({"role": "user", "content": content}));
                }
            }
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
        .collect();
    Request {
        url: "https://openrouter.ai/api/v1/chat/completions".into(),
        headers: vec![
            ("authorization".into(), format!("Bearer {}", cfg.api_key)),
            ("content-type".into(), "application/json".into()),
            ("x-title".into(), "IsNotAEngine".into()),
        ],
        body: json!({"model": cfg.model, "max_tokens": MAX_OUTPUT_TOKENS, "tools": tools, "messages": messages}),
    }
}

// ---------------------------------------------------------------------------- resposta

/// Mensagem de erro da API (`{"error": {"message": ...}}` nos três provedores), se houver.
pub fn api_error(body: &Value) -> Option<String> {
    let e = body.get("error")?;
    e.get("message").and_then(Value::as_str).map(str::to_string).or_else(|| e.as_str().map(str::to_string))
}

pub fn parse_response(kind: Kind, body: &Value) -> Result<Turn, String> {
    if let Some(msg) = api_error(body) {
        return Err(msg);
    }
    match kind {
        Kind::Claude => parse_claude(body),
        Kind::Gemini => parse_gemini(body),
        Kind::OpenRouter => parse_openrouter(body),
    }
}

fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn parse_claude(body: &Value) -> Result<Turn, String> {
    let blocks = body.get("content").and_then(Value::as_array).ok_or("the reply has no content")?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("tool_use") => calls.push(Call {
                id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: b.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
            _ => {} // thinking e afins: ficam só no `raw`
        }
    }
    let finish = match body.get("stop_reason").and_then(Value::as_str) {
        Some("tool_use") => Finish::ToolUse,
        Some("max_tokens") => Finish::MaxTokens,
        Some("refusal") => Finish::Refusal,
        _ if !calls.is_empty() => Finish::ToolUse,
        _ => Finish::Done,
    };
    let u = body.get("usage").cloned().unwrap_or(Value::Null);
    Ok(Turn {
        text,
        calls,
        raw: Value::Array(blocks.clone()),
        usage: Usage { input: u64_at(&u, "input_tokens"), output: u64_at(&u, "output_tokens"), cached: u64_at(&u, "cache_read_input_tokens") },
        finish,
    })
}

fn parse_gemini(body: &Value) -> Result<Turn, String> {
    if let Some(reason) = body.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(format!("Gemini blocked the request ({reason})"));
    }
    let cand = body.pointer("/candidates/0").ok_or("Gemini returned no candidates")?;
    let content = cand.get("content").cloned().unwrap_or_else(|| json!({"role": "model", "parts": []}));
    let parts = content.get("parts").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if let Some(fc) = p.get("functionCall") {
            calls.push(Call {
                // O Gemini não numera as chamadas: o índice basta, o `name` é o que ele usa para casar.
                id: format!("call_{i}"),
                name: fc.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: fc.get("args").cloned().unwrap_or_else(|| json!({})),
            });
        } else if p.get("thought").and_then(Value::as_bool) != Some(true)
            && let Some(t) = p.get("text").and_then(Value::as_str)
        {
            text.push_str(t);
        }
    }
    let finish = match cand.get("finishReason").and_then(Value::as_str) {
        Some("MAX_TOKENS") => Finish::MaxTokens,
        Some("SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "SPII") => Finish::Refusal,
        _ if !calls.is_empty() => Finish::ToolUse,
        _ => Finish::Done,
    };
    let u = body.get("usageMetadata").cloned().unwrap_or(Value::Null);
    Ok(Turn {
        text,
        calls,
        raw: json!({"role": "model", "parts": parts}),
        usage: Usage { input: u64_at(&u, "promptTokenCount"), output: u64_at(&u, "candidatesTokenCount") + u64_at(&u, "thoughtsTokenCount"), cached: u64_at(&u, "cachedContentTokenCount") },
        finish,
    })
}

fn parse_openrouter(body: &Value) -> Result<Turn, String> {
    let choice = body.pointer("/choices/0").ok_or("the reply has no choices")?;
    let msg = choice.get("message").ok_or("the reply has no message")?;
    let text = msg.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let calls: Vec<Call> = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|c| Call {
                    id: c.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string(),
                    args: c
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(Value::Null),
                })
                .collect()
        })
        .unwrap_or_default();
    // O que volta ao histórico: o conteúdo e as chamadas como vieram (sem campos nulos). A API exige
    // `content` ou `tool_calls`; sem nenhum dos dois, vai um texto vazio.
    let mut raw = json!({"role": "assistant"});
    if let Some(c) = msg.get("content").filter(|c| !c.is_null()) {
        raw["content"] = c.clone();
    }
    if let Some(t) = msg.get("tool_calls").filter(|t| !t.is_null()) {
        raw["tool_calls"] = t.clone();
    }
    if raw.get("content").is_none() && raw.get("tool_calls").is_none() {
        raw["content"] = json!("");
    }
    let finish = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("length") => Finish::MaxTokens,
        Some("content_filter") => Finish::Refusal,
        _ if !calls.is_empty() => Finish::ToolUse,
        _ => Finish::Done,
    };
    let u = body.get("usage").cloned().unwrap_or(Value::Null);
    Ok(Turn {
        text,
        calls,
        raw,
        usage: Usage { input: u64_at(&u, "prompt_tokens"), output: u64_at(&u, "completion_tokens"), cached: 0 },
        finish,
    })
}

/// Lista de modelos de uma resposta de `GET models` do provedor: `(id, rótulo)`.
pub fn parse_models(kind: Kind, body: &Value) -> Vec<(String, String)> {
    let list = |key: &str| body.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<(String, String)> = match kind {
        Kind::Claude => list("data")
            .iter()
            .filter_map(|m| Some((m.get("id")?.as_str()?.to_string(), m.get("display_name").and_then(Value::as_str).unwrap_or("").to_string())))
            .collect(),
        Kind::Gemini => list("models")
            .iter()
            .filter(|m| {
                m.get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some("generateContent")))
            })
            .filter_map(|m| {
                let id = m.get("name")?.as_str()?.trim_start_matches("models/").to_string();
                Some((id, m.get("displayName").and_then(Value::as_str).unwrap_or("").to_string()))
            })
            .collect(),
        Kind::OpenRouter => list("data")
            .iter()
            .filter(|m| m.pointer("/supported_parameters").and_then(Value::as_array).is_none_or(|a| a.iter().any(|x| x.as_str() == Some("tools"))))
            .filter_map(|m| Some((m.get("id")?.as_str()?.to_string(), m.get("name").and_then(Value::as_str).unwrap_or("").to_string())))
            .collect(),
    };
    out.sort();
    out
}

/// URL e cabeçalhos para listar os modelos da conta.
pub fn models_request(kind: Kind, api_key: &str) -> (String, Vec<(String, String)>) {
    match kind {
        Kind::Claude => (
            "https://api.anthropic.com/v1/models?limit=100".into(),
            vec![("x-api-key".into(), api_key.into()), ("anthropic-version".into(), "2023-06-01".into())],
        ),
        Kind::Gemini => ("https://generativelanguage.googleapis.com/v1beta/models?pageSize=200".into(), vec![("x-goog-api-key".into(), api_key.into())]),
        Kind::OpenRouter => ("https://openrouter.ai/api/v1/models".into(), vec![("authorization".into(), format!("Bearer {api_key}"))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: Kind, vision: bool) -> Config {
        Config { kind, model: "m".into(), api_key: "SEGREDO".into(), vision, base_url: None }
    }

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "psl_render".into(),
                description: "render".into(),
                schema: json!({"type": "object", "properties": {"code": {"type": "string"}, "params": {"type": "object", "additionalProperties": {"type": "number"}}}, "required": ["code"]}),
            },
            ToolDef { name: "ping".into(), description: "no args".into(), schema: json!({"type": "object", "properties": {}, "required": []}) },
        ]
    }

    fn history() -> Vec<Msg> {
        vec![
            Msg::User(vec![Part::Text("faça um oceano".into())]),
            Msg::Assistant { raw: json!([{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "tool_use", "id": "tu_1", "name": "psl_render", "input": {"code": "x"}}]) },
            Msg::ToolResults(vec![ToolOut {
                call_id: "tu_1".into(),
                name: "psl_render".into(),
                parts: vec![Part::Text("ok".into()), Part::Image(vec![1, 2, 3])],
                is_error: false,
            }]),
        ]
    }

    #[test]
    fn base_url_replaces_only_the_origin() {
        let mut c = cfg(Kind::Claude, true);
        c.base_url = Some("http://127.0.0.1:9/".into());
        assert_eq!(build_request(&c, "S", &[], &[]).url, "http://127.0.0.1:9/v1/messages");
        c.kind = Kind::Gemini;
        assert_eq!(build_request(&c, "S", &[], &[]).url, "http://127.0.0.1:9/v1beta/models/m:generateContent");
        c.kind = Kind::OpenRouter;
        assert_eq!(build_request(&c, "S", &[], &[]).url, "http://127.0.0.1:9/api/v1/chat/completions");
        c.base_url = None;
        assert!(build_request(&c, "S", &[], &[]).url.starts_with("https://openrouter.ai/"));
    }

    #[test]
    fn kinds_roundtrip() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.id()), Some(k));
        }
        assert_eq!(Kind::parse("gpt"), None);
    }

    #[test]
    fn claude_request_echoes_raw_blocks_and_nests_images_in_tool_results() {
        let r = claude_request(&cfg(Kind::Claude, true), "SYS", &history(), &tools());
        assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
        assert!(r.headers.iter().any(|(k, v)| k == "x-api-key" && v == "SEGREDO"));
        assert!(r.headers.iter().any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
        let b = &r.body;
        assert_eq!(b["system"], "SYS");
        assert_eq!(b["cache_control"]["type"], "ephemeral");
        assert!(b.get("thinking").is_none() && b.get("temperature").is_none(), "nada de parâmetros removidos nos modelos novos");
        assert_eq!(b["tools"][0]["input_schema"]["required"][0], "code");
        // o bloco de raciocínio volta intacto, com a assinatura
        assert_eq!(b["messages"][1]["content"][0]["signature"], "sig");
        let tr = &b["messages"][2]["content"][0];
        assert_eq!(tr["type"], "tool_result");
        assert_eq!(tr["tool_use_id"], "tu_1");
        assert_eq!(tr["content"][1]["type"], "image");
        assert_eq!(tr["content"][1]["source"]["data"], "AQID");
        assert_eq!(tr["is_error"], false);
        // sem visão: a imagem não é enviada
        let blind = claude_request(&cfg(Kind::Claude, false), "SYS", &history(), &tools());
        assert_eq!(blind.body["messages"][2]["content"][0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn gemini_schema_drops_unsupported_keywords() {
        let r = gemini_request(&cfg(Kind::Gemini, true), "SYS", &history(), &tools());
        assert!(r.url.ends_with("/models/m:generateContent"));
        assert!(!r.url.contains("SEGREDO"), "a chave vai no cabeçalho, nunca na URL");
        assert!(r.headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "SEGREDO"));
        let decl = &r.body["tools"][0]["functionDeclarations"];
        let params = &decl[0]["parameters"];
        assert!(params["properties"]["params"].get("additionalProperties").is_none());
        assert_eq!(params["required"][0], "code");
        assert!(decl[1].get("parameters").is_none(), "ferramenta sem argumentos não leva `parameters`");
        // resposta de ferramenta: functionResponse primeiro, depois a imagem
        let parts = &r.body["contents"][2]["parts"];
        assert_eq!(parts[0]["functionResponse"]["name"], "psl_render");
        assert_eq!(parts[0]["functionResponse"]["response"]["output"], "ok\n[image]");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(r.body["system_instruction"]["parts"][0]["text"], "SYS");
    }

    #[test]
    fn openrouter_request_puts_images_in_a_follow_up_user_message() {
        let r = openrouter_request(&cfg(Kind::OpenRouter, true), "SYS", &history(), &tools());
        assert!(r.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer SEGREDO"));
        let m = r.body["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "tu_1");
        assert_eq!(m[4]["role"], "user");
        assert!(m[4]["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(r.body["tools"][0]["function"]["name"], "psl_render");
        // sem visão não há mensagem de imagens
        let blind = openrouter_request(&cfg(Kind::OpenRouter, false), "SYS", &history(), &tools());
        assert_eq!(blind.body["messages"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn parses_claude_replies_with_tools_thinking_and_usage() {
        let body = json!({
            "content": [{"type": "thinking", "thinking": "", "signature": "s"}, {"type": "text", "text": "Vou renderizar."}, {"type": "tool_use", "id": "tu_9", "name": "psl_render", "input": {"code": "c"}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 40, "cache_read_input_tokens": 80}
        });
        let t = parse_response(Kind::Claude, &body).unwrap();
        assert_eq!(t.text, "Vou renderizar.");
        assert_eq!((t.calls.len(), t.calls[0].id.as_str(), t.calls[0].args["code"].as_str()), (1, "tu_9", Some("c")));
        assert_eq!(t.finish, Finish::ToolUse);
        assert_eq!(t.usage, Usage { input: 100, output: 40, cached: 80 });
        assert_eq!(t.raw.as_array().unwrap().len(), 3, "o raciocínio fica no raw");
        for (reason, want) in [("end_turn", Finish::Done), ("max_tokens", Finish::MaxTokens), ("refusal", Finish::Refusal)] {
            let b = json!({"content": [{"type": "text", "text": "x"}], "stop_reason": reason});
            assert_eq!(parse_response(Kind::Claude, &b).unwrap().finish, want);
        }
    }

    #[test]
    fn parses_gemini_replies_skipping_thoughts() {
        let body = json!({
            "candidates": [{"content": {"role": "model", "parts": [
                {"text": "pensando...", "thought": true},
                {"text": "Certo."},
                {"functionCall": {"name": "psl_render", "args": {"code": "c"}}, "thoughtSignature": "abc"}
            ]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5, "thoughtsTokenCount": 7}
        });
        let t = parse_response(Kind::Gemini, &body).unwrap();
        assert_eq!(t.text, "Certo.");
        assert_eq!((t.calls[0].name.as_str(), t.calls[0].id.as_str()), ("psl_render", "call_2"));
        assert_eq!(t.finish, Finish::ToolUse);
        assert_eq!(t.usage.output, 12);
        assert_eq!(t.raw["parts"][2]["thoughtSignature"], "abc", "a assinatura volta no histórico");
        let blocked = json!({"promptFeedback": {"blockReason": "SAFETY"}});
        assert!(parse_response(Kind::Gemini, &blocked).unwrap_err().contains("SAFETY"));
        let refusal = json!({"candidates": [{"finishReason": "SAFETY"}]});
        assert_eq!(parse_response(Kind::Gemini, &refusal).unwrap().finish, Finish::Refusal);
    }

    #[test]
    fn parses_openrouter_replies_and_tolerates_bad_arguments() {
        let body = json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "psl_render", "arguments": "{\"code\":\"x\"}"}},
                {"id": "c2", "type": "function", "function": {"name": "psl_render", "arguments": "{quebrado"}}
            ]}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 9, "completion_tokens": 3}
        });
        let t = parse_response(Kind::OpenRouter, &body).unwrap();
        assert_eq!(t.calls.len(), 2);
        assert_eq!(t.calls[0].args["code"], "x");
        assert!(t.calls[1].args.is_null(), "argumentos inválidos viram null (a ferramenta responde com erro)");
        assert_eq!(t.finish, Finish::ToolUse);
        assert_eq!(t.raw["tool_calls"][0]["id"], "c1");
        assert!(t.raw.get("content").is_none(), "content nulo não é reenviado");
        assert_eq!(t.usage, Usage { input: 9, output: 3, cached: 0 });
    }

    #[test]
    fn api_errors_surface_the_providers_message() {
        for body in [
            json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}),
            json!({"error": {"code": 400, "message": "API key not valid", "status": "INVALID_ARGUMENT"}}),
            json!({"error": {"message": "No auth credentials found", "code": 401}}),
        ] {
            for k in Kind::ALL {
                assert!(parse_response(k, &body).is_err());
            }
        }
        assert_eq!(api_error(&json!({"error": "texto simples"})).as_deref(), Some("texto simples"));
    }

    #[test]
    fn model_lists_are_filtered_and_sorted() {
        let claude = json!({"data": [{"id": "claude-b", "display_name": "B"}, {"id": "claude-a", "display_name": "A"}]});
        assert_eq!(parse_models(Kind::Claude, &claude)[0], ("claude-a".to_string(), "A".to_string()));
        let gemini = json!({"models": [
            {"name": "models/gemini-x", "displayName": "X", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/embed", "displayName": "E", "supportedGenerationMethods": ["embedContent"]}
        ]});
        assert_eq!(parse_models(Kind::Gemini, &gemini), vec![("gemini-x".to_string(), "X".to_string())]);
        let or = json!({"data": [
            {"id": "a/b", "name": "AB", "supported_parameters": ["tools", "max_tokens"]},
            {"id": "c/d", "name": "CD", "supported_parameters": ["max_tokens"]}
        ]});
        assert_eq!(parse_models(Kind::OpenRouter, &or).len(), 1, "só modelos com ferramentas");
        let (url, headers) = models_request(Kind::Gemini, "K");
        assert!(!url.contains('K') || !url.contains("key="), "a chave não vai na URL");
        assert_eq!(headers[0].0, "x-goog-api-key");
    }
}
