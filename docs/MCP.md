# Servidor MCP (criar wallpapers com IA)

`IsNotAEngine.exe --mcp` é um servidor [MCP](https://modelcontextprotocol.io) sobre stdio. Ele deixa um cliente de IA (Claude Code, Claude Desktop, Gemini CLI, Cursor...) **criar, testar e aplicar wallpapers** usando o PSL (Procedural Shader Lab) da própria engine. Código: `src/mcp/`.

**Quem paga:** o servidor roda na máquina do usuário e **não chama nenhuma IA**: ele só oferece ferramentas. Quem escreve o shader é o cliente de IA, com a assinatura ou a chave que o usuário já tem. Nenhuma chave passa pela engine. Cada geração gasta a cota do plano do usuário.

## Instalação

```
IsNotAEngine.exe --mcp-config
```

imprime os comandos prontos com o caminho do seu executável. Resumo:

- **Claude Code:** `claude mcp add isnotaengine -- "C:\caminho\IsNotAEngine.exe" --mcp`
- **Claude Desktop, Cursor e outros:** na seção `mcpServers` do JSON de configuração:
  ```json
  "isnotaengine": { "command": "C:\\caminho\\IsNotAEngine.exe", "args": ["--mcp"] }
  ```
- **Gemini CLI:** a mesma entrada em `~/.gemini/settings.json`.

Depois é só pedir: *"crie um wallpaper de oceano à noite com lua e ondas lentas"*. Compilar, renderizar e salvar funcionam sem o app aberto; **aplicar** exige o app aberto (usa o [canal de controle](CONTROL_PIPE.md)).

## Ferramentas

| Ferramenta | O que faz |
|---|---|
| `get_guide` | Entrega o guia de autoria: contrato de shader (`shader`), wallpapers web (`web`) ou o fluxo recomendado (`workflow`). A IA lê antes de escrever. |
| `psl_compile` | Compila HLSL ou GLSL (Shadertoy) e devolve erros com número de linha (do GLSL do autor, não do convertido) ou OK com avisos e parâmetros `// param:`. |
| `psl_render` | Compila e renderiza até 6 quadros (até 1920×1080) como imagens, com métricas objetivas: ms de GPU, brilho, pixels pretos/estourados, movimento entre quadros e *hints* em texto ("muito escuro", "quase sem movimento", "caro: 7 ms a 1080p"). Saída determinística. |
| `render_still` | Renderiza um quadro em até 7680×4320 e salva como PNG: wallpaper de imagem estática (sem renderização contínua). Devolve o caminho e uma miniatura. |
| `web_preview` | Renderiza uma página HTML com o mesmo WebView2 e a mesma política sem rede do desktop e devolve capturas em instantes à escolha. Mostra, por exemplo, um `fetch` bloqueado. |
| `save_wallpaper` | Salva um shader (precisa compilar) ou uma página HTML na pasta de importados. Nunca sobrescreve um wallpaper diferente. |
| `apply_wallpaper` | Aplica por id do catálogo ou caminho, em um monitor ou em todos. |
| `list_wallpapers` | Lista o catálogo e os importados com os ids. |

## Segurança

- **Salvar só dentro de `%LOCALAPPDATA%\IsNotAEngine\wallpapers`**, com o nome saneado (`file_stem`); nunca um caminho escolhido pela IA.
- **Limites:** fonte ≤ 256 KiB, resolução ≤ 7680×4320, prévias ≤ 1920×1080 e 6 quadros, compilação com timeout de 30 s, páginas web ≤ 256 KiB e esperas somando ≤ 60 s.
- **Páginas web** rodam sem rede (`connect-src 'none'`), como no desktop.
- **Shaders rodam na GPU do usuário.** Não há como abortar um desenho longo: um laço enorme pode causar um reset do driver (TDR) do Windows, que afeta todos os apps por um instante. O app detecta a perda do dispositivo e se recupera sozinho (recuperação de GPU), mas a tela pode piscar. O guia pede laços pequenos e limitados.
- **Pipe de controle:** só o dono do processo acessa; `apply_wallpaper` passa pelas mesmas validações da bandeja.
- O cliente de IA normalmente pede aprovação antes de cada ferramenta nova.
- O stdout é exclusivo do protocolo; logs saem no stderr.

## Testes

`cargo test mcp` cobre o protocolo (handshake, erros JSON-RPC, notificações), as ferramentas, os limites e a conversão de números de linha. Teste manual com um cliente real: configure como acima e peça um wallpaper.
