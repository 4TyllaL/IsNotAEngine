# Worker de compilação de shaders (contrato v1)

Estado: contrato (C01) **implementado em `src/compiler_worker.rs`** (C02). Todos os chamadores de
shaders do usuário passam pelo worker: MCP, `--lab`, prévia do Lab no Studio e wallpapers de shader
(via `ShaderPipeline::compile`, só em cache frio). Limitação: o filho é encerrado com `kill()`, sem
Job Object. A seção "Por que" descreve o estado **anterior** à implementação.

## Por que

O `D3DCompile` (FXC) pode demorar muito em loops desenrolados e **não pode ser cancelado** dentro do
processo. Hoje:

- O MCP e o Lab de linha de comando compilam numa `std::thread` e esperam com `recv_timeout`
  (30 s). No timeout só **param de esperar**: a thread continua consumindo CPU e fica pendurada até o
  processo sair.
- A prévia do Lab no Studio (`studio::lab::set_code`) e os wallpapers de shader
  (`ShaderPipeline::compile`, em `wallpaper/shader.rs`) compilam **na própria thread chamadora, sem
  timeout nenhum**. Um shader patológico congela o Studio ou o motor.

A saída é compilar num **processo filho** que possa ser encerrado (`TerminateProcess`).

## Chamadores (a revisar na C02)

| Chamador | Arquivo | Hoje | Deve virar |
| --- | --- | --- | --- |
| MCP `psl_compile` e ferramentas de render | `src/mcp/psl.rs` (`compile`) | thread + timeout 30 s | worker, mesmo `Failure::Timeout` |
| `--lab render|seq|watch` | `src/lab/mod.rs` | thread + timeout 30 s, saída 3 no timeout | worker, mesmas saídas (2 erro, 3 timeout, 4 pipeline) |
| Prévia do Lab no Studio | `src/studio/lab.rs` (`set_code`) | síncrono, sem limite | worker, mantendo o shader anterior em erro |
| Wallpaper de shader (catálogo, `.hlsl`, hot-reload) | `src/wallpaper/shader.rs` | síncrono via `ShaderPipeline::compile` + cache | cache primeiro; só a falha de cache vai ao worker |
| Vídeo | `src/wallpaper/video.rs` | HLSL fixo embutido | **fora do escopo** (fonte confiável, constante) |

`render::pipeline::compile_uncached` continua sendo o núcleo: o worker é só um invólucro de processo
em volta dele. O cache de bytecode (`get_or_compile_bytecode`) fica no processo principal.

## Processo

- Mesmo executável: `IsNotAEngine.exe --compile-worker`. Trata o argumento **antes** de logging,
  mutex de instância única, WebView, áudio ou qualquer acesso ao desktop (como `--mcp` já faz).
- Um pedido por processo, sem estado entre pedidos (simplicidade acima de latência; medir na B01
  antes de pensar em pool).
- Sem rede, sem credenciais e sem desktop: o worker não lê `config.toml`, não abre o Credential
  Manager, não cria janelas e não usa o dispositivo D3D11 (o `D3DCompile` não precisa dele).
  O ambiente do filho deve ser reduzido ao mínimo (`env_clear` + `SystemRoot`/`PATH` se o FXC exigir).
- **Dono do processo:** quem chama (processo principal). Ele cria o filho, escreve o pedido no
  stdin, lê a resposta do stdout e **sempre** coleta o filho (`wait`). Em timeout ou cancelamento:
  `kill()` e `wait()`. Preferir um *Job Object* com `KILL_ON_JOB_CLOSE` para que o filho morra junto
  se o principal cair.
- stderr do filho é ignorado pelo protocolo (só diagnóstico); stdout contém **somente** a resposta.

## Protocolo (v1)

UTF-8, JSON de uma linha por mensagem, terminada em `\n`. Um pedido, uma resposta, depois o filho
sai com código 0.

### Pedido

```json
{"v":1,"name":"shader.hlsl","source":"<HLSL completo, já com common.hlsli>"}
```

- `v`: versão do contrato; o worker recusa versões que não conhece.
- `name`: só para mensagens de erro; sem caminhos (o worker não lê arquivos).
- `source`: o HLSL **final**. A conversão de GLSL, `with_common` e as diretivas `// param:` ficam no
  processo principal (comportamento atual preservado).
