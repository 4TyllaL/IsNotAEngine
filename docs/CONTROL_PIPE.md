# Canal de controle (named pipe)

A engine expõe `\\.\pipe\IsNotAEngine.Control.v1` para docks, widgets e scripts do próprio usuário lerem o estado e enviarem comandos. Código: `src/control/`.

## Segurança

- Só o dono do processo e SYSTEM acessam (DACL `D:P(A;;GA;;;OW)(A;;GA;;;SY)`); clientes remotos são recusados.
- A primeira instância é criada com `FILE_FLAG_FIRST_PIPE_INSTANCE`, então outro processo não consegue se passar pelo servidor.
- No máximo 8 clientes simultâneos; linha de requisição com mais de 4096 bytes derruba a conexão.
- `set_wallpaper` aceita só id do catálogo ou arquivo existente (endereços da internet não são aceitos), as mesmas regras da bandeja.
- Se o pipe não puder ser criado (por exemplo, já existe), a engine registra um aviso e segue sem o canal.

## Protocolo

UTF-8, uma mensagem JSON por linha (`\n`).

| Requisição | Efeito |
|---|---|
| `{"cmd":"state"}` | Devolve o estado atual. |
| `{"cmd":"list_wallpapers"}` | Devolve `{"ok":true,"wallpapers":[...]}`: o catálogo embutido e os arquivos importados (até 500). Cada item tem `id`, `title`, `category`, `kind` (`shader`, `web`, `video`, `livewallpaper`, `image`), `audio` e `source` (`catalog` ou `imported`). O `id` vale em `set_wallpaper`. |
| `{"cmd":"set_setting","name":"fps","value":60}` | Muda um ajuste. Valor fora da faixa ou nome desconhecido volta como `{"ok":false,"error":"..."}` e nada muda. Nomes e faixas na tabela abaixo. |
| `{"cmd":"subscribe"}` | Responde `ok` e passa a empurrar eventos de estado. A conexão vira só de eventos; use outra para comandos. |
| `{"cmd":"pause"}` / `{"cmd":"resume"}` / `{"cmd":"toggle_pause"}` | Pausa manual. |
| `{"cmd":"next_wallpaper"}` | Avança a playlist agora. |
| `{"cmd":"open_studio"}` | Abre o Studio. |
| `{"cmd":"set_wallpaper","monitor":0,"wallpaper":"matrix"}` | Aplica no monitor; sem `monitor`, aplica em todos. |

Ajustes aceitos por `set_setting`:

| Nome | Valor | Faixa |
|---|---|---|
| `fps` | inteiro | 5 a 240 |
| `speed` | número | 0.1 a 3.0 |
| `pause_covered` | booleano | pausa quando o desktop está coberto |
| `pause_battery` | booleano | pausa na bateria |
| `mouse` | booleano | interação com o mouse |
| `parallax` | booleano | parallax |
| `playlist_enabled` | booleano | liga ou desliga a playlist |
| `playlist_minutes` | inteiro | 1 a 1440 |
| `playlist_shuffle` | booleano | ordem aleatória |

O Studio não valida esses campos; o pipe sim (um FPS 0, por exemplo, dividiria por zero no timer do loop). Início com o Windows, cor de destaque e tela de bloqueio ficam só no Studio.

Resposta: `{"ok":true,"state":{...}}` ou `{"ok":false,"error":"..."}`. Em comandos, `ok` significa **aceito e enfileirado**: o `state` devolvido é o anterior ao efeito. Para ver o resultado, assine eventos.

Evento: `{"event":"state","state":{...}}`, enviado só quando algo muda (pausa, algum ajuste ou o wallpaper de algum monitor). Sem polling: a engine compara por tick, sem alocar quando nada mudou.

Estado:

```json
{"v":1,"paused":false,"auto_paused":false,"fps":60,"speed":1.0,
 "pause_covered":true,"pause_battery":false,"mouse":true,"parallax":true,
 "playlist_enabled":false,"playlist_minutes":10,"playlist_shuffle":false,
 "monitors":[{"id":0,"wallpaper":"matrix"}]}
```

`paused` é a pausa manual; `auto_paused` é a automática (desktop coberto ou bateria). A versão do protocolo (`v`) sobe, e o nome do pipe muda junto, só em quebra de compatibilidade.

## Teste rápido (PowerShell)

```powershell
$p = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'IsNotAEngine.Control.v1', 'InOut')
$p.Connect(2000)
$w = New-Object System.IO.StreamWriter($p); $w.AutoFlush = $true
$r = New-Object System.IO.StreamReader($p)
$w.WriteLine('{"cmd":"state"}'); $r.ReadLine()
```

## Testes

- `cargo test control` roda os testes do protocolo.
- `cargo test control::pipe -- --ignored` roda o teste de ida e volta no pipe real; feche a engine antes, porque o nome do pipe é fixo.
