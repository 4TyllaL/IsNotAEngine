# Fluxo recomendado: criar um wallpaper com o MCP

1. **Entenda o pedido** e escolha o formato:
   - **Shader** (`psl_render`): animação procedural na GPU. Ótimo para gradientes, fluidos, ondas, fractais, campos de estrelas, "vidro", neon. Custo baixo.
   - **Web** (`web_preview`): relógios, dashboards, texto, qualquer coisa que dependa de DOM/Canvas/SVG e dos dados do sistema (`window.engine`).
   - **Imagem estática** (`render_still`): um quadro de um shader em alta resolução (até 7680x4320). Fica igual na tela e não renderiza a cada quadro, então tende a custar muito menos (ainda não medido): bom para bateria ou quando a animação não importa.
2. **Leia o guia** (`get_guide`, tópico `shader` ou `web`) antes de escrever.
3. **Escreva e renderize**. Para shaders, GLSL no formato do Shadertoy (`mainImage`) costuma sair melhor que HLSL do zero: `language: "glsl"`.
4. **Itere olhando o resultado**:
   - Peça instantes bem espaçados (`times: [1, 8, 20]`) para julgar a animação, não só um quadro.
   - Leia os *hints*: imagem escura demais, estourada, achatada, sem movimento, ou cara.
   - O alvo é ≤ 4 ms de GPU a 1080p (o wallpaper roda em cada monitor, o tempo todo).
   - Corrija **uma coisa por vez** e renderize de novo. Em geral 3 a 6 voltas bastam.
5. **Salve** com `save_wallpaper` (ou `render_still` para imagem) e **aplique** com `apply_wallpaper` passando o caminho devolvido. O app precisa estar aberto para aplicar.

## Dicas de aparência
- Fundos escuros e sóbrios deixam os ícones do desktop legíveis; evite brilho médio acima de ~0,6.
- Comprima os realces (`col / (1.0 + col)`) em vez de deixar estourar.
- Movimento lento é melhor que rápido: o wallpaper fica na tela o dia inteiro.
- Funcione em qualquer proporção (16:9, 21:9, retrato): derive coordenadas da altura (`(U - 0.5*iResolution.xy) / iResolution.y`).
- Sem texturas nem buffers: tudo é matemática dentro de `mainImage`.

## Segurança
Shaders rodam na GPU do usuário e não há como abortar um desenho longo: mantenha todo laço com limite fixo e pequeno. Wallpapers web rodam sem rede (CSP).
