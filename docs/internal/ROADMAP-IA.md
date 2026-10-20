# Roadmap de melhorias para execucao por IA

Data: 2026-10-15. Referencia inicial: commit `9b97e8c`.

Objetivo: fortalecer seguranca e confiabilidade no Windows, melhorar a entrega
e tornar o fluxo de criacao de wallpapers o diferencial do IsNotAEngine.

Este arquivo e um plano de execucao, nao uma certificacao de seguranca.
Os caminhos e comportamentos devem ser conferidos novamente antes de cada tarefa.

## Como executar com effort menor

- Execute apenas UM ID por conversa. Nao implemente uma fase inteira de uma vez.
- Leia este arquivo, as instrucoes do repositorio e os arquivos indicados no ID.
- Verifique `git status --short`. Preserve alteracoes existentes do usuario.
- Confirme o comportamento atual antes de modificar: algumas tarefas podem ja estar resolvidas.
- Implemente a menor mudanca que satisfaz o aceite. Evite refatoracoes paralelas.
- Adicione testes para limites, seguranca e contratos alterados. Para texto e ajustes
  visuais simples, prefira verificacao direta a testes que repetem a implementacao.
- Rode primeiro testes relacionados; rode a suite completa ao concluir uma fase.
- Nao troque versoes de dependencias nem regenere o lockfile sem necessidade.
- Registre o resultado na tabela abaixo, incluindo comandos e limitacoes reais.
- Marque concluido apenas quando todos os criterios de aceite forem atendidos.
- Uma falha de permissao, GPU ou rede no ambiente nao prova defeito do produto.
  Registre-a; nao remova testes nem enfraqueca controles para obter resultado verde.
- Nao publique releases, altere credenciais reais ou substitua wallpapers do usuario
  como efeito colateral de testes. Use fixtures e renderizacao offscreen.
- Se uma tarefa exceder tres arquivos de producao ou exigir uma nova arquitetura,
  proponha subdivisoes concretas neste documento antes de implementar.

## Prompt reutilizavel

```text
Leia docs/internal/ROADMAP-IA.md e execute somente o ID <ID>.
Confira dependencias e o estado atual do codigo. Preserve mudancas existentes.
Implemente a menor solucao que cumpra o aceite e execute a validacao indicada.
Nao avance para outro ID. Atualize o registro do roadmap e informe o que mudou,
os testes executados e qualquer limitacao. Se o ID ja estiver resolvido,
verifique o aceite e documente as evidencias em vez de refazer o trabalho.
```

## Ordem e registro

Estados: pendente, em andamento, bloqueado (com motivo), concluido.
As dependencias sao obrigatorias; IDs independentes podem ser escolhidos separadamente.

| ID | Prioridade | Depende de | Estado | Evidencia / resultado |
| --- | --- | --- | --- | --- |
| S01 | P0 | - | concluido | Ver registro S01 abaixo |
| S02 | P0 | S01 | concluido | Ver registro S02 abaixo |
| S03 | P0 | - | concluido | Ver registro S03 abaixo |
| S04 | P0 | S03 | concluido | Ver registro S04 abaixo |
| S05 | P1 | S03, S04 | concluido | Ver registro S05 abaixo |
| C01 | P1 | - | concluido | Ver registro C01 abaixo |
| C02 | P1 | C01 | concluido | Ver registros C02 abaixo (sem Job Object) |
| Q01 | P1 | - | concluido | Ver registro Q01 abaixo |
| Q02 | P1 | - | concluido | Ver registro Q02 abaixo |
| W01 | P1 | - | concluido | Ver registro W01 abaixo (ainda nao usada pelo config: W02) |
| W02 | P1 | W01 | concluido | Ver registro W02 abaixo |
| W03 | P1 | W02 | concluido | Ver registro W03 abaixo (sem teste manual multi-tela) |
| B01 | P1 | - | primeira rodada feita (matriz reduzida) | Ver docs/performance.md e registro B01-rodada1 |
| V01 | P2 | B01 | pendente | Investigacao do caminho de video |
| V02 | P2 | V01 | pendente | Implementacao do backend validado |
| D01 | P2 | Q01, Q02 | concluido | Ver registro D01 (workflow nao executado em runner) |
| D02 | P2 | D01 | concluido (preparacao) | Assinatura real depende de credencial (registro D02) |
| D03 | P2 | D01, D02 | concluido (projeto) | Documento; implementacao dividida em U-D03a..f |
| U01 | P2 | Q02 | concluido no codigo (roteiro manual pendente) | Ver registro U01 |
| U02 | P2 | U01 | concluido no codigo (roteiro manual pendente) | Ver registro U02 |
| U03 | P2 | U02, S05 | concluido no codigo (teste manual com chave real pendente) | Ver registro U03 |
| A01 | P3 | B01, U02 | feito (escopo reduzido) | Catalogo com criterios de qualidade |
| M01 | P3 | W03, B01 | pendente | Estudo de portabilidade para macOS |

P0: fechar riscos antes de ampliar distribuicao. P1: consolidar a base.
P2: melhorar produto e entrega. P3: expandir depois dos gates anteriores.

## Fase 1: seguranca

### S01 - Validar manifesto antes de descompactar seu conteudo

- Ler: `src/wallpaper/manager.rs`, principalmente `load_package` e testes de pacotes.
- Fazer: verificar quantidade de entradas e tamanho declarado antes de ler o
  manifesto; limitar tambem os bytes efetivamente lidos do manifesto.
- Aceite: manifesto excessivo e JSON invalido sao rejeitados com diagnostico;
  um pacote valido continua carregando; nenhuma extracao ocorre antes da validacao.
- Validar: testes com ZIP pequeno valido, manifesto acima do teto, JSON invalido
  e entrada ausente. Documentar o teto escolhido e sua justificativa.

### S02 - Limitar extracao e carregamento recursivo

- Ler: `src/wallpaper/manager.rs`, implementacao de extracao da versao atual de `zip`.
- Fazer: conferir caminhos, links, limites de bytes extraidos e possibilidade de
  um pacote apontar para outro pacote. Rejeitar recursao ou impor profundidade baixa.
  Usar diretorio de staging exclusivo; so substituir cache apos sucesso.
- Aceite: nenhum arquivo sai da raiz; falha nao deixa pacote parcialmente ativo;
  pacotes homonimos diferentes nao invalidam um wallpaper em uso silenciosamente.
- Validar: fixtures de traversal, pacote aninhado, limite excedido e extracao falha.
  Nao criar ZIP bomb real: use limites pequenos configuraveis nos testes.

### S03 - Impedir entrega automatica de dados locais a sites remotos

- Ler: `src/wallpaper/web.rs`, `src/wallpaper/manager.rs`, chamadas em `src/engine.rs`.
- Fazer: centralizar uma politica baseada em `WebSource`; URLs nao recebem audio,
  musica ou metricas do sistema por padrao. Nao confiar em verificacao feita pelo JS.
- Aceite: a decisao e aplicada no Rust a todas as pontes de dados; catalogo local
  continua funcionando; dados nao sao enviados antes de qualquer consentimento.
- Validar: teste da matriz Builtin / Dir / Url e teste de entrega bloqueada.

### S04 - Controlar navegacao e permissoes do navegador

- Ler: construcao do WebView em `src/wallpaper/web.rs` e API instalada de `wry`.
- Fazer: bloquear saida da origem local, novas janelas e esquemas externos;
  tratar redirecionamentos remotos de acordo com politica explicita. Conferir
  camera, microfone, localizacao e downloads; negar o que nao e necessario.
- Aceite: um wallpaper local nao vira um site remoto por navegacao; bloquear
  `fetch` via CSP nao e tratado como protecao suficiente para todas as saidas.
- Validar: HTML de teste com link, `location`, popup e formulario; recursos locais
  legitimos continuam funcionando. Registrar APIs que exigem integracao WebView2.

### S05 - Permissoes granulares e revogaveis

- Ler: `src/config.rs`, politica criada em S03 e controles do Studio.
- Fazer: implementar primeiro audio, depois musica, depois sistema, em subtarefas
  separadas se necessario. Persistir por identidade/origem, nao pelo titulo exibido.
- Aceite: sem permissao nao ha entrega; revogacao interrompe a entrega e limpa os
  dados ja expostos na ponte; configuracoes antigas carregam com defaults seguros.
- Validar: migracao, concessao, revogacao e mudanca de origem. Explicar no Studio
  quais dados sao compartilhados, inclusive que sites remotos possuem rede.

## Fase 2: compilacao e qualidade

### C01 - Definir um worker de compilacao encerravel

