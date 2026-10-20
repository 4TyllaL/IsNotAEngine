# Assinatura dos artefatos (Windows)

Estado: **preparada, não ativa**. O workflow de release já sabe assinar, mas só faz isso se você fornecer
um certificado. Sem ele, o release sai **sem assinatura**, e isso fica escrito no log e no resumo da
execução. Nada aqui foi testado com um certificado real.

## Por que assinar

Sem assinatura, o Windows (SmartScreen) avisa "editor desconhecido" ao abrir o instalador, e antivírus
desconfiam mais de um `.exe` novo. Assinar não deixa o programa mais seguro: só prova quem publicou e que
o arquivo não mudou depois da assinatura. Mesmo assinado, o SmartScreen pode avisar por um tempo até o
certificado ganhar reputação (certificados EV costumam evitar isso; os comuns, não).

## Opções de certificado (você escolhe)

| Opção | Custo / esforço | Observação |
| --- | --- | --- |
| Certificado de assinatura de código (OV/EV) de uma autoridade comercial | pago, anual | Funciona com `signtool` e `.pfx`. Algumas autoridades hoje entregam a chave só em token de hardware ou nuvem, o que não cabe neste fluxo com `.pfx`. |
| Azure Trusted Signing / assinatura em nuvem | assinatura mensal | Exige trocar o passo de `signtool` por uma ação do provedor. **Não implementado aqui.** |
| Certificado autoassinado | grátis | Não é confiável para outras pessoas; serve só para testar o fluxo. |

O repositório implementa o caminho **`.pfx` + `signtool`**. Os outros exigem trabalho adicional.

## O que você precisa fornecer (e onde)

Nada disso entra no repositório. No GitHub: *Settings › Secrets and variables › Actions*.

| Nome | Tipo | Conteúdo |
| --- | --- | --- |
| `WINDOWS_SIGNING_PFX_BASE64` | segredo | O `.pfx` em base64 (`[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`). |
| `WINDOWS_SIGNING_PFX_PASSWORD` | segredo | A senha do `.pfx`. |
| `WINDOWS_SIGNING_TIMESTAMP_URL` | variável (opcional) | Servidor de carimbo de tempo RFC 3161. Padrão: `http://timestamp.digicert.com`. O carimbo mantém a assinatura válida depois que o certificado expira; ele é um serviço de terceiros e recebe o hash do arquivo. |

## Como funciona

Ordem no job `windows` de `.github/workflows/release.yml`:

1. Compila o executável.
2. `scripts/sign-artifact.ps1` assina `IsNotAEngine.exe` (e verifica com `signtool verify /pa`).
3. O Inno Setup gera o instalador **já com o `.exe` assinado**.
4. O instalador é assinado.
5. O `.zip` portátil é montado com o `.exe` assinado.
6. Inventário de dependências e **depois** as somas SHA-256 (assim as somas cobrem os arquivos finais).
7. `scripts/release-check.ps1 -Phase post -RequireSigned` confere que o instalador e o `.exe` do zip têm
   assinatura Authenticode válida (`Get-AuthenticodeSignature`). Se não houve assinatura, a conferência roda
   sem `-RequireSigned`.

Regras do script de assinatura:

- **Sem credenciais:** avisa, escreve `signed=false` e segue (o release sai sem assinatura).
- **Com credenciais e falha** (certificado inválido, servidor de carimbo fora do ar, `signtool` ausente): o
  job **falha**. Nunca publica algo que deveria estar assinado e não está.
- O `.pfx` é gravado num arquivo temporário e apagado ao final; a senha só existe no ambiente do passo.

## Como verificar um arquivo baixado

```powershell
Get-AuthenticodeSignature .\IsNotAEngine-Setup-<versão>.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
Get-FileHash .\IsNotAEngine-Setup-<versão>.exe -Algorithm SHA256   # compare com SHA256SUMS.txt
```

`Status` deve ser `Valid`. O `SHA256SUMS.txt` fica ao lado dos arquivos na mesma página de release, então
**ele sozinho não prova quem publicou**; a assinatura é que prova.

## O que falta

- Um certificado real fornecido por você e um teste de ponta a ponta do workflow (nunca executado num runner).
- Decidir entre `.pfx` e assinatura em nuvem.
- Assinar o `IsNotAEngine.exe` portátil não resolve o SmartScreen do **instalador** sozinho; os dois são
  assinados.
- Atualizador (D03) depende de uma chave de assinatura própria do manifesto, diferente do certificado
  Authenticode (veja `docs/internal/updater-design.md`).
