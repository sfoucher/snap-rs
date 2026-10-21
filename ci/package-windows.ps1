# Builds dist\snap-rs-<version>.msi from the pixi environment.
#   powershell -File ci\package-windows.ps1
#   ci\package-windows.ps1 -SkipBuild
# Needs the WiX CLI on PATH. Use v5: dotnet tool install --global wix --version 5.0.2
# (v7 refuses to run until you accept the paid-support OSMF EULA.)
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$envDir = Join-Path $root '.pixi\envs\default'
$bin = Join-Path $envDir 'Library\bin'
$stage = Join-Path $root 'dist\app'
$version = (Select-String -Path 'crates\snap-gui\Cargo.toml' -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

if (-not $SkipBuild) {
    pixi run cargo build --release -p snap-gui
    if ($LASTEXITCODE) { throw 'cargo build failed' }
}
$exe = Join-Path $root 'target\release\snap-gui.exe'
if (-not (Test-Path $exe)) { throw "missing $exe" }

# Names of the DLLs a PE file imports (normal and delay-load tables).
function Get-Imports([string]$path) {
    $b = [IO.File]::ReadAllBytes($path)
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    $nsec = [BitConverter]::ToUInt16($b, $pe + 6)
    $opt = $pe + 24
    if ([BitConverter]::ToUInt16($b, $opt) -ne 0x20B) { throw "$path is not PE32+" }
    $secs = $opt + [BitConverter]::ToUInt16($b, $pe + 20)
    function Rva2Off([uint32]$rva) {
        for ($i = 0; $i -lt $nsec; $i++) {
            $s = $secs + 40 * $i
            $va = [BitConverter]::ToUInt32($b, $s + 12)
            $raw = [BitConverter]::ToUInt32($b, $s + 16)
            $sz = [Math]::Max([BitConverter]::ToUInt32($b, $s + 8), $raw)
            if ($rva -ge $va -and $rva -lt $va + $sz) { return [int]($rva - $va + [BitConverter]::ToUInt32($b, $s + 20)) }
        }
        -1
    }
    function CStr([int]$off) {
        $e = $off
        while ($b[$e]) { $e++ }
        [Text.Encoding]::ASCII.GetString($b, $off, $e - $off)
    }
    # data directory 1 = import (20-byte entries, name at +12); 13 = delay import (32-byte, name at +4)
    foreach ($d in @(@(1, 20, 12), @(13, 32, 4))) {
        $rva = [BitConverter]::ToUInt32($b, $opt + 112 + 8 * $d[0])
        if (-not $rva) { continue }
        $off = Rva2Off $rva
        while ($off -ge 0 -and [BitConverter]::ToUInt32($b, $off + $d[2]) -ne 0) {
            CStr (Rva2Off ([BitConverter]::ToUInt32($b, $off + $d[2])))
            $off += $d[1]
        }
    }
}

if (Test-Path 'dist') { Remove-Item 'dist' -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $exe $stage

# Copy every imported DLL that lives in the conda env, transitively. System DLLs
# (kernel32, api-ms-win-*, ucrtbase) are not in Library\bin or are skipped, so they stay system-provided.
$seen = @{}
$queue = [Collections.Generic.Queue[string]]::new()
$queue.Enqueue($exe)
while ($queue.Count) {
    foreach ($name in Get-Imports $queue.Dequeue()) {
        $key = $name.ToLower()
        if ($seen[$key] -or $key -like 'api-ms-win-*' -or $key -eq 'ucrtbase.dll') { continue }
        $seen[$key] = $true
        $src = Join-Path $bin $name
        if (Test-Path $src) {
            Copy-Item $src $stage
            $queue.Enqueue($src)
        }
    }
}

# Portable layout read by use_bundled_data(): <exe dir>\share\{gdal,proj}.
$share = Join-Path $stage 'share'
New-Item -ItemType Directory $share | Out-Null
foreach ($d in 'gdal', 'proj') { Copy-Item (Join-Path $envDir "Library\share\$d") $share -Recurse }

"staged $((Get-ChildItem $stage -Recurse -File).Count) files, $([Math]::Round((Get-ChildItem $stage -Recurse -File | Measure-Object Length -Sum).Sum / 1MB)) MB"

if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    throw 'wix not found. Install it with: dotnet tool install --global wix --version 5.0.2'
}
$msi = "dist\snap-rs-$version.msi"
wix build installer\snap-rs.wxs -arch x64 -d "Version=$version" -d "StageDir=$stage" -o $msi
if ($LASTEXITCODE) { throw 'wix build failed' }
"built $msi"
