# Desempenho: método e baseline (B01)

Estado: **primeira rodada de medições feita em 2026-10-16 (uma máquina, matriz reduzida; veja Resultados).**
Nenhum número deste projeto (nem o do README) deve ser citado como desempenho medido sem uma linha aqui que o respalde. Tolerâncias (limites aceitáveis) só serão definidas **depois** de
termos medidas reais, nunca antes.

## O que medir e por quê

O custo de um wallpaper é a **soma** de tudo que ele faz o computador gastar, não só o tempo de GPU de um
quadro. Por isso medimos, juntos:

| Métrica | De onde vem | Cuidado |
| --- | --- | --- |
| CPU (% de um núcleo) | `TotalProcessorTime` do `IsNotAEngine.exe` **e de todos os descendentes** (WebView2) | Inclui o Studio quando ele está aberto |
| Memória privada e working set (MB) | `Get-Process`, mesma árvore de processos | O WebView2 divide parte da memória; some a árvore inteira |
| GPU: utilização (%) e memória dedicada (MB) | Contadores `\GPU Engine(*)` e `\GPU Process Memory(*)` por PID | Podem faltar em alguns drivers; então o resultado diz "indisponível" |
| Tempo de quadro (ms) | **Não é medido de fora.** Use o Lab (`--lab render`) só para o custo de GPU do shader isolado | `gpu_ms` do Lab é custo de desenho offscreen, **não** é consumo total nem frame pacing do desktop |

**O que este método não mede:** energia/bateria (use `powercfg /batteryreport` ou um medidor externo e
descreva o método), latência de entrada, temperatura, e frame time do desktop real (exigiria captura com
PresentMon/ETW, que fica para depois).

## Ferramenta

`scripts/perf-sample.ps1` amostra a árvore de processos durante N segundos e grava `samples.csv` e
`summary.json` (média, p95, máximo e quantidade de amostras de cada métrica). Ele **não inicia nem altera
nada**: você prepara o cenário e roda o script.

```powershell
# 60 s de medição depois de 15 s de aquecimento (descartados)
powershell -File scripts\perf-sample.ps1 -ProcessName IsNotAEngine -Seconds 60 -WarmupSeconds 15 -Label "shader-aurora-studio-fechado"
```

Limites conhecidos do script: a passada do Windows (enumerar processos e ler contadores) leva de ~1 s a
alguns segundos, então o intervalo efetivo (`effective_interval_s` no resumo) fica acima do pedido; a CPU
é calculada pela diferença entre amostras, então a primeira é descartada; o resultado depende de quanto o
sistema todo está ocupado (feche outros aplicativos pesados).

## Cenários (matriz)

Cada linha é uma medição de 60 s depois de 15 s de aquecimento, repetida **3 vezes** (relatar as três e a
mediana, não só a melhor).

| Tipo | Wallpaper de referência | Estados |
| --- | --- | --- |
| Shader | `aurora` (cenário leve) e `plasma_flow` (cenário médio) | Studio fechado · Studio aberto · desktop coberto por janela maximizada |
| Web | `matrix` (áudio) e `clock_mono` (leve) | Studio fechado · Studio aberto · desktop coberto |
| Vídeo | um `.mp4` H.264 de 1080p30 e um de 4K30 (arquivos de teste **seus**, que não vão para o repositório) | Studio fechado · Studio aberto · desktop coberto |
| Controle | Nenhum wallpaper do motor (Windows com a imagem estática) | só para ter a linha de base do computador |

Observações do cenário "coberto": com várias telas, a pausa vale **por monitor** (W03); registre qual tela
foi coberta e qual ficou à vista.

## Roteiro de uma medição

1. Feche aplicativos pesados; ligue na tomada (ou registre que é bateria); deixe o plano de energia padrão.
2. Registre o ambiente (tabela abaixo) **uma vez por sessão de medição**.
3. Aplique o wallpaper pelo Studio, ajuste o FPS (padrão do projeto: 30) e coloque o cenário (Studio aberto/fechado, coberto).
4. Rode `perf-sample.ps1` com um `-Label` que descreva o cenário.
5. Guarde `summary.json` e `samples.csv` em `perf/<data>-<máquina>/` (essa pasta **não** é versionada por padrão;
   resultados que forem citados em documentação entram na seção "Resultados" abaixo, com o comando e a versão).
6. Repita 3 vezes; reinicie o motor entre cenários de tipos diferentes.

## Ambiente (preencher a cada sessão)

