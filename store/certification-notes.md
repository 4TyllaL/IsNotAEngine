# Notas para a certificação

Texto sugerido para o campo "Notas para a certificação" do Partner Center (em inglês, que é o que o revisor lê):

> IsNotAEngine is a full-trust Win32 app (runFullTrust) that renders animated wallpapers on the desktop layer
> behind the icons (a child window of the Progman/WorkerW window, DirectComposition). That is why it needs
> the restricted capability runFullTrust.
>
> How to test: launch the app; the Studio opens. Pick a wallpaper in the gallery and click Apply. The tray icon
> (click the ^ arrow if hidden) opens a menu to pause or change wallpaper. "Iniciar com o Windows" in Settings
> uses the package StartupTask (disabled by default; the user turns it on).
>
> Network: none, except the optional "Criar com IA" tab, which calls the Anthropic/Google/OpenRouter APIs with an
> API key the user types. No key is provided or needed to review the app. The key is stored in Windows Credential Manager.
>
> Imported web wallpapers run in WebView2 with a Content-Security-Policy that blocks network access.
> WASAPI loopback is used only to read the level of the audio playing on the PC (no microphone, nothing recorded).

## Conferir antes de enviar

- [x] Nome reservado (`AtyllaAzevedo.IsNotAEngine`, Store ID 9PCM2JPNSFTZ); o pacote já usa a identidade do Partner Center.
- [x] Capturas de tela: `store/screenshots/studio-01` a `studio-06` (1600x900, PNG) mostram o Studio atual: galeria com ajustes, vídeo de fábrica, catálogo em grade, Criar, Rotação e Motor. Foram geradas com a interface real do Studio fora da janela do app (as prévias dos shaders são renderizadas pelo próprio motor e os vídeos são os de fábrica); se quiser capturas da janela do app, substitua. `01` a `06` são renders 1920x1080 dos wallpapers.
- [x] Logotipos da Store (`store/assets/`): pôster 9:16 `poster-720x1080.png` e `poster-1440x2160.png`; arte de caixa 1:1 `box-1080.png` e `box-2160.png`; ícone do bloco `icon-300.png`, `icon-150.png` e `icon-71.png`. (`icon-1080.png` é o ícone sem texto.) Para a listagem em inglês: `poster-en-720x1080.png`, `poster-en-1440x2160.png`, `box-en-1080.png` e `box-en-2160.png` (mesmo desenho, texto "Animated wallpapers for Windows"). O Studio só tem interface em português; as capturas `studio-*` servem para as duas listagens.
- [x] Política de privacidade publicada: https://4tyllal.github.io/isnotaengine/privacy/ (texto-fonte em `docs/PRIVACY.md`).
- [ ] Classificação etária (questionário IARC): sem violência, sem conteúdo adulto, sem compras. O recurso de IA gera código a partir de texto do usuário: responder com honestidade.
- [ ] Preço: R$ 10,95 (confirmar que o portal aceita esse valor; considerar um período de teste grátis). Categoria: Personalização.
- [ ] Por ser pago, o **perfil fiscal** (Brasil) e o **perfil de pagamento** (Microsoft Corporation, "Sell") precisam estar "Concluído" antes do envio: Configurações da conta > Perfis fiscais e Perfis de pagamento.
- [ ] Testar o pacote **instalado** (não só a pasta de build), incluindo "Iniciar com o Windows" (StartupTask): **ainda não testado empacotado**.
- [x] WACK rodado em 2026-10-17 no pacote final 1.0.0.0 (com os vídeos de fábrica): OVERALL_RESULT=PASS, sem FAIL nem WARN (Windows 11 26200, kit 10.0.26100.8249). Instalado com `packaging/msix/test-install.ps1` (pacote assinado com certificado de teste; o enviado à Store continua sem assinatura).
