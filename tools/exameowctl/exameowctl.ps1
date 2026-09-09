# exameowctl.ps1 - Windows launcher for the exameowctl study bridge CLI.
#
# For each of EXAMEOW_BASE_URL / EXAMEOW_TOKEN this launcher reads the
# persistent Windows environment (User scope first, then Machine scope when
# the User value is empty) and injects the resolved value into the current
# process ($env:) before starting Python. Resolved values are never echoed,
# printed, or logged.
#
# Usage (double-dash flags only, so PowerShell does not try to bind them):
#   .\exameowctl.ps1 status
#   .\exameowctl.ps1 feed --subject "信息新技术" --limit 10 --json
#   .\exameowctl.ps1 ack 42 --subject "信息新技术"
#   .\exameowctl.ps1 bank validate example_bank.json --json

[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CliArgs
)

$ErrorActionPreference = 'Stop'

function Get-ExameowScopedEnvValue {
    param([string]$Name)
    # 1) Windows User scope (HKCU\Environment); 2) Machine scope if User is empty.
    $value = $null
    try {
        $value = [Environment]::GetEnvironmentVariable($Name, 'User')
    } catch {
        $value = $null
    }
    if ([string]::IsNullOrWhiteSpace($value)) {
        try {
            $value = [Environment]::GetEnvironmentVariable($Name, 'Machine')
        } catch {
            $value = $null
        }
    }
    return $value
}

foreach ($name in @('EXAMEOW_BASE_URL', 'EXAMEOW_TOKEN')) {
    $value = Get-ExameowScopedEnvValue -Name $name
    if (-not [string]::IsNullOrWhiteSpace($value)) {
        [Environment]::SetEnvironmentVariable($name, $value, 'Process')
    }
}

# UTF-8 output regardless of the console code page (bank content has CJK text).
$env:PYTHONUTF8 = '1'

$script = Join-Path $PSScriptRoot 'exameowctl.py'

function Test-PythonCandidate {
    # Verifies the interpreter actually runs (a stale py launcher config or the
    # Microsoft Store python.exe stub both "exist" but cannot execute scripts).
    param([string]$Executable, [string[]]$PreArgs)
    $ErrorActionPreference = 'Continue'  # keep native stderr redirects non-fatal (PS 5.1)
    & $Executable @PreArgs -c 'import sys' 1>$null 2>$null
    return ($LASTEXITCODE -eq 0)
}

# Prefer the py launcher (python.org installs), fall back to python on PATH.
if (Get-Command -Name 'py' -ErrorAction SilentlyContinue) {
    if (Test-PythonCandidate -Executable 'py' -PreArgs @('-3')) {
        & py -3 $script @CliArgs
        exit $LASTEXITCODE
    }
}

if (Get-Command -Name 'python' -ErrorAction SilentlyContinue) {
    if (Test-PythonCandidate -Executable 'python' -PreArgs @()) {
        & python $script @CliArgs
        exit $LASTEXITCODE
    }
}

Write-Error "Python 3 was not found on PATH. Install Python 3.8+ from https://www.python.org/ and tick 'Add python.exe to PATH'."
exit 127
