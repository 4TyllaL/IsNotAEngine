//! Conversor de shaders do Shadertoy (GLSL, `mainImage`) para o contrato HLSL do motor.
//!
//! A conversão é textual, sobre tokens (sem expandir macros). O resultado é um shader
//! autossuficiente: declara o próprio `cbuffer` (prefixo `st_`, mesmo layout de
//! `ShaderConstants`) para que nomes do usuário como `time` ou `resolution` não colidam.
//!
//! Suportado: tipos e funções comuns, `iResolution`/`iTime`/`iMouse`/`iDate`/`iFrame`,
//! constantes globais, arrays com construtor e multiplicação de matrizes (os operandos são
//! trocados, pois o HLSL preenche matrizes por linha e o GLSL por coluna).
//! Não suportado (erro explícito): `iChannel*`/texturas, multipass, `inverse`, som.

use std::collections::HashSet;

/// Tamanho máximo aceito (igual ao limite do contrato de shader).
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

pub struct Converted {
    pub hlsl: String,
    pub warnings: Vec<String>,
    /// Linha (1-based) do `hlsl` onde começa o código do autor; serve para traduzir números de
    /// linha de erros do compilador de volta para o GLSL original.
    pub body_line: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum T {
    Ws(String),
    Comment(String),
    Id(String),
    Num(String),
    P(String),
}

impl T {
    fn text(&self) -> &str {
        match self {
            T::Ws(s) | T::Comment(s) | T::Id(s) | T::Num(s) | T::P(s) => s,
        }
    }
    fn is_trivia(&self) -> bool {
        matches!(self, T::Ws(_) | T::Comment(_))
    }
    fn is_p(&self, s: &str) -> bool {
        matches!(self, T::P(p) if p == s)
    }
    fn is_id(&self, s: &str) -> bool {
        matches!(self, T::Id(p) if p == s)
    }
}

// ---------------------------------------------------------------- lexer

fn tokenize(src: &str) -> Vec<T> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    const PUNCT3: [&str; 2] = ["<<=", ">>="];
    const PUNCT2: [&str; 17] = [
        "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=",
    ];
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' && matches!(c.get(i + 1), Some('\n') | Some('\r')) {
            // continuação de linha (macros): vira espaço que não encerra a diretiva
            let mut s = String::from("\\");
            i += 1;
            while i < c.len() && (c[i] == '\r' || c[i] == '\n') {
                s.push(c[i]);
                i += 1;
                if s.ends_with('\n') {
                    break;
                }
            }
            out.push(T::Ws(s));
        } else if ch.is_whitespace() {
            let s = i;
            while i < c.len() && c[i].is_whitespace() && c[i] != '\\' {
                i += 1;
            }
            let text: String = c[s..i].iter().collect();
            if text.contains('\n') {
                line_start = true;
            }
            out.push(T::Ws(text));
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            let s = i;
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            out.push(T::Comment(c[s..i].iter().collect()));
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            let s = i;
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(c.len());
            out.push(T::Comment(c[s..i].iter().collect()));
        } else if ch == '#' && line_start {
            // diretivas que só existem no GLSL são descartadas inteiras
            let mut j = i + 1;
            while j < c.len() && (c[j] == ' ' || c[j] == '\t') {
                j += 1;
            }
            let s = j;
            while j < c.len() && c[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word: String = c[s..j].iter().collect();
            if matches!(word.as_str(), "version" | "extension" | "pragma") {
                while j < c.len() && c[j] != '\n' {
                    j += 1;
                }
                i = j;
            } else {
                out.push(T::P("#".into()));
                i += 1;
                line_start = false;
            }
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let s = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(T::Id(c[s..i].iter().collect()));
            line_start = false;
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let s = i;
            while i < c.len() {
                let d = c[i];
                if d.is_ascii_alphanumeric() || d == '.' || d == '_' {
                    // `1e-3`: o sinal faz parte do número
                    if (d == 'e' || d == 'E') && !c[s..i].starts_with(&['0', 'x']) && matches!(c.get(i + 1), Some('+') | Some('-')) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(T::Num(c[s..i].iter().collect()));
            line_start = false;
        } else {
            let rest: String = c[i..(i + 3).min(c.len())].iter().collect();
            let p = PUNCT3
                .iter()
                .find(|p| rest.starts_with(**p))
                .or_else(|| PUNCT2.iter().find(|p| rest.starts_with(**p)))
                .map(|p| p.to_string())
                .unwrap_or_else(|| ch.to_string());
            i += p.chars().count();
            out.push(T::P(p));
            line_start = false;
        }
    }
    out
}

fn sig_prev(t: &[T], i: usize) -> Option<usize> {
    (0..i).rev().find(|&j| !t[j].is_trivia())
}

fn sig_next(t: &[T], i: usize) -> Option<usize> {
    (i..t.len()).find(|&j| !t[j].is_trivia())
}

fn matching_close(t: &[T], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (j, tok) in t.iter().enumerate().skip(open) {
        if let T::P(p) = tok {
            match p.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn matching_open(t: &[T], close: usize) -> Option<usize> {
    let mut depth = 0i32;
    for j in (0..=close).rev() {
        if let T::P(p) = &t[j] {
            match p.as_str() {
                ")" | "]" | "}" => depth += 1,
                "(" | "[" | "{" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

// ---------------------------------------------------------------- tabelas

const UNSUPPORTED: [&str; 17] = [
    "iChannel0", "iChannel1", "iChannel2", "iChannel3", "iChannelTime", "iChannelResolution", "texture",
    "textureLod", "texelFetch", "textureSize", "textureGrad", "texture2D", "textureCube", "inverse",
    "outerProduct", "gl_FragColor", "mainSound",
];

fn map_type(id: &str) -> Option<&'static str> {
    Some(match id {
        "vec2" => "float2",
        "vec3" => "float3",
        "vec4" => "float4",
        "ivec2" => "int2",
        "ivec3" => "int3",
        "ivec4" => "int4",
        "uvec2" => "uint2",
        "uvec3" => "uint3",
        "uvec4" => "uint4",
        "bvec2" => "bool2",
        "bvec3" => "bool3",
        "bvec4" => "bool4",
        "mat2" => "float2x2",
        "mat3" => "float3x3",
        "mat4" => "float4x4",
        _ => return None,
    })
}

fn map_func(id: &str) -> Option<&'static str> {
    Some(match id {
        "mix" => "lerp",
        "fract" => "frac",
        "mod" => "glsl_mod",
        "sign" => "glsl_sign",
        "inversesqrt" => "rsqrt",
        "dFdx" => "ddx",
        "dFdy" => "ddy",
        "floatBitsToInt" => "asint",
        "floatBitsToUint" => "asuint",
        "intBitsToFloat" | "uintBitsToFloat" => "asfloat",
        _ => return None,
    })
}

/// Nomes que no HLSL são palavras reservadas ou intrínsecas e que um shader GLSL pode usar
/// como identificador comum (`half`, `sample`, ou até redefinir `saturate`).
const RENAMED: [&str; 34] = [
    "half", "fixed", "line", "point", "triangle", "input", "output", "sample", "linear", "centroid", "precise",
    "matrix", "vector", "string", "packoffset", "register", "shared", "groupshared", "volatile", "extern",
    "inline", "snorm", "unorm", "class", "interface", "namespace", "typedef", "double", "dword", "decl",
    "sampler", "saturate", "lerp", "frac",
];
const RENAMED_INTRINSICS: [&str; 10] =
    ["mul", "rcp", "rsqrt", "ddx", "ddy", "atan2", "clip", "sincos", "noise", "isfinite"];

const MAT_TYPES: [&str; 3] = ["float2x2", "float3x3", "float4x4"];

fn is_hlsl_type(id: &str, structs: &HashSet<String>) -> bool {
    matches!(id, "float" | "int" | "uint" | "bool" | "void")
        || id.starts_with("float") && id[5..].chars().all(|c| c.is_ascii_digit() || c == 'x') && id.len() > 5
        || id.starts_with("int") && id.len() > 3 && id[3..].chars().all(|c| c.is_ascii_digit() || c == 'x')
        || id.starts_with("uint") && id.len() > 4 && id[4..].chars().all(|c| c.is_ascii_digit() || c == 'x')
        || id.starts_with("bool") && id.len() > 4 && id[4..].chars().all(|c| c.is_ascii_digit() || c == 'x')
        || structs.contains(id)
}

fn is_stmt_keyword(id: &str) -> bool {
    matches!(id, "return" | "if" | "while" | "for" | "switch" | "else" | "do" | "case")
}

// ---------------------------------------------------------------- passes

fn scan_unsupported(toks: &[T]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for t in toks {
        if let T::Id(id) = t
            && UNSUPPORTED.contains(&id.as_str())
            && !found.contains(id)
        {
            found.push(id.clone());
        }
    }
    found
}

/// Remove `precision ...;`, `uniform ...;` e qualificadores de precisão.
fn drop_glsl_only(toks: Vec<T>) -> Vec<T> {
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        let skip_stmt = match &toks[i] {
            T::Id(s) if s == "uniform" => true,
            T::Id(s) if s == "precision" => sig_next(&toks, i + 1).is_some_and(|n| {
                matches!(&toks[n], T::Id(q) if matches!(q.as_str(), "highp" | "mediump" | "lowp"))
            }),
            _ => false,
        };
        if skip_stmt {
            while i < toks.len() && !toks[i].is_p(";") {
                i += 1;
            }
            i += 1; // o ';'
            continue;
        }
        if matches!(&toks[i], T::Id(s) if matches!(s.as_str(), "highp" | "mediump" | "lowp")) {
            i += 1;
            continue;
        }
        out.push(toks[i].clone());
        i += 1;
    }
    out
}

/// Tipos, funções e nomes reservados. Idents depois de `.` (swizzle/membro) ficam como estão.
fn map_identifiers(toks: &[T]) -> Vec<T> {
    let mut out = toks.to_vec();
    for i in 0..toks.len() {
        let T::Id(id) = &toks[i] else { continue };
        if sig_prev(toks, i).is_some_and(|p| toks[p].is_p(".")) {
            continue;
        }
        let new = if let Some(t) = map_type(id) {
            t.to_string()
        } else if id == "atan" {
            let two_args = sig_next(toks, i + 1)
                .filter(|&n| toks[n].is_p("("))
                .and_then(|n| matching_close(toks, n).map(|c| (n, c)))
                .is_some_and(|(n, c)| {
                    let mut depth = 0;
                    toks[n..=c].iter().any(|t| match t {
                        T::P(p) if matches!(p.as_str(), "(" | "[") => {
                            depth += 1;
                            false
                        }
                        T::P(p) if matches!(p.as_str(), ")" | "]") => {
                            depth -= 1;
                            false
                        }
                        T::P(p) if p == "," && depth == 1 => true,
                        _ => false,
                    })
                });
            if two_args { "atan2".to_string() } else { "atan".to_string() }
        } else if let Some(f) = map_func(id) {
            f.to_string()
        } else if RENAMED.contains(&id.as_str()) || RENAMED_INTRINSICS.contains(&id.as_str()) {
            format!("{id}_")
        } else {
            continue;
        };
        out[i] = T::Id(new);
    }
    out
}

fn collect_structs(toks: &[T]) -> HashSet<String> {
    let mut s = HashSet::new();
    for i in 0..toks.len() {
        if toks[i].is_id("struct")
            && let Some(n) = sig_next(toks, i + 1)
            && let T::Id(name) = &toks[n]
        {
            s.insert(name.clone());
        }
    }
    s
}

/// Variáveis globais viram `static` (no HLSL, globais sem `static` são uniformes).
fn make_globals_static(toks: Vec<T>, structs: &HashSet<String>) -> Vec<T> {
    let mut out = Vec::with_capacity(toks.len() + 16);
    let (mut depth, mut at_start, mut in_pp, mut line_start) = (0i32, true, false, true);
    for i in 0..toks.len() {
        let tok = &toks[i];
        match tok {
            T::Ws(s) => {
                let continues = s.contains("\\\n") || s.contains("\\\r");
                if s.contains('\n') && !continues {
                    line_start = true;
                    if in_pp {
                        in_pp = false;
                        at_start = depth == 0;
                    }
                }
            }
            T::Comment(_) => {}
            _ => {
                if tok.is_p("#") && line_start {
                    in_pp = true;
                }
                line_start = false;
                if !in_pp {
                    if depth == 0 && at_start {
                        if let T::Id(id) = tok
                            && (id == "const" || is_hlsl_type(id, structs))
                            && id != "void"
                            && looks_like_variable(&toks, i)
                        {
                            out.push(T::Id("static".into()));
                            out.push(T::Ws(" ".into()));
                        }
                        at_start = false;
                    }
                    match tok {
                        T::P(p) if p == "{" => depth += 1,
                        T::P(p) if p == "}" => {
                            depth -= 1;
                            if depth == 0 {
                                at_start = true;
                            }
                        }
                        T::P(p) if p == ";" && depth == 0 => at_start = true,
                        _ => {}
                    }
                }
            }
        }
        out.push(tok.clone());
    }
    out
}

fn looks_like_variable(toks: &[T], start: usize) -> bool {
    for tok in &toks[start..] {
        match tok {
            T::P(p) if p == "(" => return false,
            T::P(p) if matches!(p.as_str(), "=" | ";" | "[" | ",") => return true,
            T::P(p) if p == "{" => return false,
            _ => {}
        }
    }
    false
}

/// `vec3[3](a, b, c)` → `{a, b, c}` (inicializador de array do HLSL).
fn rewrite_array_constructors(toks: Vec<T>, structs: &HashSet<String>) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        if let T::Id(id) = &toks[i]
            && is_hlsl_type(id, structs)
            && let Some(b) = sig_next(&toks, i + 1)
            && toks[b].is_p("[")
            && let Some(bc) = matching_close(&toks, b)
            && let Some(p) = sig_next(&toks, bc + 1)
            && toks[p].is_p("(")
            && let Some(pc) = matching_close(&toks, p)
        {
            out.push(T::P("{".into()));
            out.extend(toks[p + 1..pc].iter().cloned());
            out.push(T::P("}".into()));
            i = pc + 1;
            continue;
        }
        out.push(toks[i].clone());
        i += 1;
    }
    out
}

fn is_vector_type(id: &str) -> bool {
    ["float", "int", "uint", "bool"]
        .iter()
        .any(|b| id.strip_prefix(b).is_some_and(|n| matches!(n, "2" | "3" | "4")))
}

/// O HLSL não aceita `float4(1.0)`: com um único argumento vira cast (`((float4)(1.0))`).
/// `mat3(s)` é a diagonal no GLSL, então vai para um helper do prelúdio.
fn rewrite_single_arg_constructors(toks: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        if let T::Id(id) = &toks[i]
            && (is_vector_type(id) || MAT_TYPES.contains(&id.as_str()))
            && let Some(open) = sig_next(toks, i + 1)
            && toks[open].is_p("(")
            && let Some(close) = matching_close(toks, open)
        {
            let inner = rewrite_single_arg_constructors(&toks[open + 1..close]);
            let mut depth = 0;
            let single = inner.iter().any(|t| !t.is_trivia())
                && !inner.iter().any(|t| match t {
                    T::P(p) if matches!(p.as_str(), "(" | "[" | "{") => {
                        depth += 1;
                        false
                    }
                    T::P(p) if matches!(p.as_str(), ")" | "]" | "}") => {
                        depth -= 1;
                        false
                    }
                    T::P(p) if p == "," && depth == 0 => true,
                    _ => false,
                });
            if single && is_vector_type(id) {
                out.extend(tokenize(&format!("(({id})(")));
                out.extend(inner);
                out.extend(tokenize("))"));
            } else if single && !inner.iter().any(|t| t.is_id("transpose")) && MAT_TYPES.contains(&id.as_str()) {
                out.push(T::Id(format!("glsl_diag{}", &id[5..6])));
                out.push(T::P("(".into()));
                out.extend(inner);
                out.push(T::P(")".into()));
            } else {
                out.push(toks[i].clone());
                out.push(T::P("(".into()));
                out.extend(inner);
                out.push(T::P(")".into()));
            }
            i = close + 1;
            continue;
        }
        out.push(toks[i].clone());
        i += 1;
    }
    out
}

fn mat_names(toks: &[T]) -> HashSet<String> {
    let mut names: HashSet<String> =
        ["transpose", "glsl_diag2", "glsl_diag3", "glsl_diag4"].iter().map(|s| s.to_string()).collect();
    for i in 0..toks.len() {
        if let T::Id(id) = &toks[i]
            && MAT_TYPES.contains(&id.as_str())
            && let Some(n) = sig_next(toks, i + 1)
            && let T::Id(name) = &toks[n]
        {
            names.insert(name.clone());
        }
    }
    names
}

fn is_matrix_operand(toks: &[T], start: usize, end: usize, names: &HashSet<String>) -> bool {
    let sig: Vec<usize> = (start..end).filter(|&j| !toks[j].is_trivia()).collect();
    let Some(&first) = sig.first() else { return false };
    let T::Id(id) = &toks[first] else { return false };
    let next_is = |s: &str| sig.get(1).is_some_and(|&n| toks[n].is_p(s));
    if MAT_TYPES.contains(&id.as_str()) {
        return next_is("(");
    }
    if names.contains(id) {
        return sig.len() == 1 || next_is("(");
    }
    false
}

fn right_operand(toks: &[T], op: usize) -> Option<(usize, usize)> {
    let mut i = sig_next(toks, op + 1)?;
    let start = i;
    while matches!(&toks[i], T::P(p) if matches!(p.as_str(), "-" | "+" | "!" | "~")) {
        i = sig_next(toks, i + 1)?;
    }
    i = match &toks[i] {
        T::Num(_) | T::Id(_) => i + 1,
        T::P(p) if p == "(" => matching_close(toks, i)? + 1,
        _ => return None,
    };
    while let Some(n) = sig_next(toks, i) {
        match &toks[n] {
            T::P(p) if p == "(" || p == "[" => i = matching_close(toks, n)? + 1,
            T::P(p) if p == "." => {
                let m = sig_next(toks, n + 1)?;
                if !matches!(toks[m], T::Id(_)) {
                    break;
                }
                i = m + 1;
            }
            _ => break,
        }
    }
    Some((start, i))
}

fn left_operand(toks: &[T], op: usize) -> Option<(usize, usize)> {
    let last = sig_prev(toks, op)?;
    let end = last + 1;
    let mut j = last;
    loop {
        match &toks[j] {
            T::P(p) if p == ")" || p == "]" => {
                j = matching_open(toks, j)?;
                match sig_prev(toks, j) {
                    Some(p) => match &toks[p] {
                        T::Id(name) if !is_stmt_keyword(name) => j = p,
                        T::P(c) if c == ")" || c == "]" => {
                            j = p;
                            continue;
                        }
                        _ => return Some((j, end)),
                    },
                    None => return Some((j, end)),
                }
            }
            T::Id(_) | T::Num(_) => {}
            _ => return None,
        }
        match sig_prev(toks, j) {
            Some(p) if toks[p].is_p(".") => {
                j = sig_prev(toks, p)?;
                continue;
            }
            _ => return Some((j, end)),
        }
    }
}

fn text_of(toks: &[T], a: usize, b: usize) -> String {
    toks[a..b].iter().map(T::text).collect::<String>().trim().to_string()
}

/// `M * v` e `v * M` viram `mul(v, M)` / `mul(M, v)`; ver nota do módulo sobre a ordem.
fn rewrite_matrix_products(mut toks: Vec<T>, names: &HashSet<String>) -> (Vec<T>, usize) {
    let (mut k, mut rewritten) = (0, 0);
    while k < toks.len() {
        let is_mul = toks[k].is_p("*");
        let is_mul_assign = toks[k].is_p("*=");
        if !(is_mul || is_mul_assign) {
            k += 1;
            continue;
        }
        let (Some((ls, le)), Some((rs, re))) = (left_operand(&toks, k), right_operand(&toks, k)) else {
            k += 1;
            continue;
        };
        if !(is_matrix_operand(&toks, ls, le, names) || is_matrix_operand(&toks, rs, re, names)) {
            k += 1;
            continue;
        }
        let (l, r) = (text_of(&toks, ls, le), text_of(&toks, rs, re));
        let replacement = if is_mul { format!("mul({r}, {l})") } else { format!("{l} = mul({r}, {l})") };
        toks.splice(ls..re, tokenize(&replacement));
        rewritten += 1;
        k = ls; // reprocessa: os operandos copiados podem ter mais produtos
    }
    (toks, rewritten)
}

// ---------------------------------------------------------------- saída

const PRELUDE: &str = r#"cbuffer Constants : register(b0)
{
    float2 st_res; float st_time; float st_speed; float4 st_tint; float2 st_mouse; float2 st_click;
    float st_menabled; float st_parallax; float2 st_tintref; float4 st_clock; float4 st_date;
    float4 st_audio; float4 st_spec[4]; float4 st_params[2]; float4 st_fade;
};

float4 vs_main(uint id : SV_VertexID) : SV_Position
{
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

static float3 iResolution; static float iTime; static float iTimeDelta; static int iFrame;
static float iFrameRate; static float4 iMouse; static float4 iDate; static float4 gl_FragCoord;

float  glsl_mod(float x, float y)   { return x - y * floor(x / y); }
float2 glsl_mod(float2 x, float2 y) { return x - y * floor(x / y); }
float2 glsl_mod(float2 x, float y)  { return x - y * floor(x / y); }
float3 glsl_mod(float3 x, float3 y) { return x - y * floor(x / y); }
float3 glsl_mod(float3 x, float y)  { return x - y * floor(x / y); }
float4 glsl_mod(float4 x, float4 y) { return x - y * floor(x / y); }
float4 glsl_mod(float4 x, float y)  { return x - y * floor(x / y); }
float2x2 glsl_diag2(float s) { return float2x2(s, 0, 0, s); }
float3x3 glsl_diag3(float s) { return float3x3(s, 0, 0, 0, s, 0, 0, 0, s); }
float4x4 glsl_diag4(float s) { return float4x4(s, 0, 0, 0, 0, s, 0, 0, 0, 0, s, 0, 0, 0, 0, s); }
float  glsl_sign(float x)   { return x > 0.0 ? 1.0 : (x < 0.0 ? -1.0 : 0.0); }
float2 glsl_sign(float2 x)  { return (float2)sign(x); }
float3 glsl_sign(float3 x)  { return (float3)sign(x); }
float4 glsl_sign(float4 x)  { return (float4)sign(x); }
bool2 lessThan(float2 a, float2 b) { return a < b; }
bool3 lessThan(float3 a, float3 b) { return a < b; }
bool4 lessThan(float4 a, float4 b) { return a < b; }
bool2 greaterThan(float2 a, float2 b) { return a > b; }
bool3 greaterThan(float3 a, float3 b) { return a > b; }
bool4 greaterThan(float4 a, float4 b) { return a > b; }
bool2 lessThanEqual(float2 a, float2 b) { return a <= b; }
bool3 lessThanEqual(float3 a, float3 b) { return a <= b; }
bool4 lessThanEqual(float4 a, float4 b) { return a <= b; }
bool2 greaterThanEqual(float2 a, float2 b) { return a >= b; }
bool3 greaterThanEqual(float3 a, float3 b) { return a >= b; }
bool4 greaterThanEqual(float4 a, float4 b) { return a >= b; }
"#;

const WRAPPER: &str = r#"
float4 ps_main(float4 pos : SV_Position) : SV_Target
{
    iResolution = float3(st_res, 1.0);
    iTime = st_time * (st_speed > 0.0 ? st_speed : 1.0);
    iTimeDelta = 1.0 / 60.0;
    iFrame = (int)(iTime * 60.0);
    iFrameRate = 60.0;
    float2 fc = float2(pos.x, st_res.y - pos.y);
    gl_FragCoord = float4(fc, 0.5, 1.0);
    float2 m = st_mouse * st_res;
    bool down = st_click.x > 0.5 || st_click.y > 0.5;
    iMouse = st_menabled > 0.5 ? float4(m, down ? m : -m) : float4(0.0, 0.0, 0.0, 0.0);
    iDate = float4(st_date.x, st_date.y - 1.0, st_date.z, st_clock.x * 3600.0 + st_clock.y * 60.0 + st_clock.w);
    float4 c = float4(0.0, 0.0, 0.0, 1.0);
    mainImage(c, fc);
    return float4(saturate(c.rgb) * (1.0 - saturate(st_fade.x)), 1.0);
}
"#;

/// Nome de arquivo seguro (sem extensão) a partir do título.
pub fn file_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { ' ' })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join("_");
    if s.is_empty() { "shadertoy".into() } else { s.chars().take(48).collect() }
}

pub fn convert(src: &str, title: &str) -> Result<Converted, String> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(format!("Código grande demais ({} KiB; o limite é {} KiB).", src.len() / 1024, MAX_SOURCE_BYTES / 1024));
    }
    let toks = tokenize(src);
    let bad = scan_unsupported(&toks);
    if !bad.is_empty() {
        let channel = bad.iter().any(|b| b.starts_with("iChannel") || b.starts_with("texture") || b == "texelFetch");
        let mut msg = format!("Recursos sem suporte: {}.", bad.join(", "));
        if channel {
            msg.push_str(" Texturas e buffers (iChannel) não são suportados; só shaders de passe único que não usam texturas.");
        }
        return Err(msg);
    }
    if !toks.iter().any(|t| t.is_id("mainImage")) {
        return Err("Não encontrei `mainImage`. Cole o código da aba \"Image\" do Shadertoy.".into());
    }

    let toks = drop_glsl_only(toks);
    let toks = map_identifiers(&toks);
    let toks = rewrite_single_arg_constructors(&toks);
    let structs = collect_structs(&toks);
    let toks = make_globals_static(toks, &structs);
    let toks = rewrite_array_constructors(toks, &structs);
    let names = mat_names(&toks);
    let (toks, products) = rewrite_matrix_products(toks, &names);

    let mut warnings = Vec::new();
    if products > 0 {
        warnings.push(format!("{products} produto(s) com matriz reescrito(s); confira se a rotação/transformação ficou certa."));
    }

    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    let body: String = toks.iter().map(T::text).collect();
    let hlsl = format!(
        "// Importado do Shadertoy: {title}\n// Mantenha os créditos do autor (a licença padrão do Shadertoy é CC BY-NC-SA 3.0).\n// Convertido automaticamente de GLSL; não edite o prelúdio.\n// bloom: 0\n{PRELUDE}\n{body}\n{WRAPPER}"
    );
    // hlsl = prefixo + corpo + "\n" + WRAPPER: o corpo termina logo antes do "\n" que precede o WRAPPER.
    let body_end = hlsl.len() - WRAPPER.len() - 1;
    let body_line = hlsl[..body_end - body.len()].matches('\n').count() as u32 + 1;
    Ok(Converted { hlsl, warnings, body_line })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiles(hlsl: &str) -> Result<(), String> {
        crate::render::pipeline::compile_uncached(&crate::wallpaper::catalog::with_common(hlsl), "t")
            .map(|_| ())
            .map_err(|e| format!("{e}\n---\n{hlsl}"))
    }

    fn convert_ok(src: &str) -> Converted {
        convert(src, "teste").unwrap_or_else(|e| panic!("conversão falhou: {e}"))
    }

    #[test]
    fn default_shadertoy_shader_compiles() {
        let c = convert_ok(
            "void mainImage( out vec4 fragColor, in vec2 fragCoord )\n{\n    vec2 uv = fragCoord/iResolution.xy;\n    vec3 col = 0.5 + 0.5*cos(iTime+uv.xyx+vec3(0,2,4));\n    fragColor = vec4(col,1.0);\n}\n",
        );
        compiles(&c.hlsl).unwrap();
    }

    #[test]
    fn globals_functions_and_renames() {
        let c = convert_ok(
            "#version 300 es\nprecision highp float;\nconst float PI = 3.14159;\nvec3 palette[3] = vec3[3](vec3(1,0,0), vec3(0,1,0), vec3(0,0,1));\nfloat half = 2.0;\n\
             float hash(vec2 p){ return fract(sin(dot(p, vec2(12.9898,78.233)))*43758.5453); }\n\
             void mainImage(out vec4 O, in vec2 U){\n vec2 p = U/iResolution.xy;\n float a = atan(p.y-.5, p.x-.5);\n float m = mod(a, 1.0) + sign(a) + half;\n\
              O = vec4(mix(palette[1], palette[2], hash(p)) * m * PI, 1.0);\n}\n",
        );
        assert!(c.hlsl.contains("static const float PI"));
        assert!(c.hlsl.contains("static float3 palette[3] = {"));
        assert!(c.hlsl.contains("atan2(p.y-.5, p.x-.5)"));
        assert!(c.hlsl.contains("half_"));
        assert!(!c.hlsl.contains("#version"));
        compiles(&c.hlsl).unwrap();
    }

    #[test]
    fn matrix_products_swap_operands() {
        let c = convert_ok(
            "mat2 rot(float a){ float c=cos(a), s=sin(a); return mat2(c,-s,s,c); }\n\
             void mainImage(out vec4 O, in vec2 U){\n vec2 p = (U - .5*iResolution.xy)/iResolution.y;\n p *= rot(iTime);\n vec2 q = rot(1.0) * p;\n vec2 r = p * rot(2.0);\n O = vec4(p + q + r, 0., 1.);\n}\n",
        );
        assert!(c.hlsl.contains("p = mul(rot(iTime), p)"), "{}", c.hlsl);
        assert!(c.hlsl.contains("mul(p, rot(1.0))"));
        assert!(c.hlsl.contains("mul(rot(2.0), p)"));
        assert_eq!(c.warnings.len(), 1);
        compiles(&c.hlsl).unwrap();
    }

    #[test]
    fn glsl_matrix_semantics_match_hlsl_after_swap() {
        // GLSL: M = mat2(1,2,3,4) tem colunas (1,2) e (3,4); M * (1,0) = (1,2) e (1,0) * M = (1,3).
        // O HLSL guarda as mesmas quatro entradas por linha: mul(v, M2) = M*v, mul(M2, v) = v*M.
        let (m2, v) = ([[1.0f32, 2.0], [3.0, 4.0]], [1.0f32, 0.0]);
        let mul_vm = [v[0] * m2[0][0] + v[1] * m2[1][0], v[0] * m2[0][1] + v[1] * m2[1][1]];
        let mul_mv = [m2[0][0] * v[0] + m2[0][1] * v[1], m2[1][0] * v[0] + m2[1][1] * v[1]];
        assert_eq!(mul_vm, [1.0, 2.0]);
        assert_eq!(mul_mv, [1.0, 3.0]);
    }

    #[test]
    fn unsupported_features_are_reported() {
        let e = convert("void mainImage(out vec4 O, in vec2 U){ O = texture(iChannel0, U/iResolution.xy); }", "x").err().unwrap();
        assert!(e.contains("iChannel0") && e.contains("texture"), "{e}");
        let e = convert("void foo(){}", "x").err().unwrap();
        assert!(e.contains("mainImage"));
    }

    #[test]
    fn swizzles_after_dot_are_not_renamed() {
        let c = convert_ok("void mainImage(out vec4 O, in vec2 U){ vec4 v = vec4(1); O = vec4(v.xyz, v.w); }");
        assert!(c.hlsl.contains("v.xyz"));
        compiles(&c.hlsl).unwrap();
    }

    #[test]
    fn user_names_that_clash_with_hlsl_compile() {
        let c = convert_ok(
            "float saturate(float x){ return clamp(x, 0., 1.); }\nvoid mainImage(out vec4 O, in vec2 U){ float sample = saturate(U.x/iResolution.x); float time = iTime; O = vec4(vec3(sample*time), 1); }",
        );
        compiles(&c.hlsl).unwrap();
    }

    const FBM: &str = r#"
