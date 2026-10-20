//! Servidor MCP (Model Context Protocol) sobre stdio: deixa um cliente de IA (Claude Code,
//! Claude Desktop, Gemini CLI, Cursor...) criar wallpapers usando o PSL, com a conta do próprio
//! usuário. Rodar: `IsNotAEngine.exe --mcp`. Nenhuma chave de API passa por aqui.
//!
//! Transporte: uma mensagem JSON-RPC por linha em stdin/stdout. O stdout é só do protocolo;
//! logs vão para o stderr.

pub(crate) mod psl;
pub(crate) mod tools;
mod web;

use std::io::{BufRead, Write};

use serde_json::{Value, json};

const LATEST: &str = "2025-06-18";
const SUPPORTED: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Create wallpapers for the IsNotAEngine desktop engine. \
Call get_guide (topic \"shader\" or \"web\") first, then iterate: write, psl_render (shaders) or web_preview (HTML), \
read the images and hints, fix, repeat. Save with save_wallpaper (or render_still for a high-resolution static image) \
and apply with apply_wallpaper. Shaders are rendered on the user's GPU: keep loops bounded.";

/// Estado do servidor entre chamadas (a sessão de GPU é criada na primeira renderização).
#[derive(Default)]
pub struct Server {
    tools: tools::State,
}

fn reply(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error_reply(id: &Value, code: i32, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

impl Server {
    /// Processa uma linha. `None` para notificações e para respostas do cliente (não há o que responder).
    pub fn handle(&mut self, line: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Some(error_reply(&Value::Null, -32700, "Parse error")),
        };
        let Some(obj) = msg.as_object() else {
            return Some(error_reply(&Value::Null, -32600, "Invalid Request: expected a JSON object"));
        };
        let method = obj.get("method").and_then(Value::as_str)?;
        let id = obj.get("id")?; // sem id = notificação
        let params = obj.get("params").unwrap_or(&Value::Null);

        Some(match method {
            "initialize" => reply(id, Self::initialize(params)),
            "ping" => reply(id, json!({})),
            "tools/list" => reply(id, json!({"tools": tools::definitions()})),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return Some(error_reply(id, -32602, "Invalid params: \"name\" is required"));
                };
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                reply(id, tools::call(&mut self.tools, name, &args).to_json())
            }
            other => error_reply(id, -32601, &format!("Method not found: {other}")),
        })
    }

    fn initialize(params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(LATEST);
        let version = if SUPPORTED.contains(&requested) { requested } else { LATEST };
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "isnotaengine", "title": "IsNotAEngine", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })
    }
}

/// Laço principal em stdin/stdout. Devolve o código de saída do processo.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut server = Server::default();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(out) = server.handle(&line)
            && writeln!(stdout, "{out}").and_then(|_| stdout.flush()).is_err()
        {
            break; // o cliente fechou o pipe
        }
    }
    0
}

/// Texto de configuração para o usuário colar no cliente de IA.
pub fn config_help() -> String {
    let exe = std::env::current_exe().map_or_else(|_| "IsNotAEngine.exe".to_string(), |p| p.display().to_string());
    let json_path = exe.replace('\\', "\\\\");
    format!(
        "IsNotAEngine MCP server\n\n\
         Claude Code (run once):\n  claude mcp add isnotaengine -- \"{exe}\" --mcp\n\n\
         Claude Desktop / Cursor / other clients (add to the \"mcpServers\" section of their JSON config):\n  \
         \"isnotaengine\": {{ \"command\": \"{json_path}\", \"args\": [\"--mcp\"] }}\n\n\
         Gemini CLI (~/.gemini/settings.json, \"mcpServers\" section): the same JSON entry as above.\n\n\
         The AI uses the user's own subscription; no API key is needed. Keep the IsNotAEngine app running\n\
         to apply wallpapers; compiling, rendering and saving work without it.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(server: &mut Server, msg: Value) -> Option<Value> {
        server.handle(&msg.to_string()).map(|s| serde_json::from_str(&s).expect("resposta é JSON"))
    }

    #[test]
    fn handshake_negotiates_the_version_and_lists_capabilities() {
        let mut s = Server::default();
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "isnotaengine");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert!(r["result"]["instructions"].as_str().unwrap().contains("get_guide"));
        // versão desconhecida: responde com a mais recente que suportamos
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {"protocolVersion": "1999-01-01"}})).unwrap();
        assert_eq!(r["id"], "a");
        assert_eq!(r["result"]["protocolVersion"], LATEST);
    }

    #[test]
    fn notifications_and_client_responses_get_no_reply() {
        let mut s = Server::default();
        assert!(call(&mut s, json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        assert!(call(&mut s, json!({"jsonrpc": "2.0", "id": 7, "result": {}})).is_none());
    }

    #[test]
    fn errors_follow_json_rpc() {
        let mut s = Server::default();
        let r = s.handle("isso nao e json").unwrap();
        assert_eq!(serde_json::from_str::<Value>(&r).unwrap()["error"]["code"], -32700);
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "resources/frobnicate"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {}})).unwrap();
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(call(&mut s, json!({"jsonrpc": "2.0", "id": 4, "method": "ping"})).unwrap()["result"], json!({}));
        assert_eq!(serde_json::from_str::<Value>(&s.handle("[1,2]").unwrap()).unwrap()["error"]["code"], -32600);
    }

    #[test]
    fn tools_list_and_call_roundtrip() {
        let mut s = Server::default();
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 9);
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_guide", "arguments": {"topic": "workflow"}}})).unwrap();
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["content"][0]["type"], "text");
        // falha de ferramenta é resultado com isError, não erro de protocolo
        let r = call(&mut s, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "psl_compile", "arguments": {"code": "lixo"}}})).unwrap();
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn config_help_mentions_the_command_and_both_formats() {
        let t = config_help();
        assert!(t.contains("claude mcp add isnotaengine") && t.contains("--mcp") && t.contains("mcpServers"));
    }
}
