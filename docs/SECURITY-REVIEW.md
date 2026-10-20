# Análise de segurança do IsNotAEngine

Data: 2026-10-16. Versão: 0.1.0 (commit posterior a "remove wallpaper por endereço da internet").
Autor: revisão de código feita por IA a pedido do mantenedor. **Não é um teste de invasão nem uma auditoria
independente**: é uma leitura dirigida do código, com correções, testes e a lista do que sobra.

## Resumo

- Superfície de rede do app: **só o Criar com IA** (Claude, Gemini, OpenRouter), com a chave do próprio usuário.
  Wallpaper por endereço da internet foi **removido** nesta revisão.
- Os riscos mais relevantes que sobram vêm de **conteúdo importado pelo usuário** (um `.html` ou `.livewallpaper`
  baixado por aí roda código) e do fato de o app ser **sem assinatura** por enquanto.
- Nesta revisão foram corrigidos 1 problema médio, 1 de endurecimento médio e 4 baixos (tabela abaixo).
- Ainda **não** foi feito: teste de invasão, fuzzing de importadores, revisão do instalador em máquina limpa,
  `cargo audit` local (o CI roda a cada push e na release).

## O que está em jogo (ativos) e em quem confiamos

| Ativo | Onde fica |
| --- | --- |
| Chave de API da IA | Gerenciador de Credenciais do Windows (nunca no `config.toml`, no log nem na página) |
| Arquivos e dados do usuário | fora do app; o app só lê/escreve em `%LOCALAPPDATA%\IsNotAEngine` e onde o usuário manda |
| O desktop do usuário | o app desenha atrás dos ícones e pode trocar o wallpaper e a tela de bloqueio do Windows |
| Dinheiro (cota da IA) | limitado por rodadas e tokens; o teto em dinheiro é do provedor |

Confiamos em: o próprio usuário e o Windows. **Não** confiamos em: arquivos importados (HLSL, HTML, pacotes,
vídeo), respostas da IA, e qualquer página dentro de um WebView.

## Fronteiras de confiança e como cada uma é protegida

| Fronteira | Proteção hoje |
| --- | --- |
| Wallpaper web (embutido ou importado) → resto do app | Protocolo `wallpaper://` com CSP sem rede (`connect-src 'none'`), navegação presa à própria origem, janelas novas, permissões do navegador (câmera, microfone, localização) e downloads **negados** |
| Preview de wallpaper importado dentro do Studio | `iframe` com `sandbox="allow-scripts"` (origem opaca), com a mesma CSP sem rede |
| Studio (UI) → motor (IPC) | Só aceita mensagens do **documento** do Studio (`/` ou `/index.html`); validação no Rust de cada ação; remoção confinada à pasta de importados e mandada para a Lixeira |
| Studio ← iframes | O Studio só obedece `postMessage` do iframe do **Lab** (`e.source`) |
| Páginas do Studio → rede | CSP sem rede e sem recursos de fora (`default-src 'none'`, `object-src 'none'`, `base-uri 'none'`, `form-action 'none'`) |
| Pacote `.livewallpaper` (zip) | Limites no manifesto e nas entradas, extração em pasta temporária e só depois movida, nomes presos à pasta de destino |
| HLSL de terceiros (importado, Shadertoy, IA) | Compilado num **processo filho** (`--compile-worker`) com tempo limite e fila (processo principal não trava nem cai com um shader hostil) |
| Canal de controle (named pipe) | DACL só dono e SYSTEM, clientes remotos recusados, linha de requisição limitada, validação de cada comando |
| Servidor MCP (`--mcp`) | stdio do processo que o iniciou; só grava na pasta de importados, com nome higienizado |
| Chave da IA | Credential Manager; não sai do processo; os testes cobrem que o erro do provedor não a expõe; sem redirecionamento HTTP |
| Compilador do app | Rust seguro na lógica; `unsafe` só em chamadas Win32/COM (cerca de 170 ocorrências, todas em FFI) |

## Achados desta revisão

| # | Severidade | Achado | Situação |
| --- | --- | --- | --- |
| F1 | Média | O Studio aceitava `postMessage` de **qualquer** frame. Um wallpaper importado em preview (iframe com sandbox) podia mandar `{lab:'apply', path:...}` e fazer o Studio aplicar no desktop **qualquer arquivo local** como wallpaper (e gravar valores de parâmetros). Reproduzido numa página de teste | **Corrigido**: só o iframe do Lab é obedecido. Verificado: o iframe com sandbox gera 0 comandos, o Lab gera 1 |
| F2 | Média (endurecimento) | O IPC aceitava qualquer documento na origem `studio`, incluindo os iframes de preview e o Lab. Se o WebView2 informar a URL do frame (não verificado), uma página importada poderia chamar comandos do motor (aplicar, importar, limpar dados, **gastar cota da IA com a chave do usuário**) | **Corrigido**: só o documento `/` ou `/index.html`. Teste automático. Não confirmei se o WebView2 realmente reportaria o frame; a regra vale de qualquer forma |
| F3 | Média | Wallpaper por endereço da internet: conteúdo remoto, com rede, ao fundo do desktop, e prévia no Studio | **Removido** (motor, Studio, pipe, permissões por site). Config antiga com URL cai no wallpaper padrão; o Studio não pré-visualiza mais endereços |
| F4 | Baixa | A interface do Studio não tinha CSP | **Corrigido**: CSP sem rede nem recursos de fora. Testado só numa página estática (sem violações); **falta conferir no app real** |
| F5 | Baixa | Títulos entravam em `innerHTML` sem escape (hoje não explorável, porque nomes de arquivo do Windows não aceitam `<` nem `>`) | **Corrigido**: escape nos pontos em que o título vira HTML |
| F6 | Baixa | Cliente HTTP da IA seguiria redirecionamentos levando o cabeçalho com a chave para outro host | **Corrigido**: sem redirecionamentos (os provedores respondem direto) |

