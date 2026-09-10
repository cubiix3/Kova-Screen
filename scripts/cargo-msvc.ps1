#Requires -Version 5.1
<#
.SYNOPSIS
    Runs a cargo command inside a Visual Studio x64 dev shell.

.DESCRIPTION
    Kova links against the Windows C++ runtime, so cargo needs the LIB/INCLUDE
    environment set by the Visual Studio developer batch files. This script
    discovers a complete installation - Visual Studio or the standalone Build
    Tools - initializes its x64 environment and runs the requested cargo
    command.

    It avoids calling the system `cmd` command because some environments have a
    Node wrapper at `cmd` that breaks argument parsing.

.EXAMPLE
    .\scripts\cargo-msvc.ps1 check --workspace
    .\scripts\cargo-msvc.ps1 test --workspace -- --test-threads=1
    .\scripts\cargo-msvc.ps1 build --release
#>
param(
    [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
    [string[]]$CargoArgs
)

$ErrorActionPreference = "Stop"

function Find-VsVarsBatch {
    # vcvarsall.bat is the real entry point; vcvars64.bat is a two-line wrapper
    # that calls it. An interrupted Visual Studio update can leave the wrapper
    # in place while vcvarsall.bat is gone, so probe the real file and skip
    # installations that cannot actually configure the environment.
    $installerRoot = ${env:ProgramFiles(x86)}
    if ($installerRoot) {
        $vswhere = Join-Path $installerRoot "Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path -LiteralPath $vswhere) {
            $installations = & $vswhere -latest -prerelease -products * `
                -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
                -property installationPath
            foreach ($installation in @($installations)) {
                if (-not $installation) { continue }
                $candidate = Join-Path $installation "VC\Auxiliary\Build\vcvarsall.bat"
                if (Test-Path -LiteralPath $candidate) {
                    return $candidate
                }
            }
        }
    }

    # Fallback for installs whose vswhere is missing or only reports broken
    # instances. Visual Studio changed its installation layout over time:
    # classic releases live under a four-digit year folder ("2022"), newer ones
    # under a version-number folder ("18"), and the Build Tools land in the
    # 32-bit Program Files root. Probe every installed root and edition instead
    # of hard-coding a single layout.
    $roots = @()
    foreach ($programFiles in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if (-not $programFiles) { continue }
        $vsDir = Join-Path $programFiles "Microsoft Visual Studio"
        if (Test-Path -LiteralPath $vsDir) {
            $roots += Get-ChildItem -LiteralPath $vsDir -Directory |
                Sort-Object Name -Descending |
                ForEach-Object { $_.FullName }
        }
    }
    foreach ($root in $roots) {
        foreach ($edition in @("Community", "Professional", "Enterprise", "BuildTools", "Preview")) {
            $candidate = Join-Path $root "$edition\VC\Auxiliary\Build\vcvarsall.bat"
            if (Test-Path -LiteralPath $candidate) {
                return $candidate
            }
        }
    }
    throw "vcvarsall.bat not found. Install Visual Studio or the Visual Studio Build Tools with the Desktop development with C++ workload."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$vcvars = Find-VsVarsBatch

# Capture environment *before* running vcvars so we can diff it afterwards.
$before = @{}
foreach ($var in [Environment]::GetEnvironmentVariables("Process").Keys) {
    $before[$var] = [Environment]::GetEnvironmentVariable($var, "Process")
}

# Use the legacy Windows command processor directly via its absolute path.
$cmdExe = "$env:SystemRoot\system32\cmd.exe"
$envDump = & $cmdExe /c """$vcvars"" x64 1>nul 2>nul & set" 2>$null
$after = @{}
foreach ($line in $envDump) {
    if ($line -match "^(\w+)=(.*)$") {
        $after[$matches[1]] = $matches[2]
    }
}

# Apply all new or changed variables to the current PowerShell process.
foreach ($key in $after.Keys) {
    if ($before[$key] -ne $after[$key]) {
        [Environment]::SetEnvironmentVariable($key, $after[$key], "Process")
    }
}

Write-Host "Visual Studio x64 environment loaded from: $vcvars" -ForegroundColor Cyan

# If the user typed `cargo-msvc.ps1 cargo test ...`, drop the leading "cargo".
if ($CargoArgs[0] -eq "cargo") {
    $CargoArgs = $CargoArgs[1..($CargoArgs.Length - 1)]
}

$ErrorActionPreference = "Continue"
& cargo @CargoArgs 2>&1 | ForEach-Object { $_.ToString() }
exit $LASTEXITCODE