param(
    [ValidateSet('run', 'check', 'test', 'build', 'release', 'demo', 'screenshot')]
    [string]$Task = 'run'
)
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $projectRoot

# Prefer the complete VS 2022 toolchain; the machine also has an incomplete VS 18 installation.
$devShells = @(
    'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Launch-VsDevShell.ps1',
    'C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\Launch-VsDevShell.ps1'
)
foreach ($devShell in $devShells) {
    if (Test-Path -LiteralPath $devShell) {
        & $devShell -SkipAutomaticLocation -Arch amd64 -HostArch amd64 | Out-Null
        break
    }
}
switch ($Task) {
    'run' { cargo run --locked }
    'check' { cargo clippy --locked --all-targets -- -D warnings }
    'test' { cargo test --locked }
    'build' { cargo build --locked }
    'release' {
        cargo build --locked --release
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        New-Item -ItemType Directory -Path (Join-Path $projectRoot 'dist') -Force | Out-Null
        $manifestText = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw
        $appVersion = [regex]::Match($manifestText, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
        $releaseDirectory = Join-Path $projectRoot "dist\RPShop-$appVersion-windows-x64"
        New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\rpshop.exe') -Destination (Join-Path $releaseDirectory 'rpshop.exe') -Force
        try {
            Copy-Item -LiteralPath (Join-Path $releaseDirectory 'rpshop.exe') -Destination (Join-Path $projectRoot 'dist\rpshop.exe') -Force
        } catch {
            Write-Warning "旧版程序正在使用中；新版已生成：$releaseDirectory\rpshop.exe"
        }
    }
    'demo' { cargo run --locked -- --export-demo examples }
    'screenshot' { cargo run --locked -- --screenshot docs/screenshots/editor.png }
}
exit $LASTEXITCODE
