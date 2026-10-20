<#
.SYNOPSIS
  Monta o pacote MSIX do IsNotAEngine (para enviar ao Partner Center, que assina).
.EXAMPLE
  pwsh packaging/msix/build.ps1            # usa a identidade reservada da Store (valores padrão acima)
.NOTES
  Requer o Windows SDK (makeappx.exe) e `cargo build --release` já executado.
  Name/Publisher: copie de Partner Center > Identidade do produto. A versão vem do Cargo.toml (X.Y.Z -> X.Y.Z.0;
  a Store reserva o 4º número para si, por isso termina em 0).
#>
param(
  # Identidade reservada no Partner Center (Gerenciamento de produtos > Identidade do produto).
  [string]$IdentityName = 'AtyllaAzevedo.IsNotAEngine',
  [string]$Publisher = 'CN=9ED996AC-5110-4B53-8411-D03A0B8DC429',
  [string]$PublisherDisplay = 'Atylla Azevedo',
  [string]$OutDir = 'dist'
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$cargo = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($cargo -notmatch '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"') { throw 'versão não encontrada no Cargo.toml' }
$version = "$($Matches[1]).0"

$exe = Join-Path $root 'target\release\IsNotAEngine.exe'
if (-not (Test-Path $exe)) { throw 'rode `cargo build --release` antes' }
$makeappx = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" -ErrorAction SilentlyContinue |
  Sort-Object FullName -Descending | Select-Object -First 1
if (-not $makeappx) { throw 'makeappx.exe não encontrado (instale o Windows SDK)' }

$stage = Join-Path $root "$OutDir\msix-stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path "$stage\Assets" -Force | Out-Null
Copy-Item $exe $stage
# Vídeos de fábrica, se houver (vídeo + .json de metadados).
$videos = Join-Path $root 'assets\wallpapers\videos'
$media = Get-ChildItem $videos -File -ErrorAction SilentlyContinue | Where-Object { $_.Extension -match '^\.(mp4|webm|mov|mkv|wmv|avi|json)$' }
if ($media) {
  New-Item -ItemType Directory -Path "$stage\videos" -Force | Out-Null
  $media | Copy-Item -Destination "$stage\videos"
  Write-Host "Vídeos de fábrica: $(@($media | Where-Object Extension -ne '.json').Count)"
}

# Logos gerados a partir do ícone da marca (centralizados em fundo transparente).
Add-Type -AssemblyName System.Drawing
$icon = [System.Drawing.Image]::FromFile((Join-Path $root 'assets\brand\icon-256.png'))
function New-Logo([string]$name, [int]$w, [int]$h) {
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.InterpolationMode = 'HighQualityBicubic'; $g.SmoothingMode = 'HighQuality'
  $side = [int]([Math]::Min($w, $h) * 0.8)
  $g.DrawImage($icon, [int](($w - $side) / 2), [int](($h - $side) / 2), $side, $side)
  $g.Dispose(); $bmp.Save("$stage\Assets\$name.png", [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
}
New-Logo 'Square44x44Logo' 44 44
New-Logo 'Square150x150Logo' 150 150
New-Logo 'Wide310x150Logo' 310 150
New-Logo 'StoreLogo' 50 50
$icon.Dispose()

$xml = (Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw).
  Replace('@IDENTITY_NAME@', $IdentityName).Replace('@PUBLISHER@', $Publisher).
  Replace('@PUBLISHER_DISPLAY@', $PublisherDisplay).Replace('@VERSION@', $version)
Set-Content (Join-Path $stage 'AppxManifest.xml') $xml -Encoding utf8

$out = Join-Path $root "$OutDir\IsNotAEngine-$version-x64.msix"
& $makeappx.FullName pack /d $stage /p $out /o
if ($LASTEXITCODE -ne 0) { throw 'makeappx falhou' }
Write-Host "Pacote: $out (sem assinatura: envie ao Partner Center, que assina)"