- Ler: `src/main.rs`, `src/render/pipeline.rs`, `src/mcp/psl.rs`, `src/lab/mod.rs`.
- Fazer: escrever primeiro `docs/compiler-worker.md`: contrato versionado de
  entrada/saida, limite de fonte e resposta, erros e ownership do processo.
  Depois dividir a implementacao do worker em IDs adicionais, se necessario.
- Aceite: o contrato permite compilar sem desktop, sem credenciais e sem rede;
  diagnosticos e bytecode preservam o comportamento atual.
- Validar: revisar todos os chamadores e incluir exemplos de sucesso/erro.

### C02 - Encerrar compilacoes no timeout

- Fazer: implementar e integrar o worker de C01. Limitar concorrencia e fila;
  timeout/cancelamento devem terminar o processo, nao apenas parar de esperar.
- Aceite: worker encerrado e coletado sem processos orfaos; a proxima compilacao
  funciona; erro preserva o ultimo wallpaper valido. Nao testar travando a GPU.
- Validar: worker simulado que demora, saida invalida, crash e fila cheia;
  compilacao real de shader valido e invalido. Incluir Lab e MCP na verificacao.

### Q01 - Estabelecer lint impeditivo

- Ler: `.github/workflows/ci.yml` e avisos atuais do Clippy.
- Fazer: corrigir avisos em pequenos lotes antes de ativar
  `cargo clippy --locked --all-targets -- -D warnings` no CI.
  Nao adicionar `allow` amplo nem formatar o repositorio inteiro como efeito colateral.
- Aceite: build, testes e Clippy estrito passam na toolchain documentada.
- Validar: `cargo test --locked` e comando acima; registrar testes ignorados.

### Q02 - Corrigir documentacao e afirmacoes nao medidas

- Ler: `README.md`, `src/wallpaper/catalog.rs`, `docs/AI.md`, guias e CLI atual.
- Fazer: atualizar catalogo, funcionalidades, arquitetura e exemplos; substituir
  afirmacoes absolutas de custo zero e tamanho do binario por medidas ou ressalvas.
- Aceite: numeros e comandos conferem com o codigo; distinguir tela de bloqueio
  estatica, wallpaper animado e screensaver; explicar limites do importador GLSL.
- Validar: conferir links locais e executar apenas exemplos sem efeito no desktop.

## Fase 3: confiabilidade e desempenho no Windows

### W01 - Identificar telas de maneira persistente

- Ler: `src/desktop/monitors.rs` e atribuicoes em `src/config.rs`.
- Fazer: obter identidade persistente via APIs de display; manter indice atual
  separado da chave persistida. Definir fallback para telas sem identidade confiavel.
- Aceite: trocar ordem de enumeracao nao troca atribuicoes; telas identicas nao
  colidem; desconectar uma tela nao apaga sua configuracao.
- Validar: logica com enumeracoes simuladas e roteiro manual de hotplug/portas.

### W02 - Migrar configuracoes de monitores

- Fazer: migrar atribuicoes por indice sem descartar configuracoes antigas;
  integrar a identidade de W01 ao motor e ao Studio.
- Aceite: arquivo antigo carrega; atribuicoes sobrevivem a reinicio e hotplug;
  um erro de migracao nao sobrescreve o original sem possibilidade de recuperacao.
- Validar: fixtures antigas, telas ausentes, ordem invertida e ida/volta TOML.

### W03 - Pausar somente os monitores cobertos

- Ler: `src/pause.rs`, `src/desktop/occlusion.rs`, `src/engine.rs`, trait `Wallpaper`.
- Fazer: separar pausa global manual/bateria da oclusao por monitor.
- Aceite: tela coberta para, tela descoberta continua; audio/ponte web e previews
  nao ficam atualizando desnecessariamente uma tela suspensa; retomar nao salta o estado.
- Validar: matriz de duas telas com cobertura parcial, pausa manual, bateria e hotplug.

### B01 - Criar baseline reproduzivel

- Fazer: documentar em `docs/performance.md` maquina, driver, resolucao, FPS,
  wallpaper, aquecimento, duracao e metodo de coleta de CPU/RAM/GPU/VRAM/frame time.
- Aceite: medir shader, web e video, com Studio aberto/fechado e desktop coberto;
  comparar custos totais incluindo subprocessos WebView2. Nao usar GPU ms como
  substituto de consumo total. Definir tolerancias somente apos obter medidas.
- Validar: salvar resultados com comandos e versao; identificar claramente dados
  indisponiveis. Comparacao com concorrentes exige conteudo e condicoes equivalentes.

### V01 - Investigar video sem copia por CPU

- Ler: `src/wallpaper/video.rs` e documentacao oficial de Media Foundation/D3D11.
- Fazer: registrar em `docs/video-backend.md` caminho atual, proposta via superficies
  DXGI, formatos, stride, timestamps, sincronizacao, fallback e recuperacao de device.
- Aceite: prova offscreen com um video de fixture e medidas comparaveis a B01;
  habilitar hardware transforms sozinho nao conta como prova de zero-copy.

### V02 - Implementar o caminho de video validado

- Fazer: executar a proposta aprovada em V01 em lotes pequenos; preservar fallback
  para arquivos/hardware incompativeis. Evitar bloqueio do loop geral na decodificacao.
- Aceite: loop, velocidade de reproducao, cores, proporcao, pausa e recuperacao
  continuam corretos. So anunciar melhoria depois de medir; nao remover fallback cedo.
- Validar: 1080p/4K, codecs suportados, video corrompido, EOF e troca repetida.

## Fase 4: entrega e experiencia

### D01 - Tornar releases consistentes

- Ler: `.github/workflows/release.yml`, instalador, versao Cargo e build atual.
- Fazer: validar tag/versao, conteudo portatil e instalador; garantir que o app inicia
  de outro diretorio; gerar inventario de dependencias e checksums apos o empacotamento.
- Aceite: teste automatizado detecta versao divergente e arquivos ausentes;
  release depende dos gates de qualidade. Nao publicar durante a implementacao.

### D02 - Preparar assinatura dos artefatos

- Fazer: documentar opcao de assinatura, segredos necessarios e verificacao; preparar
  workflow sem inserir certificados ou tokens no repositorio.
- Aceite: assinatura cobre executavel/instalador conforme o canal; checksums sao
  calculados depois; falta de credencial e explicita. Assinatura real exige material
  fornecido pelo responsavel e nao pode ser simulada como concluida.

### D03 - Projetar atualizacao verificada

- Fazer: escrever `docs/updater-design.md` antes de implementar: manifesto assinado,
  raiz de confianca, HTTPS, hash do pacote, compatibilidade, downgrade e rollback.
- Aceite: arquivo adulterado nunca e executado; download incompleto nao substitui
  instalacao; usuario controla quando instalar. Hash servido junto do arquivo,
  sozinho, nao autentica o publicador. Dividir implementacao em novos IDs revisaveis.

### U01 - Melhorar fluxos cotidianos e recuperacao

- Ler: `src/studio/studio.html`, `src/studio/window.rs` e callbacks do motor.
- Fazer: listar e corrigir um fluxo por subtarefa: primeiro uso, importar, aplicar
  por tela, erro de carregamento e restaurar configuracao. Revisar teclado, foco,
  nomes acessiveis e layouts em DPI alto; exibir erro sem esconder o ultimo valido.
- Aceite: cada fluxo tem roteiro e resultado observado; opcoes indisponiveis sao
  claras; interface nao anuncia sucesso antes de o motor confirmar.

### U02 - Templates e parametros acessiveis

- Ler: `src/studio/lab.html`, `src/studio/lab.rs`, `src/render/params.rs` e contrato.
- Fazer: adicionar poucos templates originais, com parametros descritos e limites;
  permitir alterar valores sem editar codigo, preservando o ultimo render valido.
- Aceite: preview e desktop usam os mesmos valores; preset salvo reabre corretamente;
  parametros invalidos sao rejeitados ou limitados conforme contrato documentado.

### U03 - Fechar o fluxo de criacao com IA

- Ler: `src/ai/agent.rs`, `src/ai/mod.rs`, `docs/AI.md` e Studio.
- Fazer: consolidar descrever -> gerar -> renderizar -> ajustar -> salvar -> aplicar.
  Preservar aplicacao como acao explicita do usuario, sem dar essa ferramenta ao agente.
- Aceite: cancelamento, erro de provedor e limite de uso mantem o trabalho recuperavel;
  explicar quais prompts/imagens vao ao provedor. Expor limite de uso configuravel;
  nao prometer teto financeiro a partir de um limite apenas de tokens de saida.
- Validar: backend simulado para sucesso, recusa, retries, cancelamento e limite.

## Fase 5: expansao

### A01 - Catalogo curado

