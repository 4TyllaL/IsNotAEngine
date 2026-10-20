//! Parâmetros de usuário declarados no cabeçalho do shader.
//!
//! Formato (uma linha por parâmetro, nas primeiras 40 linhas, no máximo 8):
//! `// param: nome min max padrão` ou, com descrição para a interface,
//! `// param: nome min max padrão -- texto curto` (até 80 caracteres).
//! O i-ésimo parâmetro declarado chega ao shader como `P(i)` / `params[i / 4]`.
//! Linhas malformadas são ignoradas com aviso; valores são sempre limitados à faixa.

pub const MAX_PARAMS: usize = 8;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ParamSpec {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Texto curto mostrado ao lado do controle (opcional).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl ParamSpec {
    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_finite() { v.clamp(self.min, self.max) } else { self.default }
    }
}

/// Lê as declarações `// param:`. Devolve as válidas e as mensagens das inválidas.
pub fn parse(code: &str) -> (Vec<ParamSpec>, Vec<String>) {
    let mut specs = Vec::new();
    let mut problems = Vec::new();
    for (n, line) in code.lines().take(40).enumerate() {
        let Some(rest) = line.trim().strip_prefix("//") else { continue };
        let Some(args) = rest.trim().strip_prefix("param:") else { continue };
        let (args, description) = match args.split_once("--") {
            Some((a, d)) => (a, d.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>()),
            None => (args, String::new()),
        };
        let parts: Vec<&str> = args.split_whitespace().collect();
        let nums: Option<Vec<f32>> = parts.iter().skip(1).map(|v| v.parse::<f32>().ok()).collect();
        let valid_name = parts
            .first()
            .is_some_and(|s| s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        match (valid_name, nums.as_deref()) {
            (true, Some(&[min, max, default]))
                if min.is_finite() && max.is_finite() && default.is_finite() && min <= max =>
            {
                if specs.len() == MAX_PARAMS {
                    problems.push(format!("linha {}: mais de {MAX_PARAMS} parâmetros, ignorado", n + 1));
                    continue;
                }
                specs.push(ParamSpec { name: parts[0].to_string(), min, max, default: default.clamp(min, max), description });
            }
            _ => problems.push(format!("linha {}: esperado `// param: nome min max padrão`", n + 1)),
        }
    }
    (specs, problems)
}

/// Valores finais: padrões, com `overrides` (por NOME) aplicados e sempre limitados à faixa de cada
/// parâmetro. Nomes desconhecidos são ignorados; NaN/infinito volta ao padrão.
pub fn resolve(specs: &[ParamSpec], overrides: &[(String, f32)]) -> [f32; MAX_PARAMS] {
    let mut values = defaults(specs);
    for (name, v) in overrides {
        if let Some(i) = specs.iter().position(|s| &s.name == name) {
            values[i] = specs[i].clamp(*v);
        }
    }
    values
}

/// Nome de parâmetro válido (o mesmo que `parse` aceita).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `nome:valor,nome:valor` (query da prévia) em pares; ignora entradas inválidas, no máximo 8.
pub fn parse_overrides(s: &str) -> Vec<(String, f32)> {
    s.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            let v: f32 = v.trim().parse().ok()?;
            (valid_name(k.trim()) && v.is_finite()).then(|| (k.trim().to_string(), v))
        })
        .take(MAX_PARAMS)
        .collect()
}

/// `// retint: #rrggbb` (primeiras 40 linhas): cor que, no visual padrão do shader, representa o "destaque".
/// Devolve `[ângulo YIQ, 1.0]` para o constant buffer, ou `[0, 0]` quando a diretiva não existe.
pub fn retint_ref(code: &str) -> [f32; 2] {
    for line in code.lines().take(40) {
        let Some(rest) = line.trim().strip_prefix("//") else { continue };
        let Some(arg) = rest.trim().strip_prefix("retint:") else { continue };
        let c = crate::engine::parse_hex_color(arg.trim());
        if c[3] > 0.0 {
            let i = 0.596 * c[0] - 0.274 * c[1] - 0.322 * c[2];
            let q = 0.211 * c[0] - 0.523 * c[1] + 0.312 * c[2];
            return [q.atan2(i), 1.0];
        }
    }
    [0.0, 0.0]
}

/// Valores padrão no layout do constant buffer.
pub fn defaults(specs: &[ParamSpec]) -> [f32; MAX_PARAMS] {
    let mut out = [0.0; MAX_PARAMS];
    for (o, s) in out.iter_mut().zip(specs) {
        *o = s.default;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retint_directive() {
        assert_eq!(retint_ref("float4 ps_main() {}"), [0.0, 0.0]);
        let r = retint_ref("// x
// retint: #ff0000
");
        assert_eq!(r[1], 1.0);
        assert!((r[0] - (0.211f32).atan2(0.596)).abs() < 1e-4);
        assert_eq!(retint_ref("// retint: nada")[1], 0.0);
    }

    #[test]
    fn parses_and_validates() {
        let src = "// param: scale 0.5 4 9\n// param: bad 1 0 0.5\n// param: x y z w\nfloat4 ps_main() : SV_Target { return 0; }";
        let (specs, problems) = parse(src);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].default, 4.0);
        assert_eq!(problems.len(), 2);
        assert_eq!(specs[0].clamp(f32::NAN), 4.0);
        assert_eq!(defaults(&specs)[0], 4.0);
    }

    #[test]
    fn description_is_optional_and_trimmed() {
        let src = "// param: a 0 1 0.5 -- Brilho geral\n// param: b 0 1 0.5\n// param: c 0 1 0.5 --   \n";
        let (specs, problems) = parse(src);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(specs[0].description, "Brilho geral");
        assert_eq!(specs[1].description, "");
        assert_eq!(specs[2].description, "");
        let long = format!("// param: a 0 1 0.5 -- {}\n", "x".repeat(200));
        assert_eq!(parse(&long).0[0].description.chars().count(), 80);
    }

    #[test]
    fn resolve_clamps_ignores_unknown_and_rejects_non_finite() {
        let specs = parse("// param: k 0 2 1\n// param: m 1 5 3\n").0;
        let v = resolve(&specs, &[("k".into(), 9.0), ("m".into(), f32::NAN), ("x".into(), 1.0)]);
        assert_eq!(&v[..2], &[2.0, 3.0]);
        assert_eq!(resolve(&specs, &[("k".into(), -5.0)])[0], 0.0);
    }

    #[test]
    fn parse_overrides_filters_garbage() {
        assert_eq!(parse_overrides("a:1.5, b:2"), vec![("a".into(), 1.5), ("b".into(), 2.0)]);
        assert!(parse_overrides("a:x,:3,../:1,b:inf,c:NaN").is_empty());
        assert_eq!(parse_overrides(&(0..20).map(|i| format!("p{i}:1")).collect::<Vec<_>>().join(",")).len(), MAX_PARAMS);
    }
}
