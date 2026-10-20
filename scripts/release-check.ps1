<#
.SYNOPSIS
  Confere a consistência de uma release do IsNotAEngine (versões, arquivos, somas e inicialização).

.DESCRIPTION
  -Phase pre   Antes de compilar: a versão do Cargo.toml, do instalador (.iss) e da tag (se houver) concordam.
  -Phase post  Depois de empacotar: o instalador, o zip portátil, as somas SHA-256 e o inventário de
               dependências existem e batem; o .exe do zip inicia fora da pasta do projeto.
  -RequireSigned  Exige assinatura Authenticode válida no instalador e no .exe do zip (só use quando o
               release foi assinado; sem credenciais o release sai NÃO assinado e isto fica desligado).
  -SelfTest    Prova que as verificações detectam versão divergente, arquivo ausente e soma errada
               (usa arquivos falsos; não compila nada).

  Compatível com Windows PowerShell 5.1 e PowerShell 7.
#>
[CmdletBinding()]
param(
    [ValidateSet('pre', 'post')][string]$Phase = 'pre',
    [string]$Tag = '',
    [string]$DistDir = 'dist',
    [string]$Root = '',
    [switch]$SelfTest,
    [switch]$RequireSigned
)

$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path) }
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Get-CargoVersion([string]$root) {
    $inPackage = $false
    foreach ($line in Get-Content (Join-Path $root 'Cargo.toml')) {
        if ($line -match '^\s*\[(.+)\]\s*$') { $inPackage = ($Matches[1] -eq 'package'); continue }
        if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    throw 'version não encontrada em [package] do Cargo.toml'
}

function Get-IssVersion([string]$root) {
    foreach ($line in Get-Content (Join-Path $root 'installer\IsNotAEngine.iss')) {
        if ($line -match '#define\s+AppVersion\s+"([^"]+)"') { return $Matches[1] }
    }
    throw 'AppVersion padrão não encontrada no instalador'
}

# Versão "de lançamento" de uma tag: v0.2.0 -> 0.2.0 (sufixos de pré-lançamento são ignorados).
function ConvertTo-ReleaseVersion([string]$tag) { return ($tag.TrimStart('v') -replace '-.*$', '') }

function Test-Versions([string]$cargo, [string]$iss, [string]$tag) {
    $errors = @()
    if ($iss -ne $cargo) { $errors += "instalador (.iss) usa $iss, mas o Cargo.toml usa $cargo" }
    if ($tag) {
        $t = ConvertTo-ReleaseVersion $tag
        if ($t -ne $cargo) { $errors += "a tag $tag pede $t, mas o Cargo.toml usa $cargo" }
    }
    return $errors
}

function Test-Dist([string]$dist, [string]$version) {
    $errors = @()
    $setup = Join-Path $dist "IsNotAEngine-Setup-$version.exe"
    $zip = Join-Path $dist "IsNotAEngine-$version-portable.zip"
    $sums = Join-Path $dist 'SHA256SUMS.txt'
    $deps = Join-Path $dist 'DEPENDENCIES.txt'
    foreach ($f in @($setup, $zip, $sums, $deps)) {
        if (-not (Test-Path $f)) { $errors += "arquivo ausente: $(Split-Path -Leaf $f)" }
        elseif ((Get-Item $f).Length -eq 0) { $errors += "arquivo vazio: $(Split-Path -Leaf $f)" }
    }
    if (Test-Path $zip) {
        $arc = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path $zip).Path)
        try {
            $names = $arc.Entries | ForEach-Object { $_.Name }
            foreach ($need in @('IsNotAEngine.exe', 'README.md')) {
                if ($names -notcontains $need) { $errors += "o zip portátil não contém $need" }
            }
        } finally { $arc.Dispose() }
    }
    if (Test-Path $sums) {
        $listed = @{}
        foreach ($line in Get-Content $sums) {
            if ($line -match '^([0-9a-f]{64})\s+(.+)$') { $listed[$Matches[2].Trim()] = $Matches[1] }
        }
        foreach ($f in Get-ChildItem $dist -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' }) {
            if (-not $listed.ContainsKey($f.Name)) { $errors += "SHA256SUMS.txt não lista $($f.Name)"; continue }
            $actual = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
            if ($actual -ne $listed[$f.Name]) { $errors += "soma SHA-256 diferente para $($f.Name)" }
        }
    }
    return $errors
}

# Assinatura Authenticode válida em cada arquivo (cadeia confiável e carimbo de tempo quando houver).
function Test-Signed([string[]]$files) {
    $errors = @()
    foreach ($f in $files) {
        if (-not (Test-Path $f)) { $errors += "arquivo a verificar não existe: $f"; continue }
        $sig = Get-AuthenticodeSignature -FilePath $f
        if ($sig.Status -ne 'Valid') { $errors += "assinatura inválida ou ausente em $(Split-Path -Leaf $f): $($sig.Status)" }
    }
    return $errors
}

