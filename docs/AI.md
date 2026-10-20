# Criar com IA (dentro do Studio)

A aba **Criar com IA** do Studio deixa um modelo de linguagem fazer o wallpaper: você descreve ("oceano à noite com lua cheia, ondas lentas"), a IA escreve o shader ou a página web, **renderiza, olha o resultado e corrige** (em geral 3 a 6 rodadas) e salva. Você aplica o que gostar. É o mesmo laço do [servidor MCP](MCP.md), com as mesmas ferramentas, mas dentro do app. Código: `src/ai/`.

## Provedores

| Provedor | Chave | Observação |
|---|---|---|
| **Claude** (Anthropic) | console.anthropic.com | Padrão `claude-opus-5-5`. Lê imagens. |
| **Gemini** (Google) | aistudio.google.com | Tem camada gratuita, com limites baixos de uso. **No uso gratuito o Google pode usar o que você envia para melhorar os produtos dele**: não escreva dados pessoais. Lê imagens. |
| **OpenRouter** | openrouter.ai | Um endereço, muitos modelos. Nem todo modelo lê imagens: há um interruptor "O modelo entende imagens". |

Você escolhe o modelo: **Atualizar lista** busca, na conta, os que aceitam ferramentas. Para o Claude vem preenchido; para Gemini e OpenRouter a lista é sempre a do próprio provedor, então nunca fica desatualizada.

## Chave de API e privacidade

- A chave fica no **Gerenciador de Credenciais do Windows**, protegida pela sua conta. Nunca vai para o `config.toml`, para o log nem para a página do Studio (a página só sabe se existe uma chave salva).
- **O que sai do computador:** o seu pedido, o código que a IA escreve e os **quadros renderizados** (imagens do que ela fez), para o provedor escolhido, mais as instruções fixas do app (guias de shader e de página web). **A cada rodada a conversa inteira é enviada de novo** (pedido, códigos e imagens anteriores). Nada mais.
- A chave só é enviada ao próprio provedor, pelo cabeçalho da requisição (nunca na URL).

## Segurança e custo

- A IA **não aplica nada**: só você aperta "Aplicar". Ela também não lê nem escolhe caminhos: salva apenas na pasta de wallpapers do app (mesmas regras do MCP).
- **Limites de uma geração (configuráveis em Criar com IA):** rodadas (4 a 60; padrão 24) e tokens lidos + escritos (50 mil a 5 milhões; padrão 1 milhão), além de um teto fixo de 250 mil tokens escritos. Passou de qualquer um, a geração para, a conversa continua e aparece "Continuar de onde parou". **Cancelar** funciona a qualquer momento.
- **Isto limita USO, não dinheiro.** O preço por token varia com o modelo e o provedor, e imagens e a conversa reenviada contam como tokens lidos. O app não consegue converter tokens em reais ou dólares com segurança: confira a cobrança e defina o limite de gastos no painel do próprio provedor. O Claude usa cache de prompt para barateá-la; a linha de uso no fim mostra passos e tokens.

## Nada se perde se parar

- **Salvar é explícito e a IA nunca aplica.** Quem aplica é você ("Aplicar neste monitor" / "Em todos").
- **Cancelar, erro do provedor, recusa ou limite:** o último código que renderizou com sucesso e ainda não foi salvo vira um **rascunho**: aparece no Studio (Copiar código, Abrir no Lab ou Importar como wallpaper) e fica em `%LOCALAPPDATA%\IsNotAEnginei_draftsascunho-<n>.<glsl|hlsl|html>` (os 20 mais recentes). O histórico da conversa volta ao estado anterior em caso de erro, cancelamento ou recusa, e é mantido nos limites.
- **Ajustar:** o wallpaper salvo tem "Ajustar no Lab" (abre o código e os controles de parâmetros) e, depois de salvar no Lab, "Aplicar no desktop" usa os mesmos valores da prévia.
- Falhas de rede, limite de uso (429) e sobrecarga são repetidas sozinhas com espera; erros de chave e de modelo aparecem com explicação.
- Shaders rodam na sua GPU e não dá para abortar um desenho longo (veja a nota de TDR no [MCP](MCP.md)).

## Testes

`cargo test ai::` cobre: montagem e leitura das requisições dos três provedores, o laço do agente (correção de erros, cancelamento, limites de rodadas e de tokens, recusa, volta atrás em falha), o HTTP com repetição (sobrecarga, limite de uso, chave inválida), o Gerenciador de Credenciais e ciclos completos com servidor HTTP local, ferramentas reais e GPU real: sucesso, erro do provedor no meio da geração com o rascunho recuperado, e parada por limite de rodadas e de tokens. Não há teste automático contra as APIs reais dos provedores (exigiria chaves e gastaria cota): confira com a sua chave.
