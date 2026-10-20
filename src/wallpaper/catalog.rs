//! Catálogo de wallpapers embutidos no binário.
//!
//! Tudo aqui é compilado para dentro do executável (`include_str!`), então o catálogo
//! funciona independente do diretório de trabalho (autostart, atalhos, instalador).
//! Durante o desenvolvimento, se `assets/wallpapers/shaders/<id>.hlsl` existir relativo ao
//! diretório atual, ele é carregado do disco com hot-reload.

use serde::Serialize;

/// Header HLSL prefixado em shaders que não declaram o próprio `cbuffer`.
pub const COMMON_HLSL: &str = include_str!("../../assets/wallpapers/common.hlsli");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Shader,
    Web,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Entry {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub kind: Kind,
    /// Reage ao áudio do sistema (WASAPI).
    pub audio: bool,
    #[serde(skip)]
    pub source: &'static str,
}

#[allow(unused_macros)] // reservado: a lista está sendo refeita e novos shaders voltam a usar o macro
macro_rules! shader {
    ($id:literal, $title:literal, $cat:literal, $audio:literal, $desc:literal) => {
        Entry {
            id: $id,
            title: $title,
            category: $cat,
            description: $desc,
            kind: Kind::Shader,
            audio: $audio,
            source: include_str!(concat!("../../assets/wallpapers/shaders/", $id, ".hlsl")),
        }
    };
}

macro_rules! web {
    ($id:literal, $title:literal, $cat:literal, $audio:literal, $desc:literal) => {
        Entry {
            id: $id,
            title: $title,
            category: $cat,
            description: $desc,
            kind: Kind::Web,
            audio: $audio,
            source: include_str!(concat!("../../assets/wallpapers/web/", $id, ".html")),
        }
    };
}

pub static CATALOG: &[Entry] = &[
    // Tech
    web!("matrix", "Matrix", "Tech", true, "Chuva digital com katakana espelhado; acelera com os graves."),
    web!("coding", "Coding", "Tech", false, "Uma IDE digitando código sozinha, com terminal compilando."),
    web!("system", "System · Dashboard", "Tech", false, "CPU por núcleo, memória, disco e rede reais da sua máquina, com relógio e uptime."),
    web!("terminal_build", "Terminal · Build", "Tech", false, "Um pipeline de CI rolando devagar: checkout, build, testes, lint e pacote, com o log e os testes reais do projeto."),
    web!("git_graph", "Git · Graph", "Tech", false, "Grafo de commits com ramos, forks, merges e tags, com os assuntos do próprio repositório."),
    web!("clock_mono", "Clock · Mono", "Tech", false, "Relógio minimalista em matriz de pontos, com régua de segundos, data e progresso do dia."),
    web!("terminal_boot", "Terminal · Boot", "Tech", false, "Boot de uma máquina Linux em loop: POST, kernel, serviços, login, shell e reboot."),
    web!("descriptografia", "Descriptografia", "Tech", false, "Campo de glifos cinza com ondas mais claras em movimento; o clique espalha um pulso na cor de destaque."),
    shader!("aurora", "Aurora", "Ambient", false, "Aurora boreal com cortinas de raios finos sobre serra e lago, com estrelas."),
    shader!("night_sky", "Céu noturno", "Ambient", false, "Campo denso de estrelas, Via Láctea com faixas de poeira e estrelas cadentes raras."),
    shader!("topo_dark", "Topografia escura", "Ambient", false, "Curvas de nível finas sobre um relevo que evolui devagar, com marcas de levantamento."),
    shader!("rain_glass", "Chuva na janela", "Ambient", false, "Gotas escorrendo em zigue-zague sobre uma cidade noturna desfocada; dentro de cada gota a cidade aparece nítida."),
    shader!("liquid_chrome", "Liquid Chrome", "Ambient", false, "Superfície metálica líquida refletindo um estúdio com softboxes."),
    shader!("plasma_flow", "Plasma Flow", "Ambient", false, "Campos de cor suaves que se interpenetram, com paleta contida e movimento em camadas."),
    shader!("smoke", "Fumaça", "Ambient", false, "Fumaça em domain warping lento com iluminação lateral e grão de filme. Ajustes: velocidade, brilho, calor e densidade."),
    shader!("ocean", "Oceano", "Ambient", false, "Marés de névoa iluminadas por trás, com sombra própria e filamentos finos."),
    shader!("gyroid_silk", "Gyroid · Seda", "Ambient", false, "Voo por dentro de uma superfície giroide: dobras de seda neon que mudam de cor devagar. Ajustes: voo, brilho, cores e névoa."),
];

pub fn find(id: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// Resolve identificadores antigos/caminhos (`assets/wallpapers/x.hlsl`, `x.html`) para o id.
pub fn resolve(path_or_id: &str) -> Option<&'static Entry> {
    let stem = std::path::Path::new(path_or_id)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path_or_id);
    find(stem)
}

