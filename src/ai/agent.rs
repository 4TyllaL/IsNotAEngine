//! O laço do agente: o modelo escreve, chama as ferramentas (renderizar, salvar...), vê o
//! resultado e corrige, até terminar. Independente de provedor e de rede: o modelo entra por
//! `Backend` e as ferramentas por `Toolbox`, então tudo aqui é testável com roteiros.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use super::provider::{Call, Finish, Msg, Part, ToolDef, ToolOut, Turn, Usage};

pub trait Backend {
    fn complete(&self, system: &str, history: &[Msg], tools: &[ToolDef]) -> Result<Turn, String>;
}

pub struct ToolOutput {
    pub parts: Vec<Part>,
    pub is_error: bool,
    /// Arquivo gravado por esta chamada (wallpaper salvo), para o app oferecer "Aplicar".
    pub saved: Option<PathBuf>,
}

/// Último código que renderizou/compilou e ainda NÃO foi salvo: é o trabalho a recuperar se a
/// geração parar (cancelar, erro do provedor, limite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// `shader` ou `web`.
    pub kind: &'static str,
    /// `glsl`, `hlsl` ou `html`.
    pub language: &'static str,
    pub code: String,
}

pub trait Toolbox {
    fn defs(&self) -> Vec<ToolDef>;
    fn call(&mut self, name: &str, args: &Value) -> ToolOutput;
    /// Rascunho não salvo, se houver.
    fn draft(&self) -> Option<Draft> {
        None
    }
}

pub enum Event {
    /// Esperando o modelo responder.
    Thinking,
    /// Texto que o modelo escreveu para o usuário.
    Said(String),
    Tool { name: String, summary: String },
    ToolDone { name: String, ok: bool, text: String, images: Vec<Vec<u8>> },
    Saved(PathBuf),
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub max_steps: usize,
    /// Teto de tokens de saída somados na execução (proteção contra laços caros).
    pub max_output_tokens: u64,
    /// Teto de tokens lidos + escritos na execução. Cada rodada reenvia a conversa e as imagens, então
    /// só o teto de saída não segura o uso. É um limite de USO, não de dinheiro: o preço por token
    /// depende do modelo e do provedor.
    pub max_total_tokens: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_steps: 24, max_output_tokens: 250_000, max_total_tokens: u64::MAX }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum End {
    Completed,
    Cancelled,
    StepLimit,
    TokenLimit,
    Refused,
    Failed(String),
}

pub struct Outcome {
    pub end: End,
    pub saved: Vec<PathBuf>,
    pub usage: Usage,
    pub steps: usize,
}

fn summarize(name: &str, args: &Value) -> String {
    let code_len = args.get("code").or_else(|| args.get("html")).and_then(Value::as_str).map(str::len);
    match (name, code_len) {
        (_, Some(n)) => format!("{n} chars"),
        _ => args.to_string().chars().take(80).collect(),
    }
}

fn first_line(text: &str) -> String {
    let t = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    t.chars().take(160).collect()
}

