<#
.SYNOPSIS
  Assina um arquivo com Authenticode SE as credenciais existirem; caso contrário, avisa e segue sem assinar.

.DESCRIPTION
  Lê as credenciais só de variáveis de ambiente (vindas de segredos do CI), nunca de arquivos do repositório:
    PFX_BASE64        certificado .pfx em base64 (segredo)
    PFX_PASSWORD      senha do .pfx (segredo)
    TIMESTAMP_URL     servidor de carimbo de tempo RFC 3161 (variável, não segredo; opcional)
  Escreve `signed=true|false` em GITHUB_OUTPUT. A falta de credencial NÃO falha o job, mas fica explícita no
  log e no resumo da execução: o release sai sem assinatura. Se as credenciais existem e a assinatura falha,
  o job falha (nunca publica algo que deveria estar assinado e não está).
#>
param([Parameter(Mandatory = $true)][string]$Path)

$ErrorActionPreference = 'Stop'

function Set-Output([string]$value) {
    if ($env:GITHUB_OUTPUT) { "signed=$value" >> $env:GITHUB_OUTPUT }
}

if (-not $env:PFX_BASE64 -or -not $env:PFX_PASSWORD) {
    $msg = "ASSINATURA PULADA para ${Path}: segredos WINDOWS_SIGNING_PFX_BASE64/WINDOWS_SIGNING_PFX_PASSWORD ausentes. O release sai NÃO assinado."
    Write-Host $msg
    if ($env:GITHUB_STEP_SUMMARY) { "> :warning: $msg" >> $env:GITHUB_STEP_SUMMARY }
    Set-Output 'false'
    exit 0
}

if (-not (Test-Path $Path)) { throw "arquivo a assinar não existe: $Path" }
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $signtool) { throw 'signtool.exe não encontrado (Windows SDK)' }

$pfx = Join-Path ([System.IO.Path]::GetTempPath()) ("sign-" + [guid]::NewGuid().ToString('N') + '.pfx')
try {
    [System.IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:PFX_BASE64))
    $ts = if ($env:TIMESTAMP_URL) { $env:TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    & $signtool.FullName sign /f $pfx /p $env:PFX_PASSWORD /fd SHA256 /tr $ts /td SHA256 $Path
    if ($LASTEXITCODE -ne 0) { throw "signtool sign falhou ($LASTEXITCODE) para $Path" }
    & $signtool.FullName verify /pa /q $Path
    if ($LASTEXITCODE -ne 0) { throw "signtool verify falhou ($LASTEXITCODE) para $Path" }
    Write-Host "assinado e verificado: $Path"
    Set-Output 'true'
} finally {
    Remove-Item $pfx -Force -ErrorAction SilentlyContinue
}