- **Limite de entrada:** 256 KiB de fonte (igual a `MAX_SOURCE_BYTES`) e linha de pedido de no
  máximo 300 KiB (escapes JSON). Acima disso o worker responde `rejected` sem compilar.

### Resposta

Sucesso:

```json
{"v":1,"ok":true,"vs":"<base64>","ps":"<base64>","messages":"warnings do compilador","ms":12.5}
```

Erro de compilação (equivale ao `Err(String)` de `compile_uncached`; o texto é o do FXC, e o
principal continua aplicando `clean_messages` com o deslocamento de linhas):

```json
{"v":1,"ok":false,"kind":"compile","error":"shader.hlsl(12,5): error X3004: ..."}
```

Outros `kind`: `rejected` (limite ou versão), `protocol` (JSON inválido).
**Limite de saída:** a resposta lida pelo principal é truncada em 8 MiB; mais que isso é erro
`protocol`.

Códigos de saída: `0` resposta escrita (inclusive `ok:false`); `64` pedido ilegível; qualquer outro
valor ou morte por sinal é tratado pelo principal como falha do worker (`worker_crashed`).

### Erros vistos pelo chamador

O chamador traduz para o que já existe, sem mudar mensagens para o usuário:

| Situação | Resultado |
| --- | --- |
| `ok:true` | `CompiledShader { vs, ps, messages }` |
| `ok:false, kind:compile` | `Err(texto)` / `Failure::Compile` / saída 2 do Lab |
| tempo excedido (30 s) | filho morto e coletado; `Failure::Timeout` / saída 3 |
| `rejected` | `Failure::Rejected` |
| crash, saída inválida, fila cheia | erro explícito (`worker_crashed`, `busy`); **o último wallpaper válido continua** |

## Concorrência e fila (para a C02)

- No máximo **2** workers simultâneos e fila de **4** pedidos; o pedido excedente falha na hora com
  `busy` (melhor que acumular compilações de 30 s). Valores iniciais: ajustar com a B01.
- Cancelamento (nova digitação na prévia, troca de wallpaper) mata o filho do pedido substituído.
- Um timeout não deve impedir a próxima compilação: cada pedido usa um filho novo.

## Compatibilidade

- Para o mesmo `source`, `vs`/`ps` e `messages` devem ser **idênticos** ao `compile_uncached`
  atual (mesmos flags: `D3DCOMPILE_OPTIMIZATION_LEVEL3`, alvos `vs_5_0`/`ps_5_0`,
  `stage_sources`). Teste de regressão: comparar bytes de todos os shaders do catálogo com e sem worker.
- Mudança de campo = nova versão `v`; o principal e o worker são o mesmo executável, então não há
  skew de versão em uso normal.

## Exemplos

Sucesso (resumido):

```text
> {"v":1,"name":"a.hlsl","source":"float4 ps_main(float4 p:SV_Position):SV_Target{return 1;}"}
< {"v":1,"ok":true,"vs":"RFhCQw...","ps":"RFhCQw...","messages":"","ms":3.1}
```

Erro:

```text
> {"v":1,"name":"a.hlsl","source":"float4 ps_main( {"}
< {"v":1,"ok":false,"kind":"compile","error":"a.hlsl(1,22): error X3000: syntax error: unexpected token '{'"}
```

Limite:

```text
> {"v":1,"name":"a.hlsl","source":"<300 KiB>"}
< {"v":1,"ok":false,"kind":"rejected","error":"source is 300 KiB; the limit is 256 KiB"}
```

## Fora do escopo

Sandbox de segurança do processo filho (token restrito / AppContainer) e pool de workers. O ganho
imediato é poder **encerrar** a compilação; endurecer o filho e medir latência são passos separados.

## Divisão sugerida da implementação (C02)

1. `compiler_worker.rs`: entrada `--compile-worker`, parse/serialização do protocolo e testes com
   pipes simulados (sem GPU).
2. Cliente: spawn + Job Object + timeout/kill/wait, fila e limite de concorrência; testes com um
   "worker" falso que demora, devolve lixo, trava ou morre.
3. Integrar chamadores em ordem: MCP, Lab CLI, Lab do Studio, wallpaper de shader.
