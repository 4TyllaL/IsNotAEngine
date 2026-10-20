# Contrato de shader (IsNotAEngine)

**API:** Direct3D 11 (feature level 11_0). HLSL compilado em runtime com FXC, `ps_5_0` / `vs_5_0`.

## Estágios e entradas
- Apenas pixel shader de tela cheia: `float4 ps_main(float4 pos : SV_Position) : SV_Target` (ou `f_main`).
- `vs_main` é injetado se faltar. `#include` não é suportado; `common.hlsli` é prefixado automaticamente
  (a menos que o shader declare o próprio `cbuffer`, o que sai do contrato).
- Sem texturas nem buffers extras. Um único `cbuffer Constants : register(b0)`.

## Constantes (ver `common.hlsli`)
`resolution`, `time` (+`speed`; use `T()`), `tint`/`accent()`, `mouse`, `mouse_click`, `parallax()`, `mouse_p()`/`mouse_down()`/`cursor_field()`/`cursor_swirl()`/`cursor_lens()` (efeitos do cursor; zerados quando a interação está desligada),
`clock_time`, `clock_date`, `audio_reactive`, `audio_spectrum` e `params` (lido com `P(i)`).
Coordenadas: `frag_uv()` (0..1, y para cima) e `frag_p()` (centrado, altura = 1, aspecto corrigido).
Shaders devem funcionar em qualquer proporção (16:9, 21:9, 16:10, retrato).

## Parâmetros de usuário
Até 8, nas 40 primeiras linhas: `// param: nome min max padrão [-- descrição]`. O i-ésimo declarado é `P(i)`.
A descrição (até 80 caracteres, opcional) aparece ao lado do controle no Studio.
Valores são sempre limitados a [min, max]; NaN/infinito volta ao padrão; linhas malformadas são ignoradas com aviso.

**Ajustar sem editar código:** no Studio, o wallpaper selecionado na Galeria mostra um controle por parâmetro
(painel "Ajustes"); no Lab, os mesmos controles aparecem abaixo da prévia. Os valores ficam no `config.toml`
(`[shader_params."<wallpaper>"]`, por nome do parâmetro) e a prévia e o desktop usam o **mesmo cálculo**
(limitar à faixa declarada). Parâmetro desconhecido é ignorado; no máximo 8 por wallpaper e 64 wallpapers.
"Padrões" remove os valores do usuário. No Lab, "Salvar"/"Aplicar" levam os valores atuais junto com o arquivo.

**Modelos do Lab** (menu "Começar de…" › Modelos): `ondas`, `grade_neon` e `nevoa`, em
`assets/wallpapers/templates/`, todos originais, com parâmetros descritos. Um teste compila cada modelo,
confere faixas/descrições e que a imagem renderiza algo e muda com os parâmetros.

## Pós-processamento
`// bloom: intensidade [limiar]` (opcional). Com bloom a cena é HDR (RGBA16F) e o composite comprime os realces.

## Saída e cor
Alvo `R8G8B8A8_UNORM` (sem conversão sRGB automática): escreva valores já em espaço de exibição (gama aplicada).
Alfa é ignorado (PNGs do Lab saem opacos, marcados como sRGB).

## Restrições e segurança
- Fonte ≤ 256 KiB; resolução ≤ 8K; ≤ 1200 frames por execução; compilação com timeout de 30 s (o Lab sai com código 3).
- Orçamento de referência: ver `gpu_ms` no `report.json`; mire ≤ 4 ms a 1080p em GPU de entrada.
- **Limite honesto:** não há como abortar um draw longo. Um shader com loop excessivo pode causar TDR do Windows
  (reset do driver). Sempre rode fontes não confiáveis só no Lab, nunca direto no desktop.
- Falha de compilação nunca derruba o motor: o hot-reload mantém a versão anterior.

## Lab
`IsNotAEngine.exe --lab render|seq|watch <arquivo.hlsl> [--size WxH] [--times a,b] [--frames N --fps N --start S] [--param k=v] [--audio] [--out DIR]`
Cada execução grava `lab/runs/<shader>/<data-hora>/` com PNGs, cópia da fonte e `report.json`
(diagnósticos, tempo de compilação sem cache, GPU/CPU por frame, estatísticas de imagem, memória).
Relógio, mouse e áudio são fixos (silêncio, salvo `--audio`), então a mesma fonte gera os mesmos frames.
Códigos de saída: 0 ok · 1 uso · 2 compilação · 3 timeout · 4 renderização.

## Importar do Shadertoy
Studio → "Colar do Shadertoy" (ou arrastar um `.glsl`/`.frag`). O GLSL é convertido para HLSL e importado como um
`.hlsl` comum (o arquivo gerado declara o próprio `cbuffer`, com prefixo `st_`, e desliga o bloom para a cor sair como no site).
- **Suportado:** `mainImage`, `iResolution`, `iTime`, `iTimeDelta`, `iFrame`, `iMouse`, `iDate`, `gl_FragCoord`, tipos `vec/ivec/uvec/bvec/mat`,
  `mix`/`fract`/`mod`/`sign`/`atan(y,x)`, constantes globais, arrays com construtor, construtores com um argumento (`vec4(1.0)`, `mat3(1.0)`).
- **Matrizes:** o HLSL preenche por linha e o GLSL por coluna, então `M * v` e `v * M` têm os operandos trocados na conversão
  (verificado na GPU). Cadeias como `a * b * M` com `a` vetor podem sair erradas: confira o resultado (a importação avisa no log).
- **Não suportado (a importação recusa com a lista):** `iChannel0..3`/texturas, buffers (multipass), `inverse`, `mainSound`.
- Respeite a licença do autor (padrão do Shadertoy: CC BY-NC-SA 3.0); o cabeçalho gerado pede para manter os créditos.