// fbm clássico com matriz global
const mat2 m = mat2(0.8, -0.6, 0.6, 0.8);
float hash(vec2 p) { p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float noise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
float fbm(vec2 p) {
    float v = 0.0, a = 0.5;
    for (int i = 0; i < 5; i++) { v += a * noise(p); p = m * p * 2.0; a *= 0.5; }
    return v;
}
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord - iResolution.xy) / iResolution.y;
    float t = iTime * 0.2;
    float n = fbm(uv * 3.0 + vec2(t, -t));
    vec3 col = mix(vec3(0.05, 0.1, 0.3), vec3(0.9, 0.6, 0.2), smoothstep(0.2, 0.8, n));
    col *= 1.0 - 0.5 * length(uv);
    col = pow(col, vec3(0.4545));
    float stripes = mod(floor(fragCoord.x / 8.0), 2) + mod(3.0, 2.0);
    fragColor = vec4(col * (0.9 + 0.1 * stripes), 1.0);
}
"#;

    const RAYMARCH: &str = r#"
#define MAX_STEPS 64
#define EPS 0.001
mat3 rotY(float a) { float c = cos(a), s = sin(a); return mat3(c, 0, -s, 0, 1, 0, s, 0, c); }
float sdSphere(vec3 p, float r) { return length(p) - r; }
float sdBox(vec3 p, vec3 b) { vec3 d = abs(p) - b; return min(max(d.x, max(d.y, d.z)), 0.0) + length(max(d, 0.0)); }
float map(vec3 p) {
    p = rotY(iTime) * p;
    return min(sdSphere(p - vec3(0.8, 0, 0), 0.5), sdBox(p, vec3(0.4)));
}
vec3 normal(vec3 p) {
    vec2 e = vec2(EPS, 0);
    return normalize(vec3(map(p + e.xyy) - map(p - e.xyy), map(p + e.yxy) - map(p - e.yxy), map(p + e.yyx) - map(p - e.yyx)));
}
void mainImage(out vec4 O, in vec2 U) {
    vec2 uv = (U - 0.5 * iResolution.xy) / iResolution.y;
    vec3 ro = vec3(0, 0, -3), rd = normalize(vec3(uv, 1.5));
    float d = 0.0; bool hit = false;
    for (int i = 0; i < MAX_STEPS; i++) {
        float h = map(ro + rd * d);
        if (h < EPS) { hit = true; break; }
        d += h; if (d > 10.0) break;
    }
    vec3 col = vec3(0.0);
    if (hit) { vec3 n = normal(ro + rd * d); col = vec3(0.5) + 0.5 * n; col *= max(dot(n, normalize(vec3(1, 1, -1))), 0.1); }
    mat3 I = mat3(1.0);
    col = I * col;
    O = vec4(sqrt(col), 1.0);
}
"#;

    #[test]
    fn realistic_shaders_compile() {
        for (name, src) in [("fbm", FBM), ("raymarch", RAYMARCH)] {
            let c = convert(src, name).unwrap_or_else(|e| panic!("{name}: {e}"));
            compiles(&c.hlsl).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    /// Renderiza 1x1 na GPU e devolve o pixel RGBA (sem gama: o Shadertoy escreve direto).
    fn render_pixel(src: &str) -> [u8; 4] {
        use crate::render::offscreen::{OffscreenRenderer, preview_constants};
        let c = convert_ok(src);
        let mut r = OffscreenRenderer::new().expect("GPU");
        let px = r
            .render("t", &crate::wallpaper::catalog::with_common(&c.hlsl), preview_constants(1, 1, 0.0, [0.0; 4]))
            .expect("render");
        [px[0], px[1], px[2], px[3]]
    }

    #[test]
    fn matrix_products_match_glsl_on_the_gpu() {
        // GLSL: mat2(1,2,3,4) tem colunas (1,2) e (3,4). M*(1,0) = (1,2); (1,0)*M = (1,3).
        let m_v = render_pixel("void mainImage(out vec4 O, in vec2 U){ O = vec4((mat2(1.,2.,3.,4.) * vec2(1,0))/10., 0., 1.); }");
        let v_m = render_pixel("void mainImage(out vec4 O, in vec2 U){ O = vec4((vec2(1,0) * mat2(1.,2.,3.,4.))/10., 0., 1.); }");
        // 0.1, 0.2 e 0.3 em 8 bits (a conversão pode arredondar para baixo ou para cima)
        let near = |a: u8, b: u8| a.abs_diff(b) <= 1;
        assert!(near(m_v[0], 26) && near(m_v[1], 51), "M*v: {m_v:?}");
        assert!(near(v_m[0], 26) && near(v_m[1], 77), "v*M: {v_m:?}");
    }

    #[test]
    fn mod_sign_atan_and_diagonal_match_glsl_on_the_gpu() {
        // mod(-1.0, 3.0) = 2 (não -1 como o fmod); sign(-2) = -1 -> 0 após saturate; mat2(1.0)*(.5,.25) = (.5,.25)
        let px = render_pixel(
            "void mainImage(out vec4 O, in vec2 U){ vec2 d = mat2(1.0) * vec2(.5, .25); O = vec4(mod(-1.0, 3.0)/4., d.y, d.x, 1.); }",
        );
        let near = |a: u8, b: u8| a.abs_diff(b) <= 1;
        assert!(near(px[0], 128) && near(px[1], 64) && near(px[2], 128), "{px:?}");
    }

    /// Ferramenta manual: `ST_IN=arquivo.glsl ST_OUT=pasta cargo test render_glsl_file -- --ignored`
    /// converte, grava o `.hlsl` e renderiza quadros em PNG para olhar o resultado.
    #[test]
    #[ignore]
    fn render_glsl_file() {
        use crate::render::offscreen::{OffscreenRenderer, encode_png, preview_constants};
        let (input, out) = (std::env::var("ST_IN").expect("ST_IN"), std::env::var("ST_OUT").expect("ST_OUT"));
        let src = std::fs::read_to_string(&input).expect("ler ST_IN");
        let c = convert(&src, "manual").unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(std::path::Path::new(&out).join("converted.hlsl"), &c.hlsl).unwrap();
        let mut r = OffscreenRenderer::new().expect("GPU");
        let code = crate::wallpaper::catalog::with_common(&c.hlsl);
        for t in [2.0f32, 9.0, 20.0] {
            let px = r.render("m", &code, preview_constants(960, 540, t, [0.0; 4])).expect("render");
            let png = encode_png(&px, 960, 540).unwrap();
            std::fs::write(std::path::Path::new(&out).join(format!("t{}.png", t as u32)), png).unwrap();
        }
    }

    #[test]
    fn output_is_not_altered_by_bloom() {
        let px = render_pixel("void mainImage(out vec4 O, in vec2 U){ O = vec4(0.5, 0.6, 0.75, 1.); }");
        let near = |a: u8, b: u8| a.abs_diff(b) <= 1;
        assert!(near(px[0], 128) && near(px[1], 153) && near(px[2], 191), "{px:?}");
    }

    #[test]
    fn single_arg_constructors_become_casts() {
        let c = convert_ok("void mainImage(out vec4 O, in vec2 U){ O = vec4(vec3(0.5), 1.); mat3 I = mat3(1.0); }");
        assert!(c.hlsl.contains("((float3)(0.5))"), "{}", c.hlsl);
        assert!(c.hlsl.contains("glsl_diag3(1.0)"));
        assert!(c.hlsl.contains("float4(((float3)(0.5)), 1.)"));
    }

    #[test]
    fn body_line_points_at_the_authors_first_line() {
        let c = convert_ok("// primeira linha do autor\nvoid mainImage(out vec4 O, in vec2 U){ O = vec4(1.); }\n");
        let line = c.hlsl.lines().nth(c.body_line as usize - 1).unwrap();
        assert_eq!(line, "// primeira linha do autor");
    }

    #[test]
    fn oversized_source_is_rejected() {
        assert!(convert(&"a".repeat(MAX_SOURCE_BYTES + 1), "x").is_err());
    }

    #[test]
    fn file_stem_is_safe() {
        assert_eq!(file_stem("Plasma / Cool!"), "Plasma_Cool");
        assert_eq!(file_stem("../../x"), "x");
        assert_eq!(file_stem("///"), "shadertoy");
    }
}