# Inicia o .exe do zip fora da pasta do projeto: sem desktop, só o que não depende de arquivos ao lado.
function Test-Smoke([string]$dist, [string]$version, [bool]$requireSigned = $false) {
    $errors = @()
    $zip = Join-Path $dist "IsNotAEngine-$version-portable.zip"
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("inae-smoke-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir | Out-Null
    try {
        [System.IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path $zip).Path, $dir)
        $exe = Join-Path $dir 'IsNotAEngine.exe'
        if ($requireSigned) { $errors += Test-Signed @($exe) }
        Push-Location $dir
        try {
            $help = & $exe --mcp-config
            if ($LASTEXITCODE -ne 0 -or -not $help) { $errors += '--mcp-config falhou fora da pasta do projeto' }
            $req = '{"v":1,"name":"smoke.hlsl","source":"float4 ps_main(float4 p : SV_Position) : SV_Target { return float4(1,0,0,1); }"}'
            $reply = $req | & $exe --compile-worker
            if ($LASTEXITCODE -ne 0 -or ($reply -join '') -notmatch '"ok":\s*true') { $errors += 'o worker de compilação não respondeu ok fora da pasta do projeto' }
        } finally { Pop-Location }
    } catch {
        $errors += "falha ao iniciar o pacote portátil: $($_.Exception.Message)"
    } finally {
        Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    }
    return $errors
}

function Write-Result([string[]]$errors, [string]$ok) {
    if ($errors.Count -gt 0) {
        $errors | ForEach-Object { Write-Host "ERRO: $_" }
        exit 1
    }
    Write-Host $ok
}

function Invoke-SelfTest {
    $failures = @()
    $expect = { param($name, $errs, $shouldFail)
        $failed = ($errs.Count -gt 0)
        if ($failed -ne $shouldFail) { $script:failures += "autoteste '$name': esperava falha=$shouldFail, veio $failed ($($errs -join '; '))" }
    }
    $script:failures = @()
    & $expect 'versões iguais' (Test-Versions '0.2.0' '0.2.0' 'v0.2.0') $false
    & $expect 'tag com sufixo' (Test-Versions '0.2.0' '0.2.0' 'v0.2.0-rc1') $false
    & $expect 'tag divergente' (Test-Versions '0.2.0' '0.2.0' 'v0.3.0') $true
    & $expect 'instalador divergente' (Test-Versions '0.2.0' '0.1.0' '') $true
    $unsigned = Join-Path ([System.IO.Path]::GetTempPath()) ("inae-unsigned-" + [guid]::NewGuid().ToString('N') + '.exe')
    Set-Content $unsigned 'nao e um executavel assinado'
    & $expect 'arquivo sem assinatura' (Test-Signed @($unsigned)) $true
    & $expect 'arquivo inexistente na checagem de assinatura' (Test-Signed @($unsigned + '.nao')) $true
    Remove-Item $unsigned -ErrorAction SilentlyContinue

    $v = '9.9.9'
    $d = Join-Path ([System.IO.Path]::GetTempPath()) ("inae-selftest-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $d | Out-Null
    try {
        $stage = Join-Path $d 'stage'; New-Item -ItemType Directory -Path $stage | Out-Null
        Set-Content (Join-Path $stage 'IsNotAEngine.exe') 'x'; Set-Content (Join-Path $stage 'README.md') 'x'
        $dist = Join-Path $d 'dist'; New-Item -ItemType Directory -Path $dist | Out-Null
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath (Join-Path $dist "IsNotAEngine-$v-portable.zip")
        Set-Content (Join-Path $dist "IsNotAEngine-Setup-$v.exe") 'setup'
        Set-Content (Join-Path $dist 'DEPENDENCIES.txt') 'dep 1.0'
        $lines = Get-ChildItem $dist -File | ForEach-Object { "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)" }
        $lines | Set-Content (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
        & $expect 'dist completo' (Test-Dist $dist $v) $false

        Set-Content (Join-Path $dist 'DEPENDENCIES.txt') 'adulterado'
        & $expect 'soma errada' (Test-Dist $dist $v) $true
        Set-Content (Join-Path $dist 'DEPENDENCIES.txt') 'dep 1.0'
        & $expect 'dist restaurado' (Test-Dist $dist $v) $false

        Remove-Item (Join-Path $dist "IsNotAEngine-Setup-$v.exe")
        & $expect 'instalador ausente' (Test-Dist $dist $v) $true
        & $expect 'versão do nome diferente' (Test-Dist $dist '1.2.3') $true

        $bad = Join-Path $d 'bad'; New-Item -ItemType Directory -Path $bad | Out-Null
        Set-Content (Join-Path $bad 'IsNotAEngine.exe') 'x'
        $badDist = Join-Path $d 'dist2'; New-Item -ItemType Directory -Path $badDist | Out-Null
        Compress-Archive -Path (Join-Path $bad '*') -DestinationPath (Join-Path $badDist "IsNotAEngine-$v-portable.zip")
        & $expect 'zip sem README' (Test-Dist $badDist $v) $true
    } finally {
        Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
    }
    return $script:failures
}

if ($SelfTest) {
    Write-Result (Invoke-SelfTest) 'autoteste ok: divergências de versão, arquivos ausentes e somas erradas são detectados'
    exit 0
}

$cargo = Get-CargoVersion $Root
if ($Phase -eq 'pre') {
    Write-Result (Test-Versions $cargo (Get-IssVersion $Root) $Tag) "versões consistentes ($cargo)"
} else {
    $dist = if ([System.IO.Path]::IsPathRooted($DistDir)) { $DistDir } else { Join-Path $Root $DistDir }
    $errs = @(Test-Dist $dist $cargo)
    if ($RequireSigned) { $errs += Test-Signed @((Join-Path $dist "IsNotAEngine-Setup-$cargo.exe")) }
    if ($errs.Count -eq 0) { $errs = @(Test-Smoke $dist $cargo ([bool]$RequireSigned)) }
    Write-Result $errs "release $cargo consistente: arquivos, somas e inicialização conferidos"
}