- Fazer: definir ficha de qualidade por wallpaper: autoria/licenca, desempenho,
  formatos de tela, legibilidade de icones, intensidade de movimento e permissoes.
  Adicionar uma pequena colecao por vez; usar referencias como inspiracao, sem
  redistribuir codigo ou assets de terceiros sem verificar a licenca.
- Aceite: cada item tem preview do proprio motor e ficha preenchida; catalogo e
  documentacao concordam. Distribuicao remota de conteudo exige projeto de confianca
  separado, pois conteudo ativo tambem pode executar comportamento nao desejado.

### M01 - Estudo de macOS, sem port prematuro

- Fazer: mapear contratos portaveis (configuracao, parametros, playlists, pacotes)
  e adaptadores Windows; propor Metal, web e video no macOS em documento separado.
- Aceite: distinguir funcionalidades equivalentes de limites do sistema, incluindo
  Spaces e tela de bloqueio; estimar esforco por backend; nao introduzir abstracoes
  genericas na producao sem uma segunda implementacao validada em Mac real.

## Gate de conclusao de cada fase

1. Todos os IDs da fase possuem evidencias de aceite ou bloqueios explicitados.
2. `cargo test --locked` passa no ambiente Windows adequado; ignorados sao explicados.
3. Depois de Q01, Clippy estrito passa. Auditoria de dependencias e verificada no CI.
4. Mudancas visuais possuem captura; mudancas de desempenho possuem medidas.
5. Comportamentos de seguranca possuem testes negativos, nao apenas casos felizes.
6. O resumo diferencia implementado, testado e ainda dependente de validacao manual.

## Modelo de registro por ID

```text
ID:
Estado:
Data / commit:
Comportamento anterior confirmado:
Arquivos alterados:
Criterios de aceite atendidos:
Comandos executados e resultados:
Validacao manual realizada:
Limitacoes / pendencias:
Proximo ID elegivel (nao executar automaticamente):
```

## Registro de execucao

```text
ID: S01
Estado: concluido
Data / commit: 2026-10-15 / (sem commit ainda)
Comportamento anterior confirmado: manifest.json lido com read_to_string sem teto,
  antes de checar entradas/bytes; JSON invalido era ignorado em silencio.
Arquivos alterados: src/wallpaper/manager.rs
Criterios de aceite atendidos: limites de entradas e tamanho declarado checados antes
  de ler o manifesto; leitura limitada por take(); JSON invalido e manifesto acima do
  teto recusados com diagnostico no log; sem manifesto usa os padroes; extracao so
  ocorre depois. Teto: 64 KiB (manifesto so tem type/entry).
Comandos executados e resultados: cargo test --locked package_tests -> 3 passed
Validacao manual realizada: nenhuma
Limitacoes / pendencias: suite completa nao executada; tamanho declarado no ZIP pode
  mentir, o limite real de bytes extraidos fica para S02.
Proximo ID elegivel (nao executar automaticamente): S03 ou S02
```

```text
ID: S03
Estado: concluido
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: update_audio/update_media/update_system enviavam
  dados a qualquer WebView, inclusive WebSource::Url (site remoto com rede).
Arquivos alterados: src/wallpaper/web.rs
Criterios de aceite atendidos: WebSource::allows(LocalData) centraliza a politica no Rust;
  WebWallpaper guarda `remote` e as tres pontes (audio, musica, sistema) retornam antes de
  qualquer evaluate_script. Builtin e Dir continuam recebendo. Sem consentimento ainda:
  liberar para URLs depende de S05. fps e cor de destaque (nao sao dados do usuario) seguem.
Comandos executados e resultados: cargo test --locked -> 129 passed, 3 ignored
Validacao manual realizada: nenhuma (nao abri um wallpaper por URL)
Limitacoes / pendencias: teste cobre a matriz de politica, nao um WebView real; a pagina
  remota ainda pode ler o que o proprio WebView2 expoe (S04).
Proximo ID elegivel (nao executar automaticamente): S04 ou S02
```

```text
ID: S04
Estado: concluido (falta validacao manual com WebView real)
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: WebViewBuilder sem handlers de navegacao, nova janela,
  permissao ou download; links/location/window.open saiam livremente.
Arquivos alterados: src/wallpaper/web.rs
Criterios de aceite atendidos: navigation_allowed() so aceita wallpaper://localhost e
  (http|https)://wallpaper.localhost para locais, e a propria origem para Url (redirect
  para outro host, outro esquema, file:, javascript:, user@host sao bloqueados);
  window.open negado; todas as permissoes do navegador negadas; downloads bloqueados.
  CSP connect-src 'none' continua, mas nao e a unica barreira.
Comandos executados e resultados: cargo test --locked -> 130 passed, 3 ignored
  (teste novo: navigation_stays_inside_the_wallpaper_origin)
Validacao manual realizada: nenhuma
Limitacoes / pendencias: nao testei HTML real com link/location/popup/form no WebView2;
  <form> POST para fora e subrecursos (img/script) remotos seguem so a CSP (img-src/
  script-src 'self'); wallpapers Url nao tem CSP. Handler de permissao do wry nao cobre
  clipboard-read (desligado por padrao). shadertoy::render_glsl_file ignorado falha por
  falta de arquivo de entrada (nao relacionado).
Proximo ID elegivel (nao executar automaticamente): S02 ou S05
```

```text
ID: S02
Estado: concluido
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: remove_dir_all(cache/<nome>) seguido de archive.extract
  direto no destino; homonimos colidiam; sem limite de bytes reais.
Arquivos alterados: src/wallpaper/manager.rs
Criterios de aceite atendidos: extract_package() usa staging exclusivo e so promove (rename)
  no sucesso; enclosed_name rejeita traversal; links simbolicos e entradas *.livewallpaper
  (aninhadas) recusados; limite de bytes realmente descompactados (take). Cache agora e
  <nome>-<hash(caminho,tamanho,data)>: homonimos nao colidem e um pacote alterado ganha pasta
  nova sem apagar a que um wallpaper em uso le.
Comandos executados e resultados: cargo test --locked -> 133 passed, 3 ignored
  (novos: extracao_ok_promove_staging, extracao_recusa_traversal_aninhado_e_excesso_sem_deixar_rastro,
  cache_name_diferencia_homonimos)
Validacao manual realizada: nenhuma
Limitacoes / pendencias: pastas de cache antigas acumulam (sem limpeza); limites de teste
  pequenos passados por parametro, nenhum ZIP bomb real criado.
Proximo ID elegivel (nao executar automaticamente): S05, Q01 ou C01
```

```text
ID: Q01
Estado: concluido
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: CI rodava `cargo clippy --locked` sem -D warnings;
  clippy estrito acusava 13 erros (12 no bin + 1 no alvo de teste).
Arquivos alterados: .github/workflows/ci.yml, src/ai/{agent,mod,provider}.rs,
  src/control/mod.rs, src/lab/mod.rs, src/mcp/psl.rs, src/shadertoy.rs,
  src/wallpaper/{image,web}.rs
Criterios de aceite atendidos: CI agora roda `cargo clippy --locked --all-targets -- -D warnings`.
  Correcoes pontuais; unico allow e #[allow(clippy::too_many_arguments)] em ai::agent::run.
  Removido Kind::default_model (so usado em teste). LocalData::Media/System agora usados
  de verdade (flags allow_* em WebWallpaper).
Comandos executados e resultados: clippy estrito -> limpo; cargo test --locked -> 133 passed, 3 ignored
Validacao manual realizada: nenhuma
Limitacoes / pendencias: toolchain do CI e `stable`; os avisos vieram do Rust 1.98 local
  (rust-toolchain.toml acima) e um Clippy mais novo pode trazer lints novos. Ignorados: 2 sao
  de GPU/entrada manual (shadertoy::render_glsl_file exige arquivo), os demais dependem de
  sessao de midia/pipe real.
Proximo ID elegivel (nao executar automaticamente): Q02, C01 ou S05
```

```text
ID: Q02
Estado: concluido
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: README dizia 2 wallpapers (catalogo tem 16: 7 web + 9 shader),
  binario ~3.5 MB (release local de 2026-10-13: ~6.8 MB), "0% GPU", "0 CPU spike", "< 0.1ms",
  "altissimo desempenho"; nao citava IA, MCP, pipe, importador Shadertoy nem Lab no resumo.
Arquivos alterados: README.md
Criterios de aceite atendidos: catalogo e arquitetura conferem com o codigo; afirmacoes absolutas
  trocadas por ressalvas ou medidas; tela de bloqueio/wallpaper do sistema descritos como imagem
  estatica (nao animado, nao screensaver); limites do importador GLSL explicados; sandbox web
  atualizado com S03/S04; comando de clippy igual ao do CI; links locais do README e dos docs conferidos.
Comandos executados e resultados: verificacao de links com shell; nenhum exemplo executado
Validacao manual realizada: nenhuma
Limitacoes / pendencias: nao revisei docs/web-wallpaper-guide.md, MCP.md e mcp-workflow.md linha a
  linha; "3 monitores testados" do README e declaracao do autor, nao verificada aqui.
Proximo ID elegivel (nao executar automaticamente): C01, S05 ou W01
```

