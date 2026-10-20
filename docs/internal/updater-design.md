# Atualização verificada: projeto (D03)

Estado: **projeto, nada implementado**. Este documento existe para ser revisado antes de qualquer código.
Não há atualizador no app hoje: quem quer uma versão nova baixa o instalador da página de releases.

## Objetivo e não objetivos

**Objetivo:** o app avisa que há uma versão nova e, **quando o usuário mandar**, baixa, verifica e instala,
de modo que um arquivo adulterado nunca seja executado e uma falha no meio não estrague a instalação.

**Não objetivos (por enquanto):** atualização silenciosa em segundo plano, atualização parcial/delta,
canais (beta/nightly), instalação sem a permissão do usuário, e telemetria. Atualizar nunca muda a
configuração do usuário (`config.toml`).

## Modelo de ameaça

| Ameaça | Resposta |
| --- | --- |
| Rede hostil (alguém entre o usuário e o servidor) troca o instalador | Manifesto assinado + hash do pacote dentro dele; HTTPS é defesa extra, não a base. |
| Servidor/CDN ou conta do GitHub comprometidos | Sem a **chave de assinatura do manifesto** (offline, fora do CI) o atacante não consegue produzir um manifesto válido. |
| Hash publicado junto do arquivo | **Não autentica o publicador** (quem troca o arquivo troca o hash). Por isso o hash só vale dentro do manifesto assinado. |
| Servir uma versão antiga e vulnerável (downgrade/replay) | Contador monotônico assinado e recusa de versões menores que a instalada. |
| Download cortado ou corrompido | Instala só depois de arquivo completo e hash conferido; o temporário nunca substitui nada. |
| Chave de assinatura vazada | Rotação por raiz de confiança (abaixo); o que mitiga é a chave nunca estar no CI. |
| Servidor lento/indisponível | O app segue funcionando; a verificação é opcional e com prazo curto. |

Fora do alcance: um atacante que já executa código com a conta do usuário (ele pode trocar o `.exe`
diretamente), e a segurança da máquina de quem assina.

## Peças

### 1. Raiz de confiança (no executável)

- Uma ou duas **chaves públicas Ed25519 embutidas no binário** (`const`). Elas validam o manifesto.
- É uma chave **própria do projeto**, separada do certificado Authenticode (`docs/signing.md`): o
  Authenticode prova o publicador ao Windows; a chave do manifesto prova ao próprio app que a atualização é legítima.
- A chave privada fica **offline** (token/máquina do mantenedor), **nunca** em segredo do CI: o CI publica
  os arquivos, uma pessoa assina o manifesto. Isso é o que protege contra um repositório comprometido.
- **Rotação:** o app aceita manifestos assinados por qualquer chave da lista embutida. Para trocar a chave,
  uma versão assinada pela chave antiga passa a embutir a nova; só depois a antiga é aposentada. Se a chave
  vazar sem rotação prévia, os usuários precisam reinstalar manualmente (risco assumido e documentado).

### 2. Manifesto assinado

`update.json` (UTF-8) + `update.json.sig` (assinatura Ed25519 de bytes exatos do arquivo, base64):

```json
{
  "schema": 1,
  "product": "IsNotAEngine",
  "version": "0.2.0",
  "counter": 7,
  "published": "2026-11-01T12:00:00Z",
  "min_supported": "0.1.0",
  "notes_url": "https://exemplo/releases/v0.2.0",
  "artifacts": [
    {
      "kind": "installer",
      "arch": "x64",
      "url": "https://exemplo/releases/download/v0.2.0/IsNotAEngine-Setup-0.2.0.exe",
      "size": 4321000,
      "sha256": "<64 hex>"
    }
  ]
}
```

- **`counter`**: inteiro que só cresce a cada release assinada. O app guarda o maior `counter` já visto e
  **recusa qualquer manifesto com `counter` menor ou igual ao da versão instalada** (impede downgrade e
  replay de manifesto antigo). A comparação de versões sozinha não basta, pois um atacante pode reapresentar
  um manifesto antigo válido.
- **`min_supported`**: se a versão instalada for menor, o app avisa que precisa de uma instalação manual
  (por exemplo, mudança de formato de configuração que o instalador novo não migra).
- **`size` e `sha256`** são conferidos depois do download; `size` também limita quanto se baixa.
- O manifesto tem teto de tamanho (64 KiB) e o `schema` desconhecido é recusado (nunca "tentar entender").
- URLs só `https://`, e o host precisa estar na lista de hosts permitidos embutida no app (para um manifesto
  assinado por engano não apontar para um domínio arbitrário). Redirecionamentos só dentro dessa lista.

