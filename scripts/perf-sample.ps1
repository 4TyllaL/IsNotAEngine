<#
.SYNOPSIS
  Amostra o custo TOTAL de um processo e de seus descendentes (CPU, memória, GPU) durante N segundos.

.DESCRIPTION
  Mede o que o usuário paga de verdade: o IsNotAEngine.exe MAIS os subprocessos do WebView2 (msedgewebview2.exe)
  que ele criou. Não inicia nem altera nada: você abre o cenário (wallpaper, Studio aberto/fechado, desktop
  coberto) e roda este script. Veja docs/performance.md para o roteiro e as limitações.

  Saída: um CSV por amostra e um resumo (média, p95, máximo) em tela e em <Out>\summary.json.
  Colunas de GPU vêm dos contadores "GPU Engine" e "GPU Process Memory" do Windows; se não existirem
  (driver antigo, contadores desligados), ficam vazias e o resumo diz "indisponível". Nada é estimado.

  Compatível com Windows PowerShell 5.1 e PowerShell 7.
#>
[CmdletBinding()]
param(
    [string]$ProcessName = 'IsNotAEngine',
    [int]$Seconds = 60,
    [int]$IntervalMs = 1000,
    [int]$WarmupSeconds = 15,
    [string]$Out = '',
    [string]$Label = ''
)

$ErrorActionPreference = 'Stop'
if (-not $Out) { $Out = Join-Path (Get-Location) ("perf-run-" + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

function Get-Tree([int]$rootPid) {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
    $ids = New-Object 'System.Collections.Generic.HashSet[int]'
    [void]$ids.Add($rootPid)
    $grew = $true
    while ($grew) {
        $grew = $false
        foreach ($p in $all) {
            if (-not $ids.Contains([int]$p.ProcessId) -and $ids.Contains([int]$p.ParentProcessId)) {
                [void]$ids.Add([int]$p.ProcessId); $grew = $true
            }
        }
    }
    return $ids
}

function Get-GpuCounters([System.Collections.Generic.HashSet[int]]$ids) {
    # Devolve @{ util = soma de utilização 3D (%); mem = soma de memória dedicada (bytes) } ou $null se indisponível.
    try {
        $eng = (Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop).CounterSamples
        $mem = (Get-Counter '\GPU Process Memory(*)\Dedicated Usage' -ErrorAction Stop).CounterSamples
    } catch { return $null }
    $util = 0.0; $bytes = 0.0
    foreach ($s in $eng) {
        if ($s.InstanceName -match '^pid_(\d+)_.*engtype_(3D|Compute|VideoDecode|VideoProcessing|Copy)') {
            if ($ids.Contains([int]$Matches[1])) { $util += $s.CookedValue }
        }
    }
    foreach ($s in $mem) {
        if ($s.InstanceName -match '^pid_(\d+)_') { if ($ids.Contains([int]$Matches[1])) { $bytes += $s.CookedValue } }
    }
    return @{ util = $util; mem = $bytes }
}

$root = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Sort-Object StartTime | Select-Object -First 1
if (-not $root) { throw "processo '$ProcessName' não está em execução. Abra o cenário e rode de novo." }
$cores = [Environment]::ProcessorCount

Write-Host "Aquecendo por $WarmupSeconds s (descartado)..."
Start-Sleep -Seconds $WarmupSeconds

$csv = Join-Path $Out 'samples.csv'
't_s,processes,cpu_pct_of_one_core,private_mb,working_set_mb,gpu_util_pct,gpu_dedicated_mb' | Out-File $csv -Encoding ascii
$ci = [System.Globalization.CultureInfo]::InvariantCulture   # CSV com ponto decimal em qualquer idioma
$first = $true
$prev = @{}   # pid -> TotalProcessorTime
$prevT = Get-Date
$rows = @()
$end = (Get-Date).AddSeconds($Seconds)
$t0 = Get-Date
while ((Get-Date) -lt $end) {
    Start-Sleep -Milliseconds $IntervalMs
    $ids = Get-Tree $root.Id
    $procs = @(Get-Process -Id ([int[]]$ids) -ErrorAction SilentlyContinue)
    $now = Get-Date
    $dt = ($now - $prevT).TotalSeconds
    $cpuDelta = 0.0
    foreach ($p in $procs) {
        $cur = $p.TotalProcessorTime.TotalSeconds
        if ($prev.ContainsKey($p.Id)) { $cpuDelta += [Math]::Max(0.0, $cur - $prev[$p.Id]) }
        $prev[$p.Id] = $cur
    }
    $prevT = $now
    if ($first) { $first = $false; continue }   # a 1ª passada só fixa a linha de base de CPU
    $cpu = if ($dt -gt 0) { 100.0 * $cpuDelta / $dt } else { 0.0 }
    $priv = ($procs | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB
    $ws = ($procs | Measure-Object WorkingSet64 -Sum).Sum / 1MB
    $gpu = Get-GpuCounters $ids
    $row = [pscustomobject]@{
        t = [Math]::Round(($now - $t0).TotalSeconds, 1); n = $procs.Count; cpu = [Math]::Round($cpu, 2)
        priv = [Math]::Round($priv, 1); ws = [Math]::Round($ws, 1)
        gpu = if ($gpu) { [Math]::Round($gpu.util, 2) } else { $null }
        vram = if ($gpu) { [Math]::Round($gpu.mem / 1MB, 1) } else { $null }
    }
    $rows += $row
    [string]::Format($ci, "{0},{1},{2},{3},{4},{5},{6}", $row.t, $row.n, $row.cpu, $row.priv, $row.ws, $row.gpu, $row.vram) | Out-File $csv -Append -Encoding ascii
}

function Get-Stats($values) {
    $v = @($values | Where-Object { $_ -ne $null } | Sort-Object)
    if ($v.Count -eq 0) { return 'indisponível' }
    $p95 = $v[[Math]::Min($v.Count - 1, [int][Math]::Ceiling(0.95 * $v.Count) - 1)]
    return [ordered]@{ mean = [Math]::Round(($v | Measure-Object -Average).Average, 2); p95 = $p95; max = $v[-1]; n = $v.Count }
}

$summary = [ordered]@{
    label = $Label
    process = $ProcessName
    root_pid = $root.Id
    logical_cores = $cores
    seconds = $Seconds
    interval_ms = $IntervalMs
    warmup_seconds = $WarmupSeconds
    started = $t0.ToString('s')
    max_processes_seen = ($rows | Measure-Object n -Maximum).Maximum
    cpu_pct_of_one_core = Get-Stats ($rows | ForEach-Object { $_.cpu })
    private_mb = Get-Stats ($rows | ForEach-Object { $_.priv })
    working_set_mb = Get-Stats ($rows | ForEach-Object { $_.ws })
    gpu_util_pct = Get-Stats ($rows | ForEach-Object { $_.gpu })
    gpu_dedicated_mb = Get-Stats ($rows | ForEach-Object { $_.vram })
    effective_interval_s = if ($rows.Count -gt 1) { [Math]::Round(($rows[-1].t - $rows[0].t) / ($rows.Count - 1), 2) } else { $null }
    notes = 'CPU em % de UM núcleo (100 = um núcleo inteiro). GPU soma os engines 3D/Compute/Vídeo/Copy dos processos medidos; não é frame time.'
}
$summary | ConvertTo-Json -Depth 4 | Out-File (Join-Path $Out 'summary.json') -Encoding utf8
$summary | ConvertTo-Json -Depth 4
Write-Host "Resultados em $Out"