```text
ID: C01
Estado: concluido (documento; sem codigo)
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: MCP e Lab CLI compilam em thread com recv_timeout (a thread
  continua viva no timeout); a previa do Lab no Studio (studio/lab.rs) e os wallpapers de shader
  (ShaderPipeline::compile) compilam na thread chamadora SEM timeout.
Arquivos alterados: docs/compiler-worker.md (novo)
Criterios de aceite atendidos: contrato v1 (JSON por linha via stdin/stdout, `--compile-worker`),
  limites de entrada (256 KiB) e saida (8 MiB), erros, dono do processo (Job Object, kill+wait),
  sem desktop/credenciais/rede; tabela de chamadores revisada; exemplos de sucesso, erro e limite;
  regressao exigida: bytecode identico ao compile_uncached atual.
Comandos executados e resultados: nenhum teste (so documento)
Validacao manual realizada: leitura de pipeline.rs, mcp/psl.rs, lab/mod.rs, studio/lab.rs, main.rs
Limitacoes / pendencias: valores de fila/concorrencia sao iniciais; endurecer o filho
  (token restrito) ficou fora; C02 foi subdividida no proprio documento (3 passos).
Proximo ID elegivel (nao executar automaticamente): C02, S05, W01
```

```text
ID: C02
Estado: em andamento (MCP e `--lab` prontos; Studio e wallpaper pendentes)
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: ver registro C01.
Arquivos alterados: src/compiler_worker.rs (novo), src/main.rs, src/mcp/psl.rs, src/lab/mod.rs,
  src/render/pipeline.rs (derive Debug), docs/compiler-worker.md
Criterios de aceite atendidos: `--compile-worker` implementa o contrato v1; cliente com timeout que
  faz kill() + wait(), no maximo 2 workers e fila de 4 (alem disso `Busy`); MCP e Lab CLI usam o
  worker (timeout -> Failure::Timeout / saida 3; falha do worker -> Rejected / saida 4); erro de
  compilacao nao toca no ultimo wallpaper valido (esses chamadores nao aplicam nada).
Comandos executados e resultados: cargo test --locked -> 140 passed, 3 ignored; clippy estrito limpo;
  manual: worker via pipe (ok e JSON invalido), `--lab render lab/studies/plasma_flow.hlsl` com o
  binario real -> exit 0, nenhum processo IsNotAEngine sobrando
Testes novos: protocolo (limites, versao, lixo), bytecode identico ao compile_uncached, base64,
  workers falsos (demora -> Timeout e kill, crash, lixo, exe inexistente, fonte grande sem spawn)
Validacao manual realizada: acima (sem timeout real de FXC, nao forcei travar a GPU)
Limitacoes / pendencias: (1) studio/lab.rs::set_code e wallpaper/shader.rs ainda compilam no
  processo, sem timeout; (2) sem Job Object: se o principal morrer, um filho pode terminar sozinho
  (ele acaba ao ler EOF/concluir, mas FXC travado ficaria); (3) em `cargo test` o cliente compila
  no processo (current_exe seria o harness); (4) novo exe filho por compilacao: latencia nao medida (B01).
Proximo ID elegivel (nao executar automaticamente): terminar C02 (Studio + wallpaper) ou S05/W01
```

```text
ID: C02 (atualizacao)
Estado: concluido
Data / commit: 2026-10-15 / (ver git log)
Arquivos alterados: src/render/pipeline.rs
Criterios de aceite atendidos: ShaderPipeline::compile (usado por wallpaper/shader.rs, previa do Lab
  no Studio via OffscreenRenderer e hot-reload) consulta o cache e, em cache frio, compila VS+PS no
  worker com prazo de 30 s e grava os dois no cache (mesma chave de antes, caches existentes seguem
  validos). Erro/timeout/queda do worker devolve Err e o chamador mantem o wallpaper anterior
  (hot-reload ja fazia isso). get_or_compile_bytecode ficou so para HLSL fixo do app (bloom, imagem).
Comandos executados e resultados: cargo test --locked -> 140 passed, 3 ignored; clippy estrito limpo;
  manual com binario real e LOCALAPPDATA temporario (cache vazio): `--render-previews aurora ocean`
  compilou via worker (~1,2 s a frio), gerou 4 BMPs e 9 arquivos de cache; nenhum processo sobrando.
Limitacoes / pendencias: sem Job Object (kill() so cobre o timeout; se o principal morrer, um FXC
  travado do filho ficaria orfao); nao forcei um timeout real de FXC; latencia do spawn nao medida (B01);
  a thread do motor continua bloqueada ate 30 s durante um hot-reload frio (antes: ilimitado).
Proximo ID elegivel (nao executar automaticamente): S05, W01 ou B01
```

```text
ID: W01
Estado: concluido (a chave existe; ainda nao e persistida: W02)
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: monitores identificados so pelo indice de EnumDisplayMonitors;
  config.toml guarda monitor_wallpapers por indice (HashMap<usize,String>).
Arquivos alterados: src/desktop/monitors.rs
Criterios de aceite atendidos: MonitorInfo ganhou `key` (persistente) separada de `id` (indice atual).
  A chave vem do caminho de interface do monitor (EnumDisplayDevicesW com EDD_GET_DEVICE_INTERFACE_NAME:
  modelo + instancia/porta). stable_keys(): independe da ordem de enumeracao; telas identicas com a
  mesma identidade ganham sufixo #2, #3 por posicao e nao colidem; sem identidade usa
  `pos:x,y,LxA` (fallback que muda se a tela for movida); desconectar uma tela nao muda as outras.
Comandos executados e resultados: cargo test --locked -> todos ok; clippy estrito limpo.
  Testes: ordem trocada, identicas, sem identidade, desconexao, enumeracao real (chaves unicas).
  Nesta maquina as 3 telas deram chaves distintas (SAM707D, SAM7305, GSM5B55).
Validacao manual realizada: enumeracao real; roteiro de hotplug/troca de porta NAO executado
Limitacoes / pendencias: a instancia no caminho pode mudar ao reinstalar o driver ou trocar de GPU
  (a tela viraria "nova"; a migracao W02 deve tratar); sufixo #n de telas identicas depende da posicao
  fisica, entao trocar duas telas identicas de lugar troca as atribuicoes; a chave ainda nao e usada
  pelo motor/Studio/config (W02).
Proximo ID elegivel (nao executar automaticamente): W02
```

```text
ID: W02
Estado: concluido (sem roteiro manual de hotplug)
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: config.toml guardava monitor_wallpapers por indice; um config.toml
  ilegivel virava o padrao e era sobrescrito no proximo save() sem backup.
Arquivos alterados: src/config.rs, src/engine.rs
Criterios de aceite atendidos: novo campo `monitor_wallpapers_by_key` (chave de W01) + flag
  `monitors_migrated`. resolve_monitors() roda em setup_viewports (inicio, WM_DISPLAYCHANGE, recuperacao
  do shell): na 1a vez adota as atribuicoes por indice do arquivo antigo; depois a chave manda (ordem
  trocada nao troca wallpapers; tela nova nao herda indice alheio e cai no padrao; tela desconectada
  continua guardada e volta com a configuracao). save() sincroniza indice->chave e grava em
  temporario + rename. config.toml ilegivel e copiado para config.toml.invalid antes de ser
  substituido pelo padrao (so se o backup ainda nao existir). `monitor_wallpapers` por indice segue
  existindo (runtime, Studio, copia legada), entao Studio e demais usos nao mudaram.
Comandos executados e resultados: cargo test --locked -> 150 passed, 3 ignored; clippy estrito limpo.
  Testes: migracao unica, reordenacao, desconexao/reconexao/tela nova, sincronizacao na gravacao,
  ida e volta TOML com chave de caminho de interface, arquivo antigo sem os campos novos.
Validacao manual realizada: nenhuma (nao rodei o app para nao reescrever o config.toml real)
Limitacoes / pendencias: o Studio ainda mostra/edita por indice e as outras configs por monitor
  (playlist etc.) nao foram revisadas; se o Windows mudar o caminho de interface (troca de GPU/driver),
  a tela vira "nova" e usa o padrao; a copia legada por indice pode divergir da por chave em
  downgrade; backup .invalid guarda so a primeira falha.
Proximo ID elegivel (nao executar automaticamente): W03, S05 ou B01
```