| Campo | Valor |
| --- | --- |
| Data | 2026-10-16 |
| Versão do app (`Cargo.toml` / commit) | 0.1.0 (build release do commit anterior a `chore: licença MIT`) |
| Build (release; toolchain `rustc -V`) | release, rustc 1.98.1 |
| Windows (edição e build) | Windows 11 Pro 26200 |
| CPU / núcleos lógicos | AMD Ryzen 5 5600G, 12 |
| GPU e versão do driver | AMD Radeon RX 7600 (32.0.31041.1004) e Radeon integrada (31.0.12027.9001) |
| RAM | 27,9 GB |
| Monitores (quantidade, resolução, taxa de atualização, quais conectados em qual GPU) | 3 telas, 1920x1080; uma a 120 Hz e outra a 59 Hz vistas pelo Windows; ligação por GPU não registrada |
| WebView2 Runtime (versão) | 155.0.4283.45 |
| Energia (tomada/bateria, plano) | desktop (sem bateria), plano padrão |
| FPS configurado no Studio | 30 |

## Resultados

Rodada de 2026-10-16, em uso real: o motor do próprio usuário, com o **mesmo wallpaper nas 3 telas**, Studio
fechado (janela escondida, WebView do Studio ainda existente), pausa por cobertura e por bateria desligadas
durante a medição, 30 fps, velocidade 1.0, 60 s depois de 15 s de aquecimento, 3 repetições. Configuração
do usuário restaurada ao final. CPU em % de **um** núcleo (100 = um núcleo inteiro; a máquina tem 12). GPU é a
soma dos engines 3D/Compute/Vídeo/Copy dos processos medidos, **não** é tempo de quadro.

| Cenário | Rep. | CPU % (média / p95) | Privada MB | GPU % | VRAM MB | Procs | Arquivos |
| --- | --- | --- | --- | --- | --- | --- | --- |
| aurora-studio-fechado | 1 | 2.88 / 6.34 | 436 | 15.2 | 318 | 7 | `perf/20261016-ryzen5600g/aurora-studio-fechado-r1/` |
| aurora-studio-fechado | 2 | 3.13 / 4.35 | 436 | 16.4 | 318 | 7 | `perf/20261016-ryzen5600g/aurora-studio-fechado-r2/` |
| aurora-studio-fechado | 3 | 2.95 / 4.87 | 435 | 16.3 | 318 | 7 | `perf/20261016-ryzen5600g/aurora-studio-fechado-r3/` |
| plasma_flow-studio-fechado | 1 | 2.84 / 4.32 | 434 | 15.7 | 318 | 7 | `perf/20261016-ryzen5600g/plasma_flow-studio-fechado-r1/` |
| plasma_flow-studio-fechado | 2 | 2.65 / 3.89 | 434 | 13.7 | 318 | 7 | `perf/20261016-ryzen5600g/plasma_flow-studio-fechado-r2/` |
| plasma_flow-studio-fechado | 3 | 2.74 / 4.85 | 439 | 14.2 | 318 | 7 | `perf/20261016-ryzen5600g/plasma_flow-studio-fechado-r3/` |
| matrix-studio-fechado | 1 | 71.13 / 96.55 | 886 | 5.6 | 583 | 15 | `perf/20261016-ryzen5600g/matrix-studio-fechado-r1/` |
| matrix-studio-fechado | 2 | 71.67 / 106.32 | 850 | 5.3 | 572 | 15 | `perf/20261016-ryzen5600g/matrix-studio-fechado-r2/` |
| matrix-studio-fechado | 3 | 71.1 / 103.54 | 915 | 5.8 | 566 | 16 | `perf/20261016-ryzen5600g/matrix-studio-fechado-r3/` |
| clock_mono-studio-fechado | 1 | 7.07 / 11.09 | 701 | 0.2 | 518 | 15 | `perf/20261016-ryzen5600g/clock_mono-studio-fechado-r1/` |
| clock_mono-studio-fechado | 2 | 11.09 / 82.21 | 699 | 0.2 | 518 | 15 | `perf/20261016-ryzen5600g/clock_mono-studio-fechado-r2/` |
| clock_mono-studio-fechado | 3 | 6.93 / 13.2 | 699 | 0.2 | 518 | 15 | `perf/20261016-ryzen5600g/clock_mono-studio-fechado-r3/` |
| aurora-pausado | 1 | 0.44 / 0.98 | 435 | 0.0 | 266 | 7 | `perf/20261016-ryzen5600g/aurora-pausado-r1/` |
| aurora-pausado | 2 | 0.57 / 1.45 | 435 | 0.0 | 266 | 7 | `perf/20261016-ryzen5600g/aurora-pausado-r2/` |
| aurora-pausado | 3 | 0.72 / 1.96 | 435 | 0.0 | 266 | 7 | `perf/20261016-ryzen5600g/aurora-pausado-r3/` |