/// Prefixa o header comum quando o shader não declara o próprio cbuffer.
pub fn with_common(code: &str) -> String {
    if code.contains("cbuffer") {
        code.to_string()
    } else {
        format!("{COMMON_HLSL}\n#line 1\n{code}")
    }
}

/// Catálogo em JSON para o Studio. Shaders levam `params`: os `// param:` declarados no código
/// (nome, faixa, padrão e descrição), para a interface montar os controles.
pub fn catalog_json() -> String {
    let items: Vec<serde_json::Value> = CATALOG
        .iter()
        .map(|e| {
            let mut v = serde_json::to_value(e).unwrap_or_default();
            if e.kind == Kind::Shader {
                v["params"] = serde_json::to_value(crate::render::params::parse(e.source).0).unwrap_or_default();
            }
            v
        })
        .collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".into())
}

/// Modelo de partida do Lab: um shader original, pequeno, com parâmetros descritos.
pub struct Template {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub source: &'static str,
}

pub static TEMPLATES: &[Template] = &[
    Template {
        id: "ondas",
        title: "Modelo · Ondas",
        description: "Linhas de onda que ondulam, com cor, espessura e velocidade ajustáveis.",
        source: include_str!("../../assets/wallpapers/templates/ondas.hlsl"),
    },
    Template {
        id: "grade_neon",
        title: "Modelo · Grade neon",
        description: "Uma grade em perspectiva que avança, com brilho e densidade ajustáveis.",
        source: include_str!("../../assets/wallpapers/templates/grade_neon.hlsl"),
    },
    Template {
        id: "nevoa",
        title: "Modelo · Névoa de cor",
        description: "Névoa lenta em duas cores, com escala, contraste e velocidade ajustáveis.",
        source: include_str!("../../assets/wallpapers/templates/nevoa.hlsl"),
    },
];

#[cfg(test)]
mod template_tests {
    use super::*;
    use crate::render::params;

    #[test]
    fn templates_compile_and_declare_described_params_in_range() {
        for t in TEMPLATES {
            let code = with_common(t.source);
            crate::render::pipeline::compile_uncached(&code, t.id).unwrap_or_else(|e| panic!("modelo {} não compila: {e}", t.id));
            let (specs, problems) = params::parse(t.source);
            assert!(problems.is_empty(), "{}: {problems:?}", t.id);
            assert!((3..=params::MAX_PARAMS).contains(&specs.len()), "{}: {} parâmetros", t.id, specs.len());
            for s in &specs {
                assert!(!s.description.is_empty(), "{}.{} sem descrição", t.id, s.name);
                assert!(s.min < s.max && (s.min..=s.max).contains(&s.default), "{}.{} faixa/padrão inválidos", t.id, s.name);
            }
            // O shader só usa P(i) para parâmetros declarados.
            let used = t.source.matches("P(").count();
            assert!(used >= specs.len(), "{}: parâmetros declarados mas pouco usados", t.id);
        }
    }

    #[test]
    fn templates_render_something_and_params_change_the_image() {
        let Ok(mut r) = crate::render::offscreen::OffscreenRenderer::new() else { return };
        let consts = || crate::render::offscreen::preview_constants(96, 54, 4.0, [0.0; 4]);
        for t in TEMPLATES {
            let code = with_common(t.source);
            let base = r.render_with_params(t.id, &code, consts(), Some(&[])).expect("render");
            let lum: Vec<f32> = base.chunks(4).map(|p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 765.0).collect();
            let mean = lum.iter().sum::<f32>() / lum.len() as f32;
            let std = (lum.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lum.len() as f32).sqrt();
            assert!(std > 0.01 && mean > 0.005, "{}: imagem chapada/preta (média {mean}, desvio {std})", t.id);
            // Subir o brilho (último param declarado como `brilho`) muda a imagem.
            let specs = params::parse(t.source).0;
            if let Some(b) = specs.iter().find(|s| s.name == "brilho") {
                let bright = r.render_with_params(t.id, &code, consts(), Some(&[("brilho".into(), b.max)])).expect("render");
                let dim = r.render_with_params(t.id, &code, consts(), Some(&[("brilho".into(), b.min)])).expect("render");
                assert_ne!(bright, dim, "{}: o brilho não altera a imagem", t.id);
            }
        }
    }

    #[test]
    fn catalog_json_exposes_shader_params_with_ranges() {
        let v: Vec<serde_json::Value> = serde_json::from_str(&catalog_json()).unwrap();
        let aurora = v.iter().find(|e| e["id"] == "aurora").expect("aurora");
        let p = aurora["params"].as_array().expect("params");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0]["name"], "intensity");
        assert!(p[0]["description"].as_str().is_some_and(|d| !d.is_empty()));
        let web = v.iter().find(|e| e["kind"] == "web").expect("web");
        assert!(web.get("params").is_none());
    }
}

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn docs_catalog_lists_every_builtin_wallpaper() {
        let doc = include_str!("../../docs/catalog.md");
        for e in CATALOG {
            assert!(doc.contains(&format!("| {} |", e.id)), "docs/catalog.md sem ficha para {}", e.id);
        }
    }
}