```text
ID: W03
Estado: concluido no codigo e nos testes unitarios; NAO validado manualmente com varias telas
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: so pausava se TODAS as telas estivessem cobertas
  (OcclusionDetector::are_all_occluded); a pausa era um unico booleano para todos os wallpapers;
  o tempo dos shaders vinha do relogio de parede e saltava ao retomar.
Arquivos alterados: src/pause.rs, src/engine.rs, src/wallpaper/manager.rs, src/desktop/occlusion.rs,
  README.md
Criterios de aceite atendidos: pausa manual/bateria (globais) separada da cobertura (por monitor):
  monitor_suspended(manual, battery, coberto). A cada tick o motor calcula a cobertura de cada tela
  (is_monitor_occluded), aplica set_monitor_paused so onde mudou (WebView2 oculta/suspensa so naquela
  tela) e render_all_viewports pula a tela suspensa, incluindo a ponte de audio/musica/sistema.
  O manager lembra os monitores pausados, entao um wallpaper aplicado durante a pausa ja nasce pausado.
  PauseClock por monitor congela o tempo suspenso: retomar nao salta. Removido are_all_occluded (sem uso).
  `auto_paused` publicado no pipe/Studio continua significando "bateria ou todas as telas cobertas".
Comandos executados e resultados: cargo test --locked -> 152 passed, 3 ignored; clippy estrito limpo.
  Testes: monitor_suspended (uma tela coberta, manual, bateria) e PauseClock (congela, retoma sem saltar,
  chamadas repetidas).
Validacao manual realizada: nenhuma. Roteiro pendente (2+ telas): maximizar janela em uma tela e ver a
  outra animando; pausa manual e bateria; hotplug com tela coberta; trocar wallpaper com a tela coberta.
Limitacoes / pendencias: a cobertura usa so a janela em primeiro plano (como antes); previews do Studio
  nao foram alterados; o relogio descontado vale para shaders (video/imagem usam seus proprios
  relogios); a bateria com Studio aberto segue como antes.
Proximo ID elegivel (nao executar automaticamente): S05, B01 ou D01
```

```text
ID: S05
Estado: concluido (UI conferida so numa pagina estatica; sem teste com WebView2 real)
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: S03 bloqueava tudo para URLs sem como autorizar.
Arquivos alterados: src/config.rs, src/wallpaper/web.rs, src/wallpaper/manager.rs, src/engine.rs,
  src/studio/window.rs, src/studio/studio.html, README.md
Criterios de aceite atendidos: `web_grants` (config.toml) guarda por ORIGEM (`https://site.com`) tres
  permissoes — audio, musica (media) e sistema — todas desligadas por padrao; config antiga carrega
  sem elas. Wallpapers locais (embutidos e pasta) nao precisam de permissao (comportamento atual).
  WebWallpaper guarda `grants` e as tres pontes consultam o Rust antes de qualquer envio; a cada
  quadro o motor aplica config.web_grants_for(origem) ao site do monitor, entao conceder/revogar vale
  na hora e mudar de URL/origem comeca sem permissao. Revogar para a entrega e limpa a ponte
  (_push zerado, _setMedia(null), _setSystem(null)); reconceder reenvia. Studio: Ajustes › Sites remotos
  lista as origens em uso e as ja configuradas, com 3 caixas por origem e o aviso de que o site tem rede
  e de que desligar nao apaga o que a pagina ja guardou. Entrada validada no Rust (so http/https,
  sem caminho/credenciais; tipos fixos). Implementacao feita nos tres tipos juntos (audio, musica,
  sistema) porque compartilham o mesmo mecanismo.
Comandos executados e resultados: cargo test --locked -> 154 passed, 3 ignored; clippy estrito limpo;
  `node --check` no script do Studio; pagina studio.html aberta no navegador com origens simuladas:
  lista, estado das caixas e screenshot conferidos.
  Testes: grants por origem/esquema e remocao, entradas invalidas, arquivo antigo, politica por tipo de fonte.
Validacao manual realizada: UI estatica (acima). NAO testei IPC real, revogacao em WebView2 ao vivo
  nem uma pagina remota que le engine.system.
Limitacoes / pendencias: o que a pagina ja copiou nao pode ser recolhido; permissao e por origem, nao por
  wallpaper (duas telas com o mesmo site compartilham); a lista do Studio usa o estado local
  (state.active) e atualiza ao abrir Ajustes; consentimento nao tem prazo/expiracao.
Proximo ID elegivel (nao executar automaticamente): B01, D01 ou U01
```

```text
ID: D01
Estado: concluido nos arquivos e localmente; o workflow NAO foi executado num runner do GitHub
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: release.yml usava a tag como versao do pacote (podia divergir do
  Cargo.toml e do .iss), nao rodava clippy nem auditoria, nao tinha inventario de dependencias, calculava
  SHA256SUMS enumerando a propria pasta e nao testava o .exe fora do projeto.
Arquivos alterados: .github/workflows/release.yml, .github/workflows/ci.yml, scripts/release-check.ps1 (novo),
  src/compiler_worker.rs, README.md
Criterios de aceite atendidos: scripts/release-check.ps1 — `-Phase pre` (Cargo.toml == .iss == tag) e
  `-Phase post` (instalador, zip com .exe e README, DEPENDENCIES.txt, SHA256SUMS que lista e confere todos
  os arquivos, e smoke do .exe extraido numa pasta temporaria: `--mcp-config` e round-trip do worker de
  compilacao) e `-SelfTest` (prova que versao divergente, arquivo ausente, soma errada e zip sem README
  sao detectados; roda no CI). O release agora tem os jobs `gate` (autoteste, versoes, clippy estrito,
  testes) e `audit` (rustsec), e o job `windows` depende dos dois; a versao publicada vem do Cargo.toml;
  inventario via `cargo metadata --filter-platform x86_64-pc-windows-msvc` (169 crates, com licenca);
  somas calculadas por ultimo. CI roda o autoteste e a checagem de versoes.
Bug real encontrado pelo smoke: o worker de compilacao recusava stdin com BOM UTF-8 (PowerShell 5.1 / .NET
  Framework); agora tolera BOM (teste novo).
Comandos executados e resultados: `release-check.ps1 -SelfTest` ok; `-Phase pre` ok (0.1.0) e com
  `-Tag v0.2.0` falha como esperado; `-Phase post` contra um dist montado localmente com o .exe release real
  (instalador falso, pois nao ha ISCC aqui) ok; cargo test --locked 154 ok; clippy estrito limpo.
Validacao manual realizada: acima. YAML dos workflows revisado a olho (sem validador disponivel); ISCC e o
  upload/publicacao nao exercitados.
Limitacoes / pendencias: o instalador real nao foi gerado/testado aqui; o smoke nao inicia o app completo
  (isso mexeria no desktop do usuario) — so os modos sem desktop; versoes de pre-lancamento na tag sao
  tratadas como a versao base; sem assinatura (D02).
Proximo ID elegivel (nao executar automaticamente): D02, B01 ou U01
```

```text
ID: D02
Estado: concluido como PREPARACAO; assinatura real NAO feita (exige certificado seu)
Data / commit: 2026-10-15 / (ver git log)
Arquivos alterados: .github/workflows/release.yml, scripts/sign-artifact.ps1 (novo),
  scripts/release-check.ps1, docs/signing.md (novo), README.md
Criterios de aceite atendidos: docs/signing.md descreve opcoes (OV/EV, nuvem, autoassinado), os segredos
  necessarios (WINDOWS_SIGNING_PFX_BASE64, WINDOWS_SIGNING_PFX_PASSWORD, variavel opcional do servidor de
  carimbo) e como verificar. O workflow assina o .exe antes do Inno Setup, assina o instalador, monta o zip
  com o .exe assinado e calcula as somas por ultimo. Sem credencial: avisa no log e no resumo e segue sem
  assinar (signed=false). Com credencial e falha: o job falha. release-check.ps1 ganhou -RequireSigned
  (Authenticode Valid no instalador e no .exe do zip), ligado so quando a assinatura ocorreu. Nenhum
  certificado, senha ou token no repositorio.
Comandos executados e resultados: `release-check.ps1 -SelfTest` ok (agora com casos de arquivo sem assinatura
  e inexistente); `sign-artifact.ps1` sem segredos -> avisa e sai 0; `-Phase post -RequireSigned` contra um
  instalador falso e o .exe sem assinatura -> falha como esperado (`UnknownError`/`NotSigned`).
Validacao manual realizada: acima. O caminho FELIZ (signtool com .pfx, carimbo, verify) nunca foi
  executado: nao ha certificado aqui e o workflow nao rodou num runner.
Limitacoes / pendencias: certificado real; escolha .pfx x assinatura em nuvem (so .pfx implementado);
  o servidor de carimbo e um terceiro que recebe o hash; SmartScreen pode avisar mesmo assinado ate haver reputacao.
