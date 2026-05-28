param(
    [string]$Channel = "",
    [string]$Version = "",
    [string]$InstallDir = "",
    [switch]$DryRun,
    [string]$Repo = "",
    [string]$BaseUrl = "",
    [string]$Target = "",
    [string]$DevDir = "",
    [switch]$Help
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

function Show-Usage {
    @"
Install WG native binaries.

Usage:
  powershell -ExecutionPolicy Bypass -File install-wg.ps1 [options]
  pwsh -File install-wg.ps1 [options]

Options:
  -Channel stable|nightly|dev   Release channel to install (default: stable).
  -Version VERSION              Install an explicit release tag/version.
  -InstallDir DIR               Install wg and nex into DIR.
  -DryRun                       Resolve and print actions without installing.
  -Repo OWNER/REPO              GitHub repository (default: graphwork/wg).
  -BaseUrl URL                  Mirror/test URL containing release-manifest.json,
                                SHA256SUMS, and the target archive.
  -Target TRIPLE                Override detected release target triple.
  -DevDir DIR                   Source checkout for -Channel dev.
  -Help                         Show this help.

Environment variables mirror the options:
  WG_INSTALL_CHANNEL, WG_INSTALL_VERSION, WG_INSTALL_DIR,
  WG_INSTALL_REPO, WG_INSTALL_BASE_URL, WG_INSTALL_TARGET,
  WG_INSTALL_DEV_DIR, WG_INSTALL_DRY_RUN.
"@
}

if ($Help) {
    Show-Usage
    exit 0
}

function Resolve-Option {
    param(
        [string]$Value,
        [string]$EnvName,
        [string]$Default
    )

    if (-not [string]::IsNullOrWhiteSpace($Value)) {
        return $Value
    }

    $envValue = [Environment]::GetEnvironmentVariable($EnvName)
    if (-not [string]::IsNullOrWhiteSpace($envValue)) {
        return $envValue
    }

    return $Default
}

function Write-Info {
    param([string]$Message)
    Write-Host $Message
}

function Write-Warn {
    param([string]$Message)
    Write-Warning $Message
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Test-Command {
    param([string]$Name)
    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Convert-VersionToTag {
    param([string]$Value)
    if ($Value -match '^(v|nightly|release-test-|dry-run-)') {
        return $Value
    }
    return "v$Value"
}

function Trim-TrailingSlash {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ""
    }
    return $Value.TrimEnd("/")
}

function Get-InstallTarget {
    param([string]$Override)

    if (-not [string]::IsNullOrWhiteSpace($Override)) {
        return $Override
    }

    $arch = ""
    $os = ""

    if ($env:OS -eq "Windows_NT") {
        $os = "Windows"
        $arch = $env:PROCESSOR_ARCHITECTURE
        if ($env:PROCESSOR_ARCHITEW6432) {
            $arch = $env:PROCESSOR_ARCHITEW6432
        }
    } elseif (Test-Command uname) {
        $os = (& uname -s).Trim()
        $arch = (& uname -m).Trim()
    } else {
        Fail "unsupported platform: cannot detect OS/architecture"
    }

    switch ($os) {
        "Linux" {
            switch ($arch) {
                { $_ -in @("x86_64", "amd64") } { return "x86_64-unknown-linux-gnu" }
                { $_ -in @("aarch64", "arm64") } { return "aarch64-unknown-linux-gnu" }
                default { Fail "unsupported Linux architecture: $arch" }
            }
        }
        "Darwin" {
            switch ($arch) {
                { $_ -in @("x86_64", "amd64") } { return "x86_64-apple-darwin" }
                { $_ -in @("aarch64", "arm64") } { return "aarch64-apple-darwin" }
                default { Fail "unsupported macOS architecture: $arch" }
            }
        }
        "Windows" {
            switch ($arch) {
                { $_ -in @("AMD64", "x86_64", "amd64") } { return "x86_64-pc-windows-msvc" }
                "ARM64" { Fail "Windows ARM64 native artifacts are not published yet; use the x86_64 PowerShell installer under emulation or build from source with -Channel dev." }
                default { Fail "unsupported Windows architecture: $arch" }
            }
        }
        default {
            Fail "unsupported OS: $os"
        }
    }
}

function Get-ArchiveExt {
    param([string]$Target)
    if ($Target -eq "x86_64-pc-windows-msvc") {
        return ".zip"
    }
    return ".tar.gz"
}

function Get-ExeExt {
    param([string]$Target)
    if ($Target -eq "x86_64-pc-windows-msvc") {
        return ".exe"
    }
    return ""
}

function Get-DefaultInstallDir {
    param([string]$Explicit)

    if (-not [string]::IsNullOrWhiteSpace($Explicit)) {
        return $Explicit
    }

    $homeDir = [Environment]::GetFolderPath("UserProfile")
    if ([string]::IsNullOrWhiteSpace($homeDir)) {
        $homeDir = $HOME
    }

    if ($env:OS -eq "Windows_NT") {
        return (Join-Path $homeDir ".wg\bin")
    }

    $localBin = Join-Path $homeDir ".local/bin"
    if ((Test-Path -LiteralPath $localBin -PathType Container) -and (Test-Path -LiteralPath $localBin -PathType Container)) {
        return $localBin
    }

    $localParent = Join-Path $homeDir ".local"
    if ((Test-Path -LiteralPath $homeDir -PathType Container) -or (Test-Path -LiteralPath $localParent -PathType Container)) {
        return $localBin
    }

    return (Join-Path $homeDir "bin")
}

function Ensure-InstallDir {
    param(
        [string]$Path,
        [bool]$IsDryRun
    )

    if ($IsDryRun) {
        Write-Info "dry-run: would create/use install dir $Path"
        return
    }

    New-Item -ItemType Directory -Force -Path $Path | Out-Null
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        Fail "install dir is not a directory: $Path"
    }
}

function Download-File {
    param(
        [string]$Url,
        [string]$Destination
    )

    if ($Url.StartsWith("file://")) {
        $uri = [Uri]$Url
        Copy-Item -LiteralPath $uri.LocalPath -Destination $Destination -Force
        return
    }

    if (Test-Path -LiteralPath $Url) {
        Copy-Item -LiteralPath $Url -Destination $Destination -Force
        return
    }

    $requestArgs = @{
        Uri = $Url
        OutFile = $Destination
    }
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        $requestArgs.UseBasicParsing = $true
    }
    Invoke-WebRequest @requestArgs
}

