<#
.SYNOPSIS
  Instala o MSIX no seu PC para testar o app empacotado (inclui "Iniciar com o Windows") e roda o WACK.
.DESCRIPTION
  O pacote que vai para a Store NÃO é assinado (a Store assina). Para instalar localmente, este script:
    1. cria um certificado de teste com o mesmo Publisher do manifesto;
    2. assina uma CÓPIA do pacote (dist\test\) com ele;
    3. confia nesse certificado (LocalMachine\TrustedPeople: precisa de administrador);
    4. instala o pacote para o seu usuário.
  Para desfazer: pwsh packaging\msix\test-install.ps1 -Uninstall
.NOTES
  Rode no PowerShell COMO ADMINISTRADOR, depois de `packaging\msix\build.ps1`.
  O certificado de teste só serve para este PC; nunca use um assim fora de testes.
#>
param(
  [string]$Package = (Join-Path $PSScriptRoot '..\..\dist\IsNotAEngine-1.0.0.0-x64.msix'),
  [switch]$Uninstall,
  [switch]$SkipWack
)
$ErrorActionPreference = 'Stop'
$id = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $id.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'abra o PowerShell como administrador' }
$identity = 'AtyllaAzevedo.IsNotAEngine'
$subject = 'CN=9ED996AC-5110-4B53-8411-D03A0B8DC429'

if ($Uninstall) {
  Get-AppxPackage -Name $identity | Remove-AppxPackage
  Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object { $_.Subject -eq $subject -and $_.FriendlyName -eq 'IsNotAEngine (teste local)' } | Remove-Item
  Write-Host 'Removido.'; return
}

$pkg = (Resolve-Path $Package).Path
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $sdk) { throw 'signtool.exe não encontrado (Windows SDK)' }

$cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature -FriendlyName 'IsNotAEngine (teste local)' `
  -CertStoreLocation 'Cert:\CurrentUser\My' -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
$cer = Join-Path $env:TEMP 'inae-test.cer'
Export-Certificate -Cert $cert -FilePath $cer | Out-Null
Import-Certificate -FilePath $cer -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople' | Out-Null

$dir = Join-Path (Split-Path $pkg) 'test'; New-Item -ItemType Directory -Force $dir | Out-Null
$signed = Join-Path $dir (Split-Path $pkg -Leaf); Copy-Item $pkg $signed -Force
& $sdk.FullName sign /fd SHA256 /sha1 $cert.Thumbprint $signed
if ($LASTEXITCODE -ne 0) { throw 'signtool falhou' }

Get-AppxPackage -Name $identity | Remove-AppxPackage -ErrorAction SilentlyContinue
Add-AppxPackage -Path $signed
Write-Host "Instalado. Abra 'IsNotAEngine' no menu Iniciar. Para testar o StartupTask: Ajustes do app > Iniciar com o Windows, e confira em Configurações do Windows > Aplicativos > Inicialização."

if (-not $SkipWack) {
  $kit = "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\appcert.exe"
  $report = Join-Path $dir 'wack.xml'
  & $kit reset
  & $kit test -appxpackagepath $pkg -reportoutputpath $report
  Select-String -Path $report -Pattern 'OVERALL_RESULT' | Select-Object -First 1
}