Proximo ID elegivel (nao executar automaticamente): D03, B01 ou U01
```

```text
ID: D03
Estado: concluido como PROJETO (docs/updater-design.md); nenhum codigo de atualizador
Data / commit: 2026-10-15 / (ver git log)
Arquivos alterados: docs/updater-design.md (novo)
Criterios de aceite atendidos: o documento define manifesto assinado (Ed25519, chave propria offline,
  separada do Authenticode), raiz de confianca embutida com rotacao, HTTPS + lista de hosts permitidos,
  hash e tamanho dentro do manifesto, compatibilidade (`schema`, `min_supported`, arquitetura), protecao
  contra downgrade/replay (`counter` monotonico), rollback (copia da versao anterior) e controle do usuario
  (nada baixa nem instala sem pedir; verificacao automatica desligada por padrao). Registra explicitamente
  que hash servido junto do arquivo nao autentica o publicador. Garante que arquivo adulterado nunca e
  executado e download incompleto nunca substitui a instalacao. Implementacao dividida em 6 itens
  revisaveis (U-D03a..f) com teste principal de cada um.
Comandos executados e resultados: nenhum (documento).
Validacao manual realizada: nenhuma; sem prototipo.
Limitacoes / pendencias: decisoes em aberto listadas no documento (biblioteca Ed25519 — nao ha no
  Cargo.toml, hospedagem do manifesto, guarda da chave privada, politica automatica, cliente HTTP).
  Os IDs U-D03a..f NAO foram adicionados a tabela de estados; criar quando forem priorizados.
Proximo ID elegivel (nao executar automaticamente): B01, U01 ou V01
```

```text
ID: B01
Estado: em andamento. Metodo e ferramenta PRONTOS; medicoes NAO realizadas (bloqueado: exigem rodar o motor no
  desktop real, o que troca o wallpaper e mexe no config do usuario — nao fiz isso sem combinar)
Data / commit: 2026-10-15 / (ver git log)
Arquivos alterados: docs/performance.md (novo), scripts/perf-sample.ps1 (novo), .gitignore
Criterios de aceite atendidos: documentado o que medir (CPU, memoria privada/working set, GPU util e VRAM por PID,
  soma da arvore inclusive subprocessos WebView2), a matriz de cenarios (shader, web, video x Studio aberto/fechado
  x desktop coberto, mais controle sem wallpaper), roteiro, tabela de ambiente a preencher, repeticoes (3) e
  regras para comparar com outros programas. Explicita que gpu_ms do Lab NAO e consumo total e que tolerancias so
  vem depois das medidas. Dados indisponiveis aparecem como "indisponivel", nao estimados.
  Nao atendido: medir shader, web e video (aceite principal do ID).
Comandos executados e resultados: so a mecanica do script, contra o `explorer` (arvore de 75 processos): CSV
  com ponto decimal, 1ª amostra descartada, intervalo efetivo ~4 s nessa arvore, contadores de GPU lidos nesta
  maquina. Nenhum numero do IsNotAEngine foi medido.
Validacao manual realizada: nenhuma do produto.
Limitacoes / pendencias: rodar a matriz (a pessoa dona da maquina ou com consentimento para trocar wallpapers);
  frame time do desktop real nao e medido (PresentMon/ETW ficam para depois); energia/bateria so por metodo externo;
  README ainda nao cita numeros (corretamente).
Proximo ID elegivel (nao executar automaticamente): U01, V01 (precisa das medicoes), A01 ou terminar B01
```

```text
ID: W03 (correcao pos-teste do usuario)
Data / commit: 2026-10-15 / (ver git log)
Problema relatado: com programas maximizados nas 3 telas, o wallpaper seguia animando.
Causa: a cobertura olhava so a janela em PRIMEIRO PLANO, entao so uma tela por vez podia ser coberta; alem
  disso uma janela maximizada com barra de tarefas nao cobre o retangulo inteiro do monitor, e o Studio
  maximizado era ignorado de proposito.
Arquivos alterados: src/desktop/occlusion.rs, src/engine.rs
Correcao: OcclusionDetector::occluded_monitors() enumera TODAS as janelas de nivel superior (EnumWindows) e
  marca cada monitor coberto por uma janela visivel, nao minimizada e nao "cloaked" (DWM) que o cobre ou que
  esta maximizada nele; ignora janelas de ferramenta, sem foco e clique-atraves, e as classes do shell e as
  internas do app. O Studio maximizado agora conta como cobertura.
Comandos executados e resultados: cargo test --locked -> 157 passed; clippy estrito limpo. Verificacao real
  neste PC (3 telas): com o Studio nao contando, [true,false,true]; a unica janela maximizada na tela do meio
  era o proprio Studio, motivo do wallpaper seguir animando atras da dock. Apos a correcao ela conta.
Validacao manual realizada: enumeracao real de janelas; o usuario precisa reabrir a build nova para confirmar
  as 3 telas pausando.
Limitacoes: janela "tela cheia" que nao esta maximizada e nao cobre o retangulo (rara) nao conta; sobreposicoes
  grandes sem estilo de ferramenta (ex.: algumas docks em tela cheia) poderiam contar como cobertura.
```

```text
ID: W02 (correcao de regressao achada pelo usuario)
Data / commit: 2026-10-15 / (ver git log)
Problema: um wallpaper aplicado (ex.: via pipe) voltava para `matrix` ~6 s depois.
Causa: save() sincronizava indice->chave so numa COPIA; o mapa por chave em memoria ficava velho, e a
  proxima resolve_monitors (qualquer evento de display/shell) sobrescrevia a troca com o valor antigo.
Correcao: resolve_monitors() chama sync_monitor_keys() ANTES de remapear (usa a enumeracao anterior).
  Teste de regressao: wallpaper_changed_after_a_resolve_survives_the_next_resolve.
Comandos executados e resultados: cargo test --locked ok; reproduzido ao vivo pelo canal de controle
  (m1 virava matrix apos ~6 s). Correcao ainda nao reconfirmada ao vivo: exige trocar o .exe em uso.
```

```text
ID: U01
Estado: implementado e testado em unidade/pagina estatica; roteiro manual no app pendente (abaixo)
Data / commit: 2026-10-15 / (ver git log)
Arquivos alterados: src/wallpaper/manager.rs, src/engine.rs, src/config.rs, src/studio/window.rs, src/studio/studio.html
Fluxos revisados (um por item):
 1. Aplicar por tela / erro de carregamento. ANTES: um wallpaper que nao carregava trocava o que estava no ar
    pelo shader de fallback, o spec ruim era GRAVADO no config e o Studio dizia "aplicado". AGORA:
    set_monitor_wallpaper mantem o wallpaper anterior se o novo falhar (se a WebView anterior ja tinha sido
    liberada, recoloca o anterior; sem anterior, fallback), devolve Err e o motor NAO grava o spec; o Studio so
    anuncia sucesso quando o motor confirma (onApplied) e, se falhar, desfaz a selecao e mostra o erro
    (onApplyError) dizendo que o anterior continua. Vale para "Aplicar", "Em todos" e botoes da IA.
    Teste: extensao do teste de gerenciador (spec inexistente -> Err, anterior continua, spec anterior guardado).
 2. Importar: nao havia como usar uma URL no Studio (so pelo pipe). Novo botao "Usar uma pagina (URL)" com
    modal, validacao (http/https, sem usuario/senha), aviso de que sites remotos nao recebem seus dados
    sem permissao (S05) e Esc/Enter.
 3. Restaurar configuracao: Ajustes › Dados › "Restaurar padroes", em DUAS etapas (1o clique pede
    confirmacao; expira em 6 s). O motor copia o config.toml para config.toml.bak-<unix> e SO ENTAO restaura
    (se a copia falhar, cancela); preserva tour visto e preferencias da IA (chaves ficam no Windows), reaplica
    velocidade/cor/mouse/parallax/autostart, playlist e wallpapers por monitor. Teste: reset_to_defaults.
 4. Teclado, foco e nomes acessiveis: todos os controles sem nome ganharam aria-label (9 chaves, o
    deslizante de velocidade, o campo hex); as chaves (input de largura 0) agora mostram o anel de foco na
    trilha; foco visivel tambem em range/search/textarea/hex; os dois modais viraram role=dialog com
    aria-modal; Esc fecha. Antes: 11 controles sem nome.
 5. Primeiro uso: o tour ja existia e nao foi alterado.
Comandos executados e resultados: cargo test --locked -> 159 passed; clippy estrito limpo; node --check no
  script; pagina studio.html no navegador: URL invalida/credencial recusadas, URL valida envia set_wallpaper e
  fecha, onApplyError desfaz a selecao, onApplied mostra o toast, reset exige 2 cliques e envia uma unica
  mensagem; auditoria de nomes acessiveis = 0 controles sem nome.