/// Roda um pedido do usuário até o modelo parar de chamar ferramentas. Em falha ou cancelamento o
/// histórico volta ao que era antes do pedido (mantém a alternância de papéis válida); nos limites
/// de passos/tokens ele fica como está, para a conversa poder continuar.
#[allow(clippy::too_many_arguments)]
pub fn run(
    history: &mut Vec<Msg>,
    prompt: &str,
    system: &str,
    backend: &dyn Backend,
    tools: &mut dyn Toolbox,
    cancel: &AtomicBool,
    limits: Limits,
    sink: &mut dyn FnMut(Event),
) -> Outcome {
    let start_len = history.len();
    let defs = tools.defs();
    history.push(Msg::User(vec![Part::Text(prompt.to_string())]));
    let mut out = Outcome { end: End::Completed, saved: Vec::new(), usage: Usage::default(), steps: 0 };

    let finish = |history: &mut Vec<Msg>, mut out: Outcome, end: End| {
        if matches!(end, End::Cancelled | End::Failed(_) | End::Refused) {
            history.truncate(start_len);
        }
        out.end = end;
        out
    };

    loop {
        if cancel.load(Ordering::Relaxed) {
            return finish(history, out, End::Cancelled);
        }
        if out.steps >= limits.max_steps {
            return finish(history, out, End::StepLimit);
        }
        if out.usage.output >= limits.max_output_tokens
            || out.usage.input.saturating_add(out.usage.cached).saturating_add(out.usage.output) >= limits.max_total_tokens
        {
            return finish(history, out, End::TokenLimit);
        }
        out.steps += 1;
        sink(Event::Thinking);
        let turn = match backend.complete(system, history, &defs) {
            Ok(t) => t,
            Err(e) if e == "cancelled" => return finish(history, out, End::Cancelled),
            Err(e) => return finish(history, out, End::Failed(e)),
        };
        out.usage.input += turn.usage.input;
        out.usage.output += turn.usage.output;
        out.usage.cached += turn.usage.cached;
        if !turn.text.trim().is_empty() {
            sink(Event::Said(turn.text.trim().to_string()));
        }
        if turn.finish == Finish::Refusal {
            return finish(history, out, End::Refused);
        }
        history.push(Msg::Assistant { raw: turn.raw.clone() });
        if turn.calls.is_empty() {
            return match turn.finish {
                Finish::MaxTokens => finish(history, out, End::Failed("the model ran out of output space; ask for something simpler".into())),
                _ => finish(history, out, End::Completed),
            };
        }

        let mut results = Vec::with_capacity(turn.calls.len());
        for Call { id, name, args } in turn.calls {
            if cancel.load(Ordering::Relaxed) {
                return finish(history, out, End::Cancelled);
            }
            sink(Event::Tool { name: name.clone(), summary: summarize(&name, &args) });
            let r = tools.call(&name, &args);
            let text: String = r.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n");
            let images: Vec<Vec<u8>> = r.parts.iter().filter_map(|p| if let Part::Image(i) = p { Some(i.clone()) } else { None }).collect();
            if let Some(path) = r.saved {
                sink(Event::Saved(path.clone()));
                out.saved.push(path);
            }
            sink(Event::ToolDone { name: name.clone(), ok: !r.is_error, text: first_line(&text), images });
            results.push(ToolOut { call_id: id, name, parts: r.parts, is_error: r.is_error });
        }
        history.push(Msg::ToolResults(results));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    /// "Modelo" roteirizado: devolve os turnos em ordem e guarda quantas mensagens viu em cada chamada.
    struct Script {
        turns: RefCell<Vec<Result<Turn, String>>>,
        seen_lens: RefCell<Vec<usize>>,
    }

    impl Script {
        fn new(turns: Vec<Result<Turn, String>>) -> Self {
            Self { turns: RefCell::new(turns.into_iter().rev().collect()), seen_lens: RefCell::new(vec![]) }
        }
    }

    impl Backend for Script {
        fn complete(&self, _s: &str, history: &[Msg], _t: &[ToolDef]) -> Result<Turn, String> {
            self.seen_lens.borrow_mut().push(history.len());
            self.turns.borrow_mut().pop().expect("o roteiro acabou")
        }
    }

    fn say(text: &str) -> Result<Turn, String> {
        Ok(Turn { text: text.into(), calls: vec![], raw: json!([{"type": "text", "text": text}]), usage: Usage { input: 10, output: 5, cached: 0 }, finish: Finish::Done })
    }

    fn call(id: &str, name: &str, args: Value) -> Result<Turn, String> {
        Ok(Turn {
            text: String::new(),
            calls: vec![Call { id: id.into(), name: name.into(), args: args.clone() }],
            raw: json!([{"type": "tool_use", "id": id, "name": name, "input": args}]),
            usage: Usage { input: 10, output: 20, cached: 0 },
            finish: Finish::ToolUse,
        })
    }

    struct Fake {
        calls: Vec<String>,
        save_path: Option<PathBuf>,
    }

    impl Toolbox for Fake {
        fn defs(&self) -> Vec<ToolDef> {
            vec![ToolDef { name: "psl_render".into(), description: "d".into(), schema: json!({"type": "object"}) }]
        }
        fn call(&mut self, name: &str, args: &Value) -> ToolOutput {
            self.calls.push(name.to_string());
            match name {
                "psl_render" if args["code"] == "ruim" => ToolOutput { parts: vec![Part::Text("COMPILE ERROR\nline 1: bad".into())], is_error: true, saved: None },
                "psl_render" => ToolOutput { parts: vec![Part::Text("COMPILED OK".into()), Part::Image(vec![9, 9])], is_error: false, saved: None },
                "save_wallpaper" => ToolOutput { parts: vec![Part::Text("Saved".into())], is_error: false, saved: self.save_path.clone() },
                _ => ToolOutput { parts: vec![Part::Text("unknown tool".into())], is_error: true, saved: None },
            }
        }
    }

    fn drive(script: &Script, tools: &mut Fake, history: &mut Vec<Msg>, cancel: &AtomicBool, limits: Limits) -> (Outcome, Vec<String>) {
        let mut log = Vec::new();
        let out = run(history, "faça um oceano", "SYS", script, tools, cancel, limits, &mut |e| {
            log.push(match e {
                Event::Thinking => "thinking".to_string(),
                Event::Said(t) => format!("said:{t}"),
                Event::Tool { name, .. } => format!("tool:{name}"),
                Event::ToolDone { name, ok, images, .. } => format!("done:{name}:{ok}:{}", images.len()),
                Event::Saved(p) => format!("saved:{}", p.display()),
            });
        });
        (out, log)
    }

    #[test]
    fn iterates_fixes_errors_then_saves() {
        let script = Script::new(vec![
            call("a", "psl_render", json!({"code": "ruim"})),
            call("b", "psl_render", json!({"code": "bom"})),
            call("c", "save_wallpaper", json!({"name": "oceano"})),
            say("Pronto!"),
        ]);
        let mut tools = Fake { calls: vec![], save_path: Some(PathBuf::from("C:/w/oceano.hlsl")) };
        let mut history = Vec::new();
        let (out, log) = drive(&script, &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        assert_eq!(out.end, End::Completed);
        assert_eq!(out.steps, 4);
        assert_eq!(tools.calls, ["psl_render", "psl_render", "save_wallpaper"]);
        assert_eq!(out.saved, vec![PathBuf::from("C:/w/oceano.hlsl")]);
        assert_eq!(out.usage, Usage { input: 40, output: 65, cached: 0 });
        // o erro de compilação voltou ao modelo como resultado de ferramenta com is_error
        let Msg::ToolResults(first) = &history[2] else { panic!("esperava resultados") };
        assert!(first[0].is_error && first[0].call_id == "a");
        // o modelo viu a conversa crescer: user, +assistant, +results, ...
        assert_eq!(*script.seen_lens.borrow(), [1, 3, 5, 7]);
        assert!(log.contains(&"done:psl_render:false:0".to_string()) && log.contains(&"done:psl_render:true:1".to_string()));
        assert!(log.contains(&"saved:C:/w/oceano.hlsl".to_string()) && log.last().unwrap() == "said:Pronto!");
        assert_eq!(history.len(), 8, "user + 3x(assistant+results) + assistant final");
    }

    #[test]
    fn api_failure_rolls_the_history_back() {
        let script = Script::new(vec![call("a", "psl_render", json!({"code": "bom"})), Err("the provider is having trouble".into())]);
        let mut tools = Fake { calls: vec![], save_path: None };
        let mut history = vec![Msg::User(vec![Part::Text("antes".into())]), Msg::Assistant { raw: json!([]) }];
        let (out, _) = drive(&script, &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        assert_eq!(out.end, End::Failed("the provider is having trouble".into()));
        assert_eq!(history.len(), 2, "volta ao estado anterior ao pedido");
    }

    #[test]
    fn cancel_is_honored_between_steps_and_rolls_back() {
        let cancel = AtomicBool::new(false);
        let script = Script::new(vec![call("a", "psl_render", json!({"code": "bom"})), say("nunca chega")]);
        let mut tools = Fake { calls: vec![], save_path: None };
        let mut history = Vec::new();
        let mut log = Vec::new();
        let out = run(&mut history, "x", "SYS", &script, &mut tools, &cancel, Limits::default(), &mut |e| {
            if matches!(e, Event::ToolDone { .. }) {
                cancel.store(true, Ordering::Relaxed); // o usuário cancela com a ferramenta rodando
            }
            log.push(());
        });
        assert_eq!(out.end, End::Cancelled);
        assert!(history.is_empty());
        assert_eq!(tools.calls.len(), 1);
    }

    #[test]
    fn step_and_token_limits_stop_the_loop_but_keep_the_conversation() {
        let many = || (0..10).map(|i| call(&format!("c{i}"), "psl_render", json!({"code": "bom"}))).collect::<Vec<_>>();
        let mut tools = Fake { calls: vec![], save_path: None };
        let mut history = Vec::new();
        let (out, _) = drive(&Script::new(many()), &mut tools, &mut history, &AtomicBool::new(false), Limits { max_steps: 3, max_output_tokens: u64::MAX, max_total_tokens: u64::MAX });
        assert_eq!((out.end, out.steps), (End::StepLimit, 3));
        assert_eq!(history.len(), 7, "mantém user + 3 passos");
        assert!(matches!(history.last(), Some(Msg::ToolResults(_))), "termina num estado válido para continuar");

        let mut history = Vec::new();
        let (out, _) = drive(&Script::new(many()), &mut tools, &mut history, &AtomicBool::new(false), Limits { max_steps: 50, max_output_tokens: 50, max_total_tokens: u64::MAX });
        assert_eq!(out.end, End::TokenLimit);
        assert_eq!(out.usage.output, 60);

        // O teto total conta o que o modelo LÊ (entrada) além do que escreve: 30 tokens por passo aqui.
        let mut history = Vec::new();
        let (out, _) = drive(&Script::new(many()), &mut tools, &mut history, &AtomicBool::new(false), Limits { max_steps: 50, max_output_tokens: u64::MAX, max_total_tokens: 90 });
        assert_eq!((out.end, out.steps), (End::TokenLimit, 3));
        assert_eq!(out.usage.input + out.usage.output, 90);
        assert!(matches!(history.last(), Some(Msg::ToolResults(_))), "a conversa fica válida para continuar");
    }

    #[test]
    fn refusal_and_truncation_are_reported() {
        let refusal = Ok(Turn { text: String::new(), calls: vec![], raw: json!([]), usage: Usage::default(), finish: Finish::Refusal });
        let mut tools = Fake { calls: vec![], save_path: None };
        let mut history = Vec::new();
        let (out, _) = drive(&Script::new(vec![refusal]), &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        assert_eq!(out.end, End::Refused);
        assert!(history.is_empty());

        let cut = Ok(Turn { text: "meio".into(), calls: vec![], raw: json!([]), usage: Usage::default(), finish: Finish::MaxTokens });
        let (out, _) = drive(&Script::new(vec![cut]), &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        assert!(matches!(out.end, End::Failed(m) if m.contains("output space")));
    }

    #[test]
    fn a_second_request_continues_the_same_conversation() {
        let script = Script::new(vec![say("Feito."), say("Mais lento agora.")]);
        let mut tools = Fake { calls: vec![], save_path: None };
        let mut history = Vec::new();
        drive(&script, &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        drive(&script, &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        assert_eq!(*script.seen_lens.borrow(), [1, 3], "o segundo pedido enxerga o primeiro");
        assert_eq!(history.len(), 4);
    }

    #[test]
    fn unknown_tools_become_error_results_not_crashes() {
        let script = Script::new(vec![call("a", "formatar_disco", json!({})), say("ok")]);
        let mut tools = Fake { calls: vec![], save_path: None };
        let mut history = Vec::new();
        let (out, log) = drive(&script, &mut tools, &mut history, &AtomicBool::new(false), Limits::default());
        assert_eq!(out.end, End::Completed);
        assert!(log.contains(&"done:formatar_disco:false:0".to_string()));
    }
}