### 3. Fluxo no app

1. **Verificar** (a pedido, ou 1 vez por dia se o usuário ligar; **desligado por padrão**): baixa
   `update.json` e `update.json.sig` com prazo curto e sem enviar nada além da requisição HTTP (sem
   identificador, sem telemetria; o servidor vê o IP e a versão no `User-Agent`, e isso é dito na interface).
2. **Validar** o manifesto: assinatura contra a raiz embutida → `schema`/`product` → `counter` maior que o
   instalado → `min_supported` → arquitetura. Qualquer falha termina em "nenhuma atualização" com o motivo no log.
3. **Perguntar**: mostra versão, tamanho e link das notas. **Nada é baixado sem o usuário pedir.**
4. **Baixar** para uma pasta temporária própria (`%LOCALAPPDATA%\IsNotAEngine\updates\<version>.part`) com
   limite de bytes igual a `size`; ao terminar, renomeia para o nome final **só depois** de conferir `size` e `sha256`.
5. **Verificar de novo no momento de instalar** (o arquivo pode ter sido trocado em disco entre o download e o clique):
   recalcula o SHA-256 do arquivo que será executado.
6. **Instalar**: executa o instalador baixado (o Inno Setup já fecha o app, mantém `config.toml` e a pasta de
   dados) com confirmação do usuário. Se o instalador for assinado (D02), o app também confere a assinatura
   Authenticode antes de executar; isso é camada extra, não substitui o manifesto.
7. **Rollback:** a atualização é só rodar o instalador novo por cima; antes, o app guarda uma cópia da versão
   instalada (`IsNotAEngine.exe`) e do `config.toml` em `updates\previous\`. Se o app novo **não iniciar** (ou o
   usuário pedir), uma opção "restaurar versão anterior" recoloca a cópia. Um download incompleto ou um hash
   que não bate **nunca** chega a este passo, então nada da instalação atual foi tocado.

### 4. O que nunca acontece

- Executar ou instalar um arquivo cujo hash não bateu, cujo manifesto não tem assinatura válida ou cujo
  `counter` não é maior.
- Substituir a instalação por um download incompleto.
- Instalar sem o usuário confirmar.
- Aceitar um host de download fora da lista permitida.

## Decisões em aberto (precisam do dono do projeto)

1. **Biblioteca Ed25519.** Não há uma no `Cargo.toml`. Opções: `ed25519-dalek` (madura, mais dependências) ou
   verificar com a API do Windows (CNG) sem dependência nova. A escolha altera o `Cargo.lock`; fica para a implementação.
2. **Onde hospedar** `update.json`. GitHub Releases funciona, mas o app passaria a depender de uma URL fixa;
   e o manifesto precisa ser assinado por uma pessoa depois do CI publicar os arquivos (fluxo manual no início).
3. **Quem guarda a chave privada** e o plano de recuperação se ela for perdida.
4. **Política de verificação automática:** desligada por padrão (recomendado) ou ligada.
5. **Cliente HTTP:** `ureq` já está no projeto (usado pela IA); reusá-lo evita dependência nova, mas precisa de
   teto de bytes e prazo próprios para esse uso.

## Divisão da implementação (cada item revisável e testável sozinho)

| ID sugerido | Entrega | Teste principal |
| --- | --- | --- |
| U-D03a | Módulo `update::manifest`: parse estrito, limites, validação de `schema`/`counter`/`min_supported`/hosts, **sem rede** | Tabela de manifestos válidos e adulterados (assinatura trocada, `counter` baixo, host fora da lista, JSON gigante) |
| U-D03b | Verificação de assinatura com a raiz embutida e suporte a duas chaves (rotação) | Vetores de teste com chaves geradas só para teste |
| U-D03c | Download com limite de bytes, `.part`, conferência de `size` e `sha256`, renomear só no sucesso | Servidor local falso: corte no meio, arquivo maior, hash errado |
| U-D03d | Ferramenta de assinatura offline (`scripts/sign-update.ps1` ou subcomando) e geração do manifesto a partir de `dist/` | Manifesto gerado verifica com a chave pública de teste |
| U-D03e | Interface no Studio (verificar, ver notas, baixar, instalar, restaurar) e a cópia de rollback | Roteiro manual, inclusive falha de rede e cancelamento |
| U-D03f | Integração com a assinatura Authenticode do D02 e orientação no instalador | Instalador falso não assinado é recusado quando o canal exige assinatura |

Cada item deve trazer testes negativos (arquivo adulterado nunca executa, download incompleto nunca substitui
a instalação), como pede o gate do roadmap.