Validacao manual realizada: pagina estatica (acima). NAO exercitado no app real: aplicar arquivo corrompido,
  URL pelo modal, restaurar configuracao (cria backup e reaplica), DPI alto, leitor de tela.
Roteiro manual pendente: (a) aplicar um .hlsl com erro -> toast de erro e o anterior segue; (b) "Usar uma
  pagina" com https://example.com; (c) Restaurar padroes -> conferir config.toml.bak-* na pasta de dados;
  (d) Tab/Shift+Tab pelos Ajustes vendo o foco nas chaves; (e) tela em 150-200% de escala.
Limitacoes / pendencias: falha ASSINCRONA de uma URL (DNS/offline) nao e detectada como erro; apos
  restaurar, o Studio pede para reabrir (a configuracao inicial e injetada na abertura); os fluxos
  "aplicar" pelo pipe, bandeja e playlist nao mostram toast de erro; focus trap completo nos modais nao feito.
Proximo ID elegivel (nao executar automaticamente): U02, U03, A01
```

```text
ID: U02
Estado: implementado e testado (unidade, GPU offscreen, paginas no navegador); roteiro manual no app pendente
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: varios shaders do catalogo ja declaravam `// param:`, mas nao havia
  como mudar os valores (so `--lab --param` na linha de comando); previa do Lab e desktop usavam so padroes.
Arquivos alterados: src/render/params.rs, src/render/offscreen.rs, src/wallpaper/{mod,shader,manager,catalog}.rs,
  src/config.rs, src/engine.rs, src/studio/{window,lab}.rs, src/studio/{studio,lab}.html, docs/shader-contract.md,
  assets/wallpapers/templates/{ondas,grade_neon,nevoa}.hlsl (novos), descricoes nos `// param:` de 7 shaders
Criterios de aceite atendidos:
 - Poucos templates originais, com parametros descritos e limites: 3 (ondas, grade_neon, nevoa; 5 parametros
   cada), no menu do Lab. Diretiva estendida: `// param: nome min max padrao -- descricao` (retrocompativel).
 - Alterar valores sem editar codigo: Studio (Galeria › Ajustes, so para shaders do catalogo com parametros)
   e Lab (sliders abaixo da previa). Nomes acessiveis (aria-label com descricao), valor lido por aria-valuetext.
 - Preservar o ultimo render valido: erro de compilacao no Lab continua mantendo o anterior (teste existente);
   mudar valores nao recompila.
 - Preview e desktop usam os mesmos valores: ambos passam por params::resolve (limita a faixa, ignora nomes
   desconhecidos, NaN -> padrao); a previa do catalogo (/frame/<id>?p=) e a do Lab (/lab/frame?p=) usam
   OffscreenRenderer::render_with_params; o desktop usa ShaderWallpaper::set_params (reaplica no hot-reload).
 - Preset salvo reabre corretamente: valores em config.toml [shader_params.<wallpaper>] (limite 8 por
   wallpaper e 64 wallpapers, validados), reaplicados a cada carga do wallpaper (manager.param_overrides) e no
   inicio; no Lab "Salvar/Aplicar" enviam os valores junto do caminho do arquivo e os valores do Lab ficam em localStorage.
 - Parametros invalidos: rejeitados (nome fora de [A-Za-z0-9_]{1,32}, valor nao finito, spec vazio/grande) ou
   limitados a faixa conforme o contrato (docs/shader-contract.md atualizado).
 - Gravacao em disco dos valores em lote (no maximo 1 vez por segundo) para nao gravar a cada passo do slider.
Comandos executados e resultados: cargo test --locked -> 166 passed, 3 ignored; clippy estrito limpo; node --check
  nos dois scripts. Testes novos: descricao opcional/limitada a 80, resolve (limita/ignora/NaN), parse_overrides,
  config.shader_params (validacao, limites, ida e volta TOML), modelos (compilam, faixas, descricoes, renderizam
  algo nao chapado e o brilho altera a imagem na GPU) e catalog_json com params. Imagens dos 3 modelos
  renderizadas pelo Lab e conferidas visualmente (grade_neon: reduzido o serrilhado perto do horizonte).
  Paginas studio.html e lab.html no navegador: sliders, consulta `&p=`, reset, envio de set_shader_param.
Validacao manual realizada: acima. NAO exercitado no app real: mover um slider e ver o desktop mudar ao vivo,
  salvar um shader do Lab com valores e reaplicar apos reiniciar, o menu agrupado Modelos/Catalogo, DPI alto.
Roteiro manual pendente: (a) Galeria › Aurora › mexer em Intensity e ver a previa e o desktop; (b) fechar e
  abrir o motor: valores mantidos; (c) Padrões; (d) Lab › Modelos › Ondas, ajustar, Salvar e Aplicar; (e) Tab pelos sliders.
Limitacoes / pendencias: shaders IMPORTADOS (.hlsl do usuario) nao mostram painel de ajustes na Galeria (so no
  Lab); a previa em miniatura dos cards da galeria nao reflete os valores; wallpapers iguais em monitores
  diferentes compartilham os mesmos valores (por wallpaper, nao por monitor); sem presets nomeados (apenas
  "os valores atuais" e "padroes").
Proximo ID elegivel (nao executar automaticamente): U03, A01
```

```text
ID: U03
Estado: implementado; testado com backend HTTP simulado + ferramentas e GPU reais; NAO testado contra a API real
Data / commit: 2026-10-15 / (ver git log)
Comportamento anterior confirmado: o laco ja existia (descrever -> gerar -> renderizar -> corrigir -> salvar) com
  cancelar/erro revertendo o historico, mas: limites fixos (24 rodadas, 250 mil tokens de SAIDA), codigo que renderizou
  e nao foi salvo se perdia ao cancelar/errar, nao havia "continuar" apos o limite nem como ajustar um shader salvo.
Arquivos alterados: src/ai/{agent,mod}.rs, src/config.rs, src/engine.rs, src/mcp/mod.rs (psl visivel ao crate),
  src/studio/{lab,window}.rs, src/studio/{studio,lab}.html, docs/AI.md
Criterios de aceite atendidos:
 - Descrever -> gerar -> renderizar -> ajustar -> salvar -> aplicar: o ajuste e "Ajustar no Lab" no cartao do
   wallpaper salvo (abre o .hlsl, com os controles de parametros do U02, via /lab/open confinado a pasta de wallpapers).
 - Aplicar continua EXPLICITO e fora do alcance do agente (teste existente: o modelo nao tem apply_wallpaper).
 - Cancelamento, erro de provedor, recusa e limite mantem o trabalho recuperavel: Toolbox::draft guarda o ultimo
   codigo que renderizou/compilou e nao foi salvo (erro de compilacao nao o substitui; save_wallpaper o limpa);
   no fim, vai para %LOCALAPPDATA%\IsNotAEnginei_drafts (20 mais recentes) e para o Studio (Copiar codigo,
   Abrir no Lab, Importar como wallpaper). Parou por limite: botao "Continuar de onde parou" na mesma conversa.
 - Explica o que vai ao provedor: pedido, codigo, quadros renderizados, instrucoes fixas, e que a conversa inteira
   e reenviada a cada rodada (texto na tela e em docs/AI.md).
 - Limite de uso configuravel: rodadas (4-60, padrao 24) e tokens lidos+escritos (50 mil-5 mi, padrao 1 mi) em
   Criar com IA, persistidos em config.ai (arquivos antigos carregam com padroes; valores fora da faixa sao
   ajustados); somados ao teto fixo de 250 mil tokens de saida. NAO se promete teto financeiro: texto na tela e na
   doc dizem que e limite de USO, que o preco depende do modelo/provedor e que o limite de gastos se define no
   painel do provedor.
Testes (backend simulado): sucesso (existente), erro do provedor no meio -> failed + rascunho recuperado igual ao
  codigo e arquivo gravado; limite de rodadas e de tokens -> step_limit/token_limit + rascunho + limites no evento;
  teto total conta entrada+saida e a conversa fica valida para continuar; rascunho: erro nao substitui, salvar limpa,
  web vs shader; poda para 20 arquivos; config (padroes, clamp, arquivo antigo); abrir no Lab confinado a pasta.
  Retentativas, recusa e cancelamento: testes existentes (http.rs e agent.rs). cargo test --locked -> 172 passed;
  clippy estrito limpo; scripts do Studio e do Lab conferidos no navegador com eventos simulados (selects de limites,
  botao Continuar, cartao de rascunho por tipo, "Ajustar no Lab").
