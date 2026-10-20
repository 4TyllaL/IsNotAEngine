<#
.SYNOPSIS
  Roda o Kit de Certificação de Aplicativos do Windows (WACK) no .msix.
.NOTES
  Precisa de PowerShell COMO ADMINISTRADOR. O appcert.exe se recusa a rodar sem elevação.
  Se o pacote não estiver assinado, o WACK ainda valida o manifesto e o conteúdo; para o teste completo de
  instalação, assine uma cópia com um certificado de teste com o mesmo Publisher do manifesto.
  O relatório (XML) sai em dist\wack.xml. Procure por `OVERALL_RESULT` = PASS e por testes FAIL.
#>
param([string]$Package = (Join-Path $PSScriptRoot '..\..\dist\IsNotAEngine-1.0.0.0-x64.msix'))
$ErrorActionPreference = 'Stop'
$id = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $id.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'abra o PowerShell como administrador' }
$kit = "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\appcert.exe"
if (-not (Test-Path $kit)) { throw 'appcert.exe não encontrado (instale o Windows SDK > App Certification Kit)' }
$pkg = (Resolve-Path $Package).Path
$report = Join-Path (Split-Path $pkg) 'wack.xml'
& $kit reset
& $kit test -appxpackagepath $pkg -reportoutputpath $report
Write-Host "Relatório: $report"
Select-String -Path $report -Pattern 'OVERALL_RESULT' | Select-Object -First 1
