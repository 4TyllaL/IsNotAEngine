# Guia de wallpapers web (IsNotAEngine)

Um wallpaper web é uma página HTML/CSS/JS (Canvas 2D, WebGL, SVG, CSS) renderizada pelo WebView2 na camada do desktop, atrás dos ícones. Um arquivo `.html` único é o formato mais simples.

## Regras da página
- **Um arquivo só**: coloque CSS e JS inline (`<style>`, `<script>`). Imagens e fontes só como `data:` URI.
- **Sem rede**: a política de segurança é `default-src 'self' data: blob:`, `connect-src 'none'`, `frame-src 'none'`. `fetch`, `XMLHttpRequest`, CDNs, Google Fonts e `<iframe>` são bloqueados.
- Sem interação de teclado nem de mouse: a página fica atrás dos ícones e não recebe eventos do navegador. O desktop não rola: `html, body { margin: 0; height: 100%; overflow: hidden; }`.
- Preencha a tela inteira em qualquer proporção (16:9, 21:9, 16:10, retrato). Use `canvas { position: fixed; inset: 0; width: 100%; height: 100% }` e redimensione pelo `devicePixelRatio`.
- `requestAnimationFrame` é limitado ao FPS configurado no Studio (`engine.fps`). Para relógios e coisas lentas, redesenhe uma vez por segundo com `setTimeout` alinhado à virada do segundo: custo de CPU quase zero.
- O wallpaper roda o tempo todo, em cada monitor: gaste pouco. Evite `shadowBlur` grande a 60 fps, `filter: blur` em tela cheia e milhares de elementos DOM.

## Ponte `window.engine`
A página recebe, antes de qualquer script:

| Campo | O que é |
|---|---|
| `engine.audio` | `{bass, mid, treble, volume, spectrum[16]}`, de 0 a 1, o áudio que o Windows está tocando |
| `engine.onAudio(cb)` | chamado a cada ~33 ms com o objeto acima |
| `engine.media` / `engine.onMedia(cb)` | música tocando agora (título, artista, capa) ou `null`. **Só wallpapers de fábrica**: páginas importadas sempre recebem `null` |
| `engine.system` / `engine.onSystem(cb)` | `cpu`, `cores`, `mem`, `disk`, `net`, `uptime_secs`, atualizado a cada 1 s. **Só wallpapers de fábrica**: páginas importadas ficam com `null` |
| `engine.tint` / `engine.onTint(cb)` | cor de destaque escolhida no Studio (`{r,g,b}`) ou `null` (automático) |
| `engine.fps` | FPS configurado |

Sempre trate a ausência (`engine` pode não existir fora do motor): `const eng = window.engine || {};`.

## Cor de destaque automática
Se a página tiver `<meta name="retint" content="#89b4fa">` (a cor dominante do desenho), o motor gira o matiz da página inteira até a cor de destaque que o usuário escolheu. Escolha uma cor de referência que represente o desenho.

## Dicas de qualidade
- Fundo escuro e sóbrio funciona melhor: os ícones do desktop ficam legíveis.
- Teste sempre em duas proporções (use `web_preview` com larguras diferentes).
- Para parâmetros de desenvolvimento, `?t=HH:MM:SS` e coisas do tipo ficam bem como `URLSearchParams`, nunca como obrigatórios.