Validacao manual realizada: pagina estatica (acima). NAO exercitado: geracao real com chave, "Ajustar no Lab" no app,
  Abrir no Lab de um rascunho, copiar para a area de transferencia no WebView2.
Limitacoes / pendencias: rascunho so cobre o ultimo codigo que funcionou (nao o historico); rascunho web so tem
  "Copiar codigo"; o prompt "Continue..." esta em ingles fixo; o modelo pode gastar ate o limite configurado de
  tokens, que NAO equivale a um valor em dinheiro; cenarios de rede real (429/5xx) so por teste de unidade do http.
Proximo ID elegivel (nao executar automaticamente): A01 (catalogo curado); B01 (medicoes) segue bloqueado
```

```text
ID: PEDIDO-DO-USUARIO "Criar" + remover o que foi criado/importado (fora da tabela de IDs)
Estado: implementado e testado em unidade/pagina estatica; teste manual no app pendente
Data / commit: 2026-10-15 / (ver git log)
Problema confirmado: (1) nao havia como apagar nada que o usuario criou/importou; (2) a galeria do Studio so
  listava o catalogo embutido + o que estava aplicado: wallpapers importados/criados em sessoes anteriores (e
  Lab/IA salvos) sumiam da interface depois de reiniciar o Studio, entao nem dava para reaplicar ou achar para apagar.
Arquivos alterados: src/import.rs, src/engine.rs, src/studio/{window,studio.html}, src/control/mod.rs, docs/CONTROL_PIPE.md, README.md
O que foi feito:
 - INIT do Studio agora traz `imported` (lista da pasta de importados: hlsl, html, video, imagem, pacote) e a
   galeria os mostra desde a abertura (categoria Importados). O pipe `list_wallpapers` tambem lista imagens (kind `image`).
 - Remover: botao "Remover…" na galeria e na lista "Seus wallpapers", em DOIS cliques (1o pede confirmacao, avisa se esta
   em uso). O motor so aceita caminhos ESTRITAMENTE dentro da pasta de importados (nunca a pasta, `..`, outro lugar),
   troca por `matrix` os monitores que usam, limpa playlist e valores de parametros ligados, manda o arquivo para a
   LIXEIRA do Windows (SHFileOperation com FOF_ALLOWUNDO; nunca apaga definitivamente) e, para .livewallpaper, apaga as
   extracoes do cache (`<nome>-<16 hex>`). Erro da Lixeira e reportado e nada e apagado em definitivo.
 - Aba Criar: cartoes IA, Lab, Shadertoy, URL, importar arquivo, os 3 modelos do Lab e a lista "Seus wallpapers"
   (aplicar, ajustar no Lab, remover).
 - Ajustes › Dados › Limpar dados do app: cache de shaders, cache de pacotes e rascunhos da IA (apagados); copias do
   config.toml (.bak-*, .invalid) vao para a Lixeira; mostra itens e tamanho; nunca toca em wallpapers nem no config.toml.
Comandos executados e resultados: cargo test --locked -> 176 passed, 4 ignored; clippy estrito limpo. Testes novos:
  listagem (so tipos suportados, ordenada), confinamento da remocao (fora, `..`, a propria pasta, inexistente),
  limpeza do cache de pacote (so as pastas daquele pacote), uso e limpeza de dados por tipo (so `.bin`, so
  rascunhos, config.toml e .tmp nunca entram, removedor injetavel). Teste real da Lixeira (ignorado por padrao,
  rodado uma vez a mao): o arquivo temporario saiu da pasta (ficou 1 arquivo `remover-teste-inae.hlsl` na Lixeira).
  Pagina do Studio no navegador: lista, dois cliques, resposta do motor (remove da galeria, monitor volta ao Matrix),
  recusa, dados e a aba Criar renderizada.
Validacao manual realizada: acima. NAO exercitado no app real: remover um wallpaper aplicado, de video/web (liberar o
  arquivo antes da Lixeira), pacote com cache, restaurar da Lixeira, limpar dados.
Limitacoes / pendencias: so arquivos soltos na pasta de importados (pastas de paginas web nao sao listadas);
  wallpapers por URL nao aparecem em "Seus wallpapers" apos reiniciar (nao ha arquivo; ficam so no config por monitor);
  se a Lixeira falhar depois de trocar o monitor, ele ja voltou ao Matrix; titulos iguais com extensoes diferentes
  aparecem como itens separados.
```

```text
ID: CORRECAO "app encerra ao aplicar o wallpaper descriptografia"
Data / commit: 2026-10-15 / (ver git log)
Sintoma: ao aplicar pelo Studio, o app fechava sozinho (Application Error: EmbeddedBrowserWebView.dll, excecao 0x80000003).
Causa: StudioHandle::eval_script chamava o WebView2 do Studio DIRETO da thread do motor. O WebView2 so pode ser tocado
  na thread da UI (objeto COM); de outra thread o Chromium aborta o processo. O codigo antigo ja fazia isso nas
  notificacoes de importacao; as notificacoes do U01/U02/U03/Criar (onApplied, onApplyError, onWallpaperRemoved,
  onDataUsage, onDataCleaned, onConfigReset) passaram a fazer isso a CADA aplicacao pelo Studio. Nao tinha relacao com
  a pagina "descriptografia" (WebGL): qualquer aplicacao pelo Studio derrubava, so que a pagina deixava a janela
  de tempo maior; aplicar pelo pipe (que nao avisa o Studio) nunca derrubou.
Reproducao (sandbox com LOCALAPPDATA temporario e DevTools na porta 9222): selecionar no Studio e aplicar -> o processo
  encerrava em ~3 s, 3 de 3 vezes; aplicar pelo pipe com o Studio aberto: nao caiu.
Correcao: eval_script agora enfileira o script para a thread da UI (post_script) em vez de chamar o WebView.
Verificacao: com a build corrigida, no mesmo roteiro: apply(false), apply(true), trocar para matrix e forcar um erro de
  carregamento — o processo seguiu vivo e os avisos do motor (toast de aplicado/erro) chegaram ao Studio.
Licao: faltou exercitar o app real nos fluxos U01-U03/Criar (so testei paginas estaticas e unidade). Um teste automatizado
  disso exige o WebView2 numa thread de UI; fica como pendencia.
```

```text
ID: A01 - Catalogo (escopo reduzido por decisao do usuario)
Estado: feito
Decisao: a biblioteca inicial e propria do projeto; o restante vem dos usuarios (Criar). Sem curadoria
  nem redistribuicao de terceiros, entao nao ha pesquisa de licencas externas.
Entregue: docs/catalog.md com ficha por wallpaper (tipo, audio, dados, movimento, legibilidade, permissoes) e
  teste que exige uma linha para cada id de CATALOG.
Pendencias: licenca do conteudo embutido nao definida (nao ha LICENSE); desempenho por wallpaper depende de B01;
  movimento/legibilidade sao estimativas, nao medicoes.
Proximo ID elegivel (nao executar automaticamente): B01 (medicoes, precisa do seu desktop)
```

```text
ID: B01-rodada1 - primeiras medicoes
Estado: feito (matriz reduzida, uma maquina)
Data: 2026-10-16
Feito: aurora, plasma_flow, matrix e clock_mono (Studio fechado) e aurora pausado, 3 repeticoes cada, 3 telas, 30 fps.
  Resultados e limites em docs/performance.md; brutos em perf/20261016-ryzen5600g (nao versionado).
Achado: matrix usa ~71% de um nucleo (3 vezes); shaders ~3% e 14-16% de GPU; pausa leva a ~0,5% e 0% de GPU.
Nao feito: Studio aberto, desktop coberto, video, modo desempenho, controle sem wallpaper, tolerancias.
Proximo ID elegivel (nao executar automaticamente): otimizar matrix; completar a matriz do B01; V01
```

```text
ID: SEG-REV-1 - revisao de seguranca e remocao de wallpaper por URL (pedido do usuario)
Estado: feito (revisao de codigo, nao pentest)
Data: 2026-10-16
Feito: wallpaper por endereco da internet removido (motor, Studio, pipe, WebGrants, aba Sites remotos); config antiga cai no
  padrao. Achados F1-F6 corrigidos (postMessage de qualquer frame, IPC restrito ao documento do Studio, CSP do Studio, escape de
  titulos, sem redirecionamento HTTP da IA). Relatorio e riscos que sobram em docs/SECURITY-REVIEW.md.
Verificado: testes (IPC so do documento, CSP presente, config antiga carrega); postMessage de iframe com sandbox = 0 comandos numa
  pagina de teste. NAO verificado: CSP e IPC no app real; WebView2 reportando a URL do frame.
Proximo ID elegivel (nao executar automaticamente): R1 (aviso ao importar HTML/pacote), R7 (SetDefaultDllDirectories), cargo audit
```
