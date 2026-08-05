$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

$Root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("wg-installer-windows-test-" + [Guid]::NewGuid().ToString("N"))
$Target = "x86_64-pc-windows-msvc"

function Fail([string]$Message) { throw $Message }
function Assert-File([string]$Path) { if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { Fail "expected file: $Path" } }
function Assert-NoFile([string]$Path) { if (Test-Path -LiteralPath $Path) { Fail "unexpected file: $Path" } }

function New-TestRelease {
    param([string]$Dir, [string]$Version, [string]$Channel, [string]$Label)
    $archiveRoot = "wg-v$Version-$Target"
    $stage = Join-Path $Dir $archiveRoot
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    foreach ($name in @("worksgood.exe", "wg.exe", "nex.exe")) {
        Set-Content -LiteralPath (Join-Path $stage $name) -Value "$name $Label" -NoNewline
    }
    Set-Content -LiteralPath (Join-Path $stage "LICENSE") -Value "test license"
    Set-Content -LiteralPath (Join-Path $stage "README-install.txt") -Value "test readme"
    $archive = "$archiveRoot.zip"
    Compress-Archive -LiteralPath $stage -DestinationPath (Join-Path $Dir $archive) -Force
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Dir $archive)).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $Dir "SHA256SUMS") -Value "$digest  $archive"
    $manifest = [ordered]@{
        schema_version = 1
        package = "workgraph"
        release_name = "wg"
        version = $Version
        tag = "v$Version"
        channel = $Channel
        binaries = @("worksgood", "wg", "nex")
        assets = @([ordered]@{ target = $Target; archive = $archive; sha256 = $digest })
    } | ConvertTo-Json -Depth 5
    Set-Content -LiteralPath (Join-Path $Dir "release-manifest.json") -Value $manifest
}

function Invoke-Installer {
    param(
        [string]$TestHome,
        [string[]]$Arguments,
        [bool]$ExpectSuccess = $true
    )
    New-Item -ItemType Directory -Force -Path $TestHome | Out-Null
    $oldHome = $env:HOME
    $oldProfile = $env:USERPROFILE
    try {
        $env:HOME = $TestHome
        $env:USERPROFILE = $TestHome
        $output = & pwsh -NoLogo -NoProfile -File (Join-Path $Root "scripts/install-wg.ps1") @Arguments 2>&1
        $code = $LASTEXITCODE
    } finally {
        $env:HOME = $oldHome
        $env:USERPROFILE = $oldProfile
    }
    if ($ExpectSuccess -and $code -ne 0) { Fail "installer failed ($code): $($output -join [Environment]::NewLine)" }
    if (-not $ExpectSuccess -and $code -eq 0) { Fail "installer unexpectedly succeeded: $($output -join [Environment]::NewLine)" }
    return [pscustomobject]@{ Code = $code; Output = ($output -join [Environment]::NewLine) }
}

try {
    New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
    $releaseOne = Join-Path $Tmp "release-one"
    $releaseTwo = Join-Path $Tmp "release-two"
    New-Item -ItemType Directory -Force -Path $releaseOne, $releaseTwo | Out-Null
    New-TestRelease $releaseOne "0.99.0" "stable" "first"
    New-TestRelease $releaseTwo "1.2.3" "nightly" "second"

    # PowerShell variable names are case-insensitive, so `$home` aliases the
    # read-only automatic `$HOME` variable. Use a distinct name on every host.
    $testHome = Join-Path $Tmp "home"
    $install = Join-Path $testHome "bin"
    $result = Invoke-Installer $testHome @("-BaseUrl", $releaseOne, "-InstallDir", $install, "-Target", $Target, "-Channel", "stable")
    foreach ($name in @("worksgood.exe", "wg.exe", "nex.exe")) { Assert-File (Join-Path $install $name) }
    Assert-File (Join-Path $testHome ".wg/install-receipt.toml")
    if ((Get-Content -LiteralPath (Join-Path $testHome ".wg/install-receipt.toml") -Raw) -notmatch 'binaries = \["worksgood", "wg", "nex"\]') { Fail "receipt missing exact binary set" }

    $dryHome = Join-Path $Tmp "dry-home"
    $dryInstall = Join-Path $dryHome "bin"
    $dry = Invoke-Installer $dryHome @("-BaseUrl", $releaseOne, "-InstallDir", $dryInstall, "-Target", $Target, "-Channel", "stable", "-DryRun")
    foreach ($name in @("worksgood.exe", "wg.exe", "nex.exe")) { Assert-NoFile (Join-Path $dryInstall $name) }
    if ($dry.Output -notmatch 'would install worksgood\.exe, wg\.exe, and nex\.exe') { Fail "dry-run omitted exact binary set: $($dry.Output)" }

    Invoke-Installer $testHome @("-BaseUrl", $releaseTwo, "-InstallDir", $install, "-Target", $Target, "-Channel", "nightly") | Out-Null
    if ((Get-Content -LiteralPath (Join-Path $install "worksgood.exe") -Raw) -ne "worksgood.exe second") { Fail "upgrade did not replace worksgood.exe" }

    $foreignHome = Join-Path $Tmp "foreign-home"
    $foreignInstall = Join-Path $foreignHome "bin"
    New-Item -ItemType Directory -Force -Path $foreignInstall | Out-Null
    Set-Content -LiteralPath (Join-Path $foreignInstall "worksgood.exe") -Value "foreign" -NoNewline
    $foreign = Invoke-Installer $foreignHome @("-BaseUrl", $releaseOne, "-InstallDir", $foreignInstall, "-Target", $Target) $false
    if ($foreign.Output -notmatch 'refusing to overwrite') { Fail "foreign collision was not explicit" }
    if ((Get-Content -LiteralPath (Join-Path $foreignInstall "worksgood.exe") -Raw) -ne "foreign") { Fail "foreign command changed" }

    $uninstallDry = Invoke-Installer $testHome @("-InstallDir", $install, "-Target", $Target, "-Uninstall", "-DryRun")
    foreach ($name in @("worksgood.exe", "wg.exe", "nex.exe")) {
        Assert-File (Join-Path $install $name)
        if ($uninstallDry.Output -notmatch [Regex]::Escape("would remove " + (Join-Path $install $name))) { Fail "uninstall dry-run omitted $name" }
    }
    Invoke-Installer $testHome @("-InstallDir", $install, "-Target", $Target, "-Uninstall") | Out-Null
    foreach ($name in @("worksgood.exe", "wg.exe", "nex.exe")) { Assert-NoFile (Join-Path $install $name) }

    Write-Host "PASS: Windows PowerShell installer installs, dry-runs, upgrades, collision-refuses, receipts, and uninstalls exact worksgood/wg/nex set"
} finally {
    Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