function Get-ChecksumForArchive {
    param(
        [string]$ArchiveName,
        [string]$ChecksumsPath
    )

    foreach ($line in Get-Content -LiteralPath $ChecksumsPath) {
        $trimmed = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed)) {
            continue
        }
        $parts = $trimmed -split '\s+'
        if ($parts.Length -lt 2) {
            continue
        }
        $name = $parts[$parts.Length - 1].TrimStart("*")
        if ($name -eq $ArchiveName) {
            return $parts[0].ToLowerInvariant()
        }
    }

    Fail "checksum for $ArchiveName not found in SHA256SUMS"
}

function Verify-Checksum {
    param(
        [string]$ArchiveName,
        [string]$ArchivePath,
        [string]$ChecksumsPath
    )

    $expected = Get-ChecksumForArchive -ArchiveName $ArchiveName -ChecksumsPath $ChecksumsPath
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $ArchivePath).Hash.ToLowerInvariant()

    if ($actual -ne $expected) {
        Fail "checksum verification failed for $ArchiveName: expected $expected, got $actual"
    }

    Write-Info "checksum: OK ($actual)"
    return $actual
}

function Verify-Attestations {
    param(
        [string]$ArchiveName,
        [string]$Repo,
        [string]$BaseUrl,
        [string]$WorkDir
    )

    if (-not (Test-Command gh)) {
        Write-Info "attestation: skipped (gh not installed)"
        return
    }

    if (-not [string]::IsNullOrWhiteSpace($BaseUrl)) {
        Write-Info "attestation: skipped (custom base URL/mirror)"
        return
    }

    Write-Info "attestation: verifying release-manifest.json, SHA256SUMS, and $ArchiveName with gh"
    Push-Location $WorkDir
    try {
        & gh attestation verify release-manifest.json --repo $Repo
        if ($LASTEXITCODE -ne 0) {
            Fail "GitHub attestation verification failed for release-manifest.json"
        }
        & gh attestation verify SHA256SUMS --repo $Repo
        if ($LASTEXITCODE -ne 0) {
            Fail "GitHub attestation verification failed for SHA256SUMS"
        }
        & gh attestation verify $ArchiveName --repo $Repo
        if ($LASTEXITCODE -ne 0) {
            Fail "GitHub attestation verification failed for $ArchiveName"
        }
    } finally {
        Pop-Location
    }
    Write-Info "attestation: OK"
}

function Expand-WgArchive {
    param(
        [string]$ArchivePath,
        [string]$Destination
    )

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    if ($ArchivePath.EndsWith(".zip")) {
        Expand-Archive -LiteralPath $ArchivePath -DestinationPath $Destination -Force
        return
    }

    if (-not (Test-Command tar)) {
        Fail "tar is required to extract $ArchivePath"
    }
    & tar -xzf $ArchivePath -C $Destination
    if ($LASTEXITCODE -ne 0) {
        Fail "failed to extract $ArchivePath"
    }
}

