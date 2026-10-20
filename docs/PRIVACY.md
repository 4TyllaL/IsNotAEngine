# Política de privacidade do IsNotAEngine

Última atualização: 2026-10-16 · Versão do app: 1.0.0 · Responsável: 4TyllaL

## Resumo
O IsNotAEngine **não coleta, não envia e não vende dados pessoais**. Não há conta, anúncios nem telemetria.

## O que fica no seu computador
- Configurações e wallpapers importados: `%LOCALAPPDATA%\IsNotAEngine`.
- Logs locais, quando você liga o console de logs nos Ajustes. Eles não são enviados a ninguém.
- A chave de API do "Criar com IA", se você cadastrar uma, fica no **Gerenciador de Credenciais do Windows**.

## O que o app lê do seu computador
- **Áudio que está tocando** (WASAPI loopback), só para animar o wallpaper. Nada é gravado nem enviado. O microfone não é usado.
- **Música em reprodução** (título e artista, via Windows) e **métricas do sistema** (uso de CPU, memória etc.), apenas para wallpapers do catálogo que as mostram. Wallpapers importados não recebem esses dados.
- Estado do Windows (bateria, jogo em tela cheia, janela cobrindo a tela) para pausar sozinho.

## Rede
O app só usa a internet no recurso **opcional "Criar com IA"**: o texto que você escreve é enviado ao provedor que você escolheu
(Anthropic, Google ou OpenRouter), com a **sua** chave, para gerar um wallpaper. Esse envio segue a política do provedor.
Wallpapers web rodam sem acesso à internet.

## Crianças
O app não coleta dados de ninguém, incluindo crianças.

## Contato
Abra uma issue no repositório do projeto ou use o contato da página do produto.