Leitura (somente o que os números sustentam):
- Shaders (`aurora`, `plasma_flow`): cerca de 3% de um núcleo e 14 a 16% de GPU com 3 telas a 30 fps.
- `clock_mono` (web leve): 7 a 11% de um núcleo (a repetição 2 teve um pico de p95 82%, provavelmente ruído do sistema).
- **`matrix` (web com áudio): cerca de 71% de um núcleo**, o mais caro de todos, três vezes consistentes. É o primeiro candidato a otimização.
- Pausado (manual): 0,4 a 0,7% de CPU e 0% de GPU, o que confirma que a pausa de fato para o desenho. A memória
  continua alta porque os processos (inclusive WebView2 do Studio) seguem vivos.
- A memória privada de ~430 MB com shader inclui o WebView2 do Studio oculto; não é só custo do wallpaper.

Limites desta rodada: uma única máquina; amostras de ~3,2 s efetivas (18 a 19 por medição); sem os cenários
"Studio aberto" e "desktop coberto", sem vídeo e sem o modo desempenho (não há comando no pipe para ligá-lo);
sem comparação com o controle "sem wallpaper"; sem bateria; o usuário seguiu usando o PC (ruído).

### Matrix: canvas 2D contra WebGL (2026-10-16, mais tarde no mesmo dia)

O `matrix` foi reescrito em WebGL (simulação na CPU, desenho e brilho na GPU, quatro passadas de tela cheia) no
lugar de milhares de `drawImage` em canvas 2D mais um filtro CSS de desfoque em tela cheia. Medição alternada
(antigo, novo, antigo, novo), mesma sessão, motor reiniciado a cada troca, 3 telas com o mesmo wallpaper, 30 fps,
soma de CPU de **todos** os processos da árvore (% de um núcleo), 30 s depois de 20 s de aquecimento:

| Versão | Medição 1 | Medição 2 |
| --- | --- | --- |
| canvas 2D (antiga) | 198 | 263 |
| WebGL (nova) | 111 | 96 |

Ou seja, algo como 2 a 2,5 vezes menos CPU nesta máquina, mas **o ruído é grande** (a antiga variou de 198 a
281 e a nova de 75 a 111 entre rodadas). Este método (soma por tipo de processo, janela de 30 s) é diferente do
`perf-sample.ps1`, e a medição da antiga feita de manhã (71%) **não é comparável** com estas: no mesmo dia a mesma
versão antiga mediu 71 e depois 198 a 281, sem que eu saiba explicar (carga do sistema, GPU ativa ou estado do
WebView2). Trate a tabela de Resultados acima como foto daquele momento e use só comparações alternadas como esta.
Brutos da rodada da manhã com a versão nova (rodada de `perf-sample`, também ruidosa): `perf/20261016-matrix-otimizado/`
(CPU 88 a 94%, GPU 8,6%).
O custo restante está quase todo no processo de GPU e nos renderizadores do WebView2 (3 telas), não no motor.

## Comparar com outros programas

Só é válido com **conteúdo equivalente** (mesma cena, resolução, taxa de quadros e áudio) e nas mesmas condições
(Studio/janela de controle do outro programa também aberta ou fechada). Sem isso, não compare e não faça
afirmações de "mais leve que". Ferramentas de terceiros têm seus próprios subprocessos: meça a árvore completa.

## Tolerâncias

Ainda **não definidas**. Depois de pelo menos 3 repetições por cenário, definimos limites por tipo de wallpaper e
passamos a tratá-los como gate (por exemplo, "regressão de CPU maior que X% em relação à mediana anterior"),
anotando a variação entre repetições para o limite não ser menor que o ruído.

## Modo desempenho

Ajustes › Modo desempenho: **Desligado**, **Automático** (padrão) ou **Sempre**. Também há o item
"Modo desempenho (sempre)" no menu da bandeja, que alterna Sempre/Automático.

Ativo (Sempre, ou Automático com um app em tela cheia, detectado por `SHQueryUserNotificationState`,
o mesmo sinal que o Windows usa para segurar notificações), o motor:
- não renderiza em nenhum monitor (o wallpaper fica no último quadro; WebViews ocultas);
- para a captura de áudio e deixa dormindo os leitores de mídia e de sistema;
- acorda uma vez por segundo (só para reavaliar o modo e atender o Studio);
- roda com prioridade ociosa e EcoQoS, e devolve a memória do processo ao Windows.

Limites: um jogo em janela sem borda não conta como tela cheia para o Windows; nesse caso vale a
pausa por cobertura. Com "Sempre" ligado, trocar de wallpaper só aparece ao sair do modo. Ainda não há medição do modo desempenho.