## Riscos que continuam (e por quê)

| # | Risco | Avaliação | O que fazer |
| --- | --- | --- | --- |
| R1 | **Wallpaper web importado é código**. Ele não tem rede (CSP) nem permissões do navegador e usa WebGL e CPU/GPU à vontade. A CSP não cobre todo canal lateral (por exemplo, resolução de DNS por `dns-prefetch`, não verificado). **Mitigado nesta revisão**: importar `.html`, pasta ou `.livewallpaper` pede confirmação no Studio ("roda código; importe só de quem você confia") e páginas importadas **não recebem** a música em reprodução nem as métricas do sistema (só o espectro de áudio) | Baixa-média. O que sobra é o espectro de áudio e o canal lateral teórico | Confirmar no app real; avaliar um interruptor por wallpaper se alguém precisar de métricas numa página própria |
| R2 | **HLSL hostil**: o compilador roda isolado, mas um shader que compila e **trava a GPU** em tempo de execução pode causar reset do driver (TDR) | Baixa-média. Há recuperação de device lost; a tela pode piscar | Tempo máximo por quadro/verificação de custo antes de aplicar; aceitar como limite conhecido |
| R3 | **Sem assinatura de código**: o Windows (SmartScreen) avisa; um binário trocado no caminho de distribuição não seria detectado | Média para distribuição | Assinar (Microsoft Store assina o MSIX; fora da Store, certificado) e publicar `SHA256SUMS` (o workflow já gera) |
| R4 | A chave da IA no Credential Manager fica legível por **qualquer processo do mesmo usuário** (padrão do Windows) | Baixa; quem já roda como você já tem acesso a mais coisas | Documentar; oferecer remover a chave nos Ajustes (já existe) |
| R5 | Pipe de controle e MCP aceitam qualquer processo **do mesmo usuário** | Baixa (mesma conta) | Manter DACL; não expor em rede |
| R6 | Atualizador **não existe** (só o projeto em `internal/updater-design.md`): bom para a superfície, ruim para corrigir falhas rápido | Informativo | Quando existir, manifesto assinado e lista fechada de hosts, como no projeto |
| R7 | DLLs do sistema carregadas por nome (por exemplo, `uxtheme.dll`) | **Corrigido na 1.0.0**: `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` no início do `main`. Verificado: o app abre, o WebView2 sobe e a renderização funciona | Nada |
| R8 | Dependências. `cargo audit` rodado em 2026-10-16: 4 avisos de severidade baixa em `rustls` 0.23.45 (usado pelo cliente HTTP da IA), **corrigidos** com a atualização para 0.23.46. Sobram 2 avisos sem correção que não afetam o app no Windows: `proc-macro-error` (não mantido, só em tempo de compilação) e `glib` (só no Linux, via GTK do wry) | Baixa | Rodar `cargo audit` antes de cada release (o gate do workflow já faz) |
| R9 | Instância única por mutex nomeado: outro processo do mesmo usuário pode "segurar" o nome e impedir o app de abrir | Baixa (negação de serviço local) | Aceitar |

## O que foi lido e o que não foi

Lido com atenção: importação e confinamento de arquivos (`import.rs`), servidor de protocolo e IPC do Studio
(`studio/window.rs`), páginas `studio.html` e `lab.html` (uso de `innerHTML`, mensagens entre frames),
`wallpaper/web.rs` e `manager.rs` (WebView2, navegação, CSP), cliente HTTP e chaves da IA (`ai/`), cliente e
worker de compilação, pipe de controle e MCP (rotas de escrita), uso de `ShellExecute`/`Command`.

**Não coberto**: fuzzing do zip, do vídeo (Media Foundation) e do decodificador de imagens; revisão da
`desktop/` (host na camada do Progman) além do que toca segurança; o instalador `.iss` em máquina limpa; teste do
app real sob AppContainer/MSIX; modelagem de ameaça do WebView2 (versão, flags); análise estática automatizada
(`cargo clippy` estrito e testes passam, mas isso não é auditoria).

## Antes de publicar (recomendado)

1. Conferir a CSP do Studio e o IPC **no app real** (abrir cada aba e importar um `.html` de teste que tente
   `window.chrome.webview.postMessage`).
2. Aviso ao importar HTML/pacote (R1) e decidir sobre música/métricas para páginas importadas.
3. ~~Rodar `cargo audit`~~ (feito, veja R8).
4. Assinar o executável ou o MSIX (R3) e publicar os `SHA256SUMS`.
5. ~~`SetDefaultDllDirectories` (R7)~~ (feito).
6. Uma segunda revisão **humana** independente da superfície de importação (zip, HTML, HLSL).
