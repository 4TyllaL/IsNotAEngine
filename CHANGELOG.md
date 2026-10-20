# Changelog

## 1.0.0 — 2026-10-16
Primeira versão pública.

- Um wallpaper independente por monitor (shaders, vídeo, páginas HTML/WebGL), com pausa por monitor.
- Studio: galeria, ajustes, playlist, editor de cores, tour inicial e Criar com IA (chave do usuário).
- Importação de shaders `.hlsl`, vídeos, imagens, páginas e pacotes `.livewallpaper`, com confirmação para páginas e pacotes.
- Modo desempenho (automático em jogos e apresentações), pausa por cobertura e bateria.
- Ãcone e menu na bandeja (recriado se o Explorer reiniciar), console de logs opcional.
- Instalador (Inno Setup) e pacote MSIX para a Microsoft Store (com StartupTask para iniciar com o Windows).
- Segurança: DLLs só do System32, `rustls` 0.23.46 (4 avisos do `cargo audit` corrigidos), sem wallpapers por endereço da internet, CSP sem rede nas páginas, análise em `docs/SECURITY-REVIEW.md`.
- Ambient: Fumaça e Gyroid · Seda ganharam ajustes (velocidade, brilho, calor/cores, densidade/névoa); o Gyroid foi reescrito em HLSL nativo, sem as faixas de cor. Tokyo Rain e Winter Cabin saíram do catálogo.
- Vídeos de fábrica: vídeos na pasta `videos` do app (ou `assets/wallpapers/videos` no desenvolvimento) entram na galeria e na bandeja com id estável `video:<nome>`; instalador e MSIX levam a pasta.
- Vídeos em loop: o fim do vídeo se mistura com o começo (fade de 1,5 s) em vez de cortar, e um loop que travava depois de 2-3 repetições foi corrigido (rebobinar inválido; agora reabre o leitor se falhar).