function Install-Binary {
    param(
        [string]$Source,
        [string]$Destination
    )

    $tmp = "$Destination.wg-install.$PID"
    Copy-Item -LiteralPath $Source -Destination $tmp -Force
    if ($env:OS -ne "Windows_NT" -and (Test-Command chmod)) {
        & chmod 0755 $tmp
    }
    Move-Item -LiteralPath $tmp -Destination $Destination -Force
}

function ConvertTo-TomlString {
    param([string]$Value)
    return ($Value.Replace("\", "\\").Replace('"', '\"'))
}

function Write-InstallReceipt {
    param(
        [string]$Version,
        [string]$Channel,
        [string]$Target,
        [string]$InstallDir,
        [string]$ReleaseUrl,
        [string]$ArtifactSha256,
        [string]$ArchiveName,
        [string]$Repo,
        [bool]$IsDryRun
    )

    $receiptPath = Join-Path $HOME ".wg/install-receipt.toml"
    if ($IsDryRun) {
        Write-Info "dry-run: would write receipt $receiptPath"
        return
    }

    $receiptDir = Split-Path -Parent $receiptPath
    New-Item -ItemType Directory -Force -Path $receiptDir | Out-Null
    $installedAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

    $content = @(
        'manager = "wg-installer"',
        ('version = "{0}"' -f (ConvertTo-TomlString $Version)),
        ('channel = "{0}"' -f (ConvertTo-TomlString $Channel)),
        ('target = "{0}"' -f (ConvertTo-TomlString $Target)),
        ('installed_at = "{0}"' -f (ConvertTo-TomlString $installedAt)),
        ('binary_dir = "{0}"' -f (ConvertTo-TomlString $InstallDir)),
        ('release_url = "{0}"' -f (ConvertTo-TomlString $ReleaseUrl)),
        ('artifact_sha256 = "{0}"' -f (ConvertTo-TomlString $ArtifactSha256)),
        ('archive = "{0}"' -f (ConvertTo-TomlString $ArchiveName)),
        ('repository = "{0}"' -f (ConvertTo-TomlString $Repo))
    ) -join [Environment]::NewLine

    $tmp = Join-Path $receiptDir ".install-receipt.toml.$PID"
    Set-Content -LiteralPath $tmp -Value ($content + [Environment]::NewLine) -NoNewline
    Move-Item -LiteralPath $tmp -Destination $receiptPath -Force
}

function Write-NextSteps {
    param(
        [string]$InstallDir,
        [string]$ExeExt
    )

    Write-Info ""
    Write-Info "WG installed:"
    Write-Info "  wg  $(Join-Path $InstallDir "wg$ExeExt")"
    Write-Info "  nex $(Join-Path $InstallDir "nex$ExeExt")"
    Write-Info ""

    $pathParts = ($env:PATH -split [IO.Path]::PathSeparator)
    if ($pathParts -notcontains $InstallDir) {
        Write-Warn "$InstallDir is not on PATH; add it before running wg"
    }

    Write-Info "Next:"
    Write-Info "  wg setup"
    Write-Info "  cd your-project"
    Write-Info "  wg init"
    Write-Info "  wg service start"
    Write-Info "  wg tui"
}

function Install-DevChannel {
    param(
        [string]$Target,
        [string]$ExeExt,
        [string]$InstallDir,
        [string]$DevDir,
        [bool]$IsDryRun
    )

    if ([string]::IsNullOrWhiteSpace($DevDir)) {
        $DevDir = (Get-Location).Path
    }
    $cargoToml = Join-Path $DevDir "Cargo.toml"
    if (-not (Test-Path -LiteralPath $cargoToml -PathType Leaf)) {
        Fail "-Channel dev requires a WG source checkout; pass -DevDir DIR"
    }

    $versionLine = Get-Content -LiteralPath $cargoToml | Where-Object { $_ -match '^version = "([^"]+)"' } | Select-Object -First 1
    $versionValue = "dev"
    if ($versionLine -match '^version = "([^"]+)"') {
        $versionValue = $Matches[1]
    }

    Write-Info "channel: dev"
    Write-Info "source: $DevDir"
    Write-Info "target: $Target"
    Write-Info "install dir: $InstallDir"
    Write-Info "checksum: skipped (dev channel builds local source)"
    Write-Info "attestation: skipped (dev channel builds local source)"

    if ($IsDryRun) {
        Write-Info "dry-run: would run cargo install --path $DevDir --locked --root <temp>"
        Write-NextSteps -InstallDir $InstallDir -ExeExt $ExeExt
        return
    }

    if (-not (Test-Command cargo)) {
        Fail "cargo is required for -Channel dev"
    }

    $workDir = Join-Path ([IO.Path]::GetTempPath()) ("wg-install-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $workDir | Out-Null
    try {
        $cargoRoot = Join-Path $workDir "cargo-root"
        & cargo install --path $DevDir --locked --root $cargoRoot --force
        if ($LASTEXITCODE -ne 0) {
            Fail "cargo install failed"
        }
        Install-Binary -Source (Join-Path $cargoRoot "bin/wg$ExeExt") -Destination (Join-Path $InstallDir "wg$ExeExt")
        Install-Binary -Source (Join-Path $cargoRoot "bin/nex$ExeExt") -Destination (Join-Path $InstallDir "nex$ExeExt")
        Write-InstallReceipt -Version $versionValue -Channel "dev" -Target $Target -InstallDir $InstallDir -ReleaseUrl "file://$DevDir" -ArtifactSha256 "dev" -ArchiveName "dev" -Repo $Repo -IsDryRun:$false
        Write-NextSteps -InstallDir $InstallDir -ExeExt $ExeExt
    } finally {
        Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Install-ReleaseChannel {
    param(
        [string]$Channel,
        [string]$Version,
        [string]$Target,
        [string]$ExeExt,
        [string]$ArchiveExt,
        [string]$InstallDir,
        [string]$Repo,
        [string]$BaseUrl,
        [bool]$IsDryRun
    )

    $workDir = Join-Path ([IO.Path]::GetTempPath()) ("wg-install-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $workDir | Out-Null

    try {
        $baseUrlTrimmed = Trim-TrailingSlash $BaseUrl
        if (-not [string]::IsNullOrWhiteSpace($baseUrlTrimmed)) {
            $manifestUrl = "$baseUrlTrimmed/release-manifest.json"
            $releaseBase = $baseUrlTrimmed
            $releaseUrl = $baseUrlTrimmed
        } elseif (-not [string]::IsNullOrWhiteSpace($Version)) {
            $tag = Convert-VersionToTag $Version
            $releaseBase = "https://github.com/$Repo/releases/download/$tag"
            $manifestUrl = "$releaseBase/release-manifest.json"
            $releaseUrl = "https://github.com/$Repo/releases/tag/$tag"
        } elseif ($Channel -eq "stable") {
            $manifestUrl = "https://github.com/$Repo/releases/latest/download/release-manifest.json"
            $releaseBase = "https://github.com/$Repo/releases/latest/download"
            $releaseUrl = "https://github.com/$Repo/releases/latest"
        } else {
            $tag = "nightly"
            $releaseBase = "https://github.com/$Repo/releases/download/$tag"
            $manifestUrl = "$releaseBase/release-manifest.json"
            $releaseUrl = "https://github.com/$Repo/releases/tag/$tag"
        }

        $manifestPath = Join-Path $workDir "release-manifest.json"
        Write-Info "manifest: $manifestUrl"
        Download-File -Url $manifestUrl -Destination $manifestPath
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json

        if (-not $manifest.version) {
            Fail "release-manifest.json is missing version"
        }

        if ($manifest.channel -and $manifest.channel -ne $Channel) {
            Write-Warn "requested channel $Channel but manifest reports $($manifest.channel)"
        }

        if ([string]::IsNullOrWhiteSpace($baseUrlTrimmed) -and [string]::IsNullOrWhiteSpace($Version) -and $Channel -eq "stable" -and $manifest.tag) {
            $releaseBase = "https://github.com/$Repo/releases/download/$($manifest.tag)"
            $releaseUrl = "https://github.com/$Repo/releases/tag/$($manifest.tag)"
        }

        $archiveName = "wg-v$($manifest.version)-$Target$ArchiveExt"
        $archiveUrl = "$releaseBase/$archiveName"
        $checksumsUrl = "$releaseBase/SHA256SUMS"
        $archivePath = Join-Path $workDir $archiveName
        $checksumsPath = Join-Path $workDir "SHA256SUMS"

        Write-Info "channel: $Channel"
        Write-Info "version: $($manifest.version)"
        Write-Info "target: $Target"
        Write-Info "archive: $archiveUrl"
        Write-Info "install dir: $InstallDir"

        if ($IsDryRun) {
            Write-Info "dry-run: would download $archiveUrl"
            Write-Info "dry-run: would verify SHA256 from $checksumsUrl"
            if ((Test-Command gh) -and [string]::IsNullOrWhiteSpace($baseUrlTrimmed)) {
                Write-Info "dry-run: would verify GitHub attestations for release-manifest.json, SHA256SUMS, and the archive with gh"
            } else {
                Write-Info "dry-run: attestation would be skipped unless gh and a GitHub release are available"
            }
            Write-Info "dry-run: would install wg$ExeExt and nex$ExeExt into $InstallDir"
            Write-InstallReceipt -Version $manifest.version -Channel $Channel -Target $Target -InstallDir $InstallDir -ReleaseUrl $releaseUrl -ArtifactSha256 "" -ArchiveName $archiveName -Repo $Repo -IsDryRun:$true
            Write-NextSteps -InstallDir $InstallDir -ExeExt $ExeExt
            return
        }

        Download-File -Url $archiveUrl -Destination $archivePath
        Download-File -Url $checksumsUrl -Destination $checksumsPath
        $artifactSha256 = Verify-Checksum -ArchiveName $archiveName -ArchivePath $archivePath -ChecksumsPath $checksumsPath
        Verify-Attestations -ArchiveName $archiveName -Repo $Repo -BaseUrl $baseUrlTrimmed -WorkDir $workDir

        $extractDir = Join-Path $workDir "extract"
        Expand-WgArchive -ArchivePath $archivePath -Destination $extractDir
        $payloadDir = Get-ChildItem -LiteralPath $extractDir -Directory | Select-Object -First 1
        if (-not $payloadDir) {
            Fail "archive did not contain a top-level directory"
        }

        $wgSource = Join-Path $payloadDir.FullName "wg$ExeExt"
        $nexSource = Join-Path $payloadDir.FullName "nex$ExeExt"
        if (-not (Test-Path -LiteralPath $wgSource -PathType Leaf)) {
            Fail "archive is missing wg$ExeExt"
        }
        if (-not (Test-Path -LiteralPath $nexSource -PathType Leaf)) {
            Fail "archive is missing nex$ExeExt"
        }

        Install-Binary -Source $wgSource -Destination (Join-Path $InstallDir "wg$ExeExt")
        Install-Binary -Source $nexSource -Destination (Join-Path $InstallDir "nex$ExeExt")
        Write-InstallReceipt -Version $manifest.version -Channel $Channel -Target $Target -InstallDir $InstallDir -ReleaseUrl $releaseUrl -ArtifactSha256 $artifactSha256 -ArchiveName $archiveName -Repo $Repo -IsDryRun:$false
        Write-NextSteps -InstallDir $InstallDir -ExeExt $ExeExt
    } finally {
        Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$Channel = Resolve-Option -Value $Channel -EnvName "WG_INSTALL_CHANNEL" -Default "stable"
$Version = Resolve-Option -Value $Version -EnvName "WG_INSTALL_VERSION" -Default ""
$InstallDir = Resolve-Option -Value $InstallDir -EnvName "WG_INSTALL_DIR" -Default ""
$Repo = Resolve-Option -Value $Repo -EnvName "WG_INSTALL_REPO" -Default "graphwork/wg"
$BaseUrl = Resolve-Option -Value $BaseUrl -EnvName "WG_INSTALL_BASE_URL" -Default ""
$Target = Resolve-Option -Value $Target -EnvName "WG_INSTALL_TARGET" -Default ""
$DevDir = Resolve-Option -Value $DevDir -EnvName "WG_INSTALL_DEV_DIR" -Default ""

$envDryRun = [Environment]::GetEnvironmentVariable("WG_INSTALL_DRY_RUN")
$isDryRun = $DryRun.IsPresent -or ($envDryRun -match '^(1|true|TRUE|yes|YES)$')

if ($Channel -notin @("stable", "nightly", "dev")) {
    Fail "-Channel must be stable, nightly, or dev"
}

$targetTriple = Get-InstallTarget -Override $Target
$archiveExt = Get-ArchiveExt -Target $targetTriple
$exeExt = Get-ExeExt -Target $targetTriple
$installPath = Get-DefaultInstallDir -Explicit $InstallDir
Ensure-InstallDir -Path $installPath -IsDryRun:$isDryRun

if ($Channel -eq "dev") {
    Install-DevChannel -Target $targetTriple -ExeExt $exeExt -InstallDir $installPath -DevDir $DevDir -IsDryRun:$isDryRun
} else {
    Install-ReleaseChannel -Channel $Channel -Version $Version -Target $targetTriple -ExeExt $exeExt -ArchiveExt $archiveExt -InstallDir $installPath -Repo $Repo -BaseUrl $BaseUrl -IsDryRun:$isDryRun
}
