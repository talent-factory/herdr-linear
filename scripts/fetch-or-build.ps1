# fetch-or-build.ps1 -- herdr [[build]] step for herdr-linear (Windows).
#
# Windows PowerShell 5.1 (Desktop) compatible -- the in-box shell on Windows 10/11/Server,
# so installing needs no extra tooling. The Windows sibling of fetch-or-build.sh: same
# behaviour, different shell.
#
# Fast path: download the prebuilt binary that matches THIS source's declared version +
# platform from the GitHub release, verify its SHA-256, and install it at
# target\release\herdr-linear.exe. The match is by VERSION, not by exact commit -- see
# fetch-or-build.sh's header for the full rationale (landing a PR shouldn't force new
# installs to compile while a release is pending).
#
# Fallback: on ANY miss (no asset for this version, network/download error, checksum
# mismatch, unmapped platform, no cargo) print a clear notice and build from source with
# cargo -- identical to the pre-prebuilt behavior, so installing never gets harder than
# before.
#
# Paths and the release base URL are overridable via env (HL_REPO_ROOT / HL_CARGO_TOML /
# HL_OUT / HL_BASE_URL).

$ErrorActionPreference = 'Stop'

$Repo = 'talent-factory/herdr-linear'

$RepoRoot = if ($env:HL_REPO_ROOT) { $env:HL_REPO_ROOT } else { Join-Path $PSScriptRoot '..' }
$CargoToml = if ($env:HL_CARGO_TOML) { $env:HL_CARGO_TOML } else { Join-Path $RepoRoot 'Cargo.toml' }
$Out = if ($env:HL_OUT) { $env:HL_OUT } else { Join-Path $RepoRoot 'target\release\herdr-linear.exe' }
$BaseUrl = if ($env:HL_BASE_URL) { $env:HL_BASE_URL } else { "https://github.com/$Repo/releases/download" }

# Build from source -- the original, unconditional behavior. A missing cargo gets a clear
# message pointing at rustup, exactly like the sh script's fallback. --features plugin is
# required: the herdr-linear binary is behind that Cargo feature.
function Build-FromSource {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        [Console]::Error.WriteLine("herdr-linear needs a Rust toolchain to build, but cargo was not found. Install Rust from https://rustup.rs then re-run: herdr plugin install $Repo")
        exit 1
    }
    & cargo build --release --features plugin
    exit $LASTEXITCODE
}

function Invoke-HlFallback {
    param([string]$Reason)
    [Console]::Error.WriteLine("herdr-linear: $Reason - building from source instead.")
    if ($script:TmpDir -and (Test-Path $script:TmpDir)) {
        Remove-Item -Recurse -Force $script:TmpDir -ErrorAction SilentlyContinue
    }
    Build-FromSource
}

# Hex SHA-256 of a file. Get-FileHash is primary; certutil is the fallback, mirroring the
# sh script's sha256sum/shasum preference order. $null on total failure.
function Get-HlSha256Hex {
    param([string]$Path)
    try {
        return (Get-FileHash -Algorithm SHA256 -Path $Path -ErrorAction Stop).Hash.ToLowerInvariant()
    } catch {
        try {
            $out = & certutil -hashfile $Path SHA256 2>$null
        } catch {
            return $null
        }
        if ($LASTEXITCODE -ne 0 -or -not $out) { return $null }
        $hashLine = $out | Where-Object { $_ -match '^[0-9a-fA-F]{64}$' } | Select-Object -First 1
        if ($hashLine) { return $hashLine.Trim().ToLowerInvariant() }
        return $null
    }
}

# A thin Invoke-WebRequest wrapper.
function Invoke-HlDownload {
    param([string]$Url, [string]$Dest)
    try {
        # Force TLS 1.2 -- Windows PowerShell 5.1 inherits the .NET Framework default, which
        # on older / policy-locked hosts can negotiate TLS 1.0 and be rejected by GitHub's
        # release CDN, needlessly dropping the prebuilt fast path to a source build.
        try {
            [Net.ServicePointManager]::SecurityProtocol =
                [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        } catch {}
        Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing -ErrorAction Stop
        return $true
    } catch {
        return $false
    }
}

# --- resolve the target triple from the platform ------------------------------------------
$Arch = $env:PROCESSOR_ARCHITECTURE
$Triple = $null
if ($Arch -eq 'AMD64') { $Triple = 'x86_64-pc-windows-msvc' }
if (-not $Triple) { Invoke-HlFallback "no prebuilt binary for Windows/$Arch" }

# --- read the version this source declares --------------------------------------------------
$Version = $null
if (Test-Path $CargoToml) {
    $match = Select-String -Path $CargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($match) { $Version = $match.Matches[0].Groups[1].Value }
}
if (-not $Version) { Invoke-HlFallback "could not read version from $CargoToml" }

$Asset = "herdr-linear-$Triple.exe"

$script:TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("hl-fob-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $script:TmpDir -Force | Out-Null

# --- version-only match + transparency "ahead" note (best-effort, never a failure) ---------
$AheadNote = ''
try {
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($git) {
        & git -C $RepoRoot rev-parse --is-inside-work-tree *> $null
        if ($LASTEXITCODE -eq 0) {
            $headRev = (& git -C $RepoRoot rev-parse HEAD 2>$null)
            if (-not $headRev) { $headRev = 'nohead' }
            $commitFile = Join-Path $script:TmpDir 'COMMIT'
            if (Invoke-HlDownload "$BaseUrl/v$Version/COMMIT" $commitFile) {
                $releaseCommit = (Get-Content $commitFile -Raw -ErrorAction SilentlyContinue)
                if ($releaseCommit) { $releaseCommit = $releaseCommit.Trim() }
                if ($releaseCommit -and ($headRev -ne $releaseCommit)) {
                    $AheadNote = " Note: this checkout ($headRev) is ahead of the v$Version release commit ($releaseCommit), so newer unreleased source is not in this binary."
                }
            }
        }
    }
} catch {
    # Best-effort only -- never block the install on a transparency note.
}

$BinUrl = "$BaseUrl/v$Version/$Asset"
$SumsUrl = "$BaseUrl/v$Version/SHA256SUMS"
$TmpBin = Join-Path $script:TmpDir $Asset
$TmpSums = Join-Path $script:TmpDir 'SHA256SUMS'

if (-not (Invoke-HlDownload $BinUrl $TmpBin)) { Invoke-HlFallback "prebuilt binary not available for v$Version ($Asset)" }
if (-not (Invoke-HlDownload $SumsUrl $TmpSums)) { Invoke-HlFallback "checksums not available for v$Version" }

# Expected hash = the SHA256SUMS line for our asset filename. Accept either the coreutils
# text-mode separator (two spaces) or binary-mode (` *`) so a line produced by
# Git-for-Windows sha256sum on the release runner still verifies.
$Expected = $null
$assetPattern = [regex]::Escape($Asset)
Get-Content $TmpSums | ForEach-Object {
    if (-not $Expected -and $_ -match "^([0-9a-fA-F]{64}) [ *]${assetPattern}`$") {
        $Expected = $Matches[1].ToLowerInvariant()
    }
}
if (-not $Expected) { Invoke-HlFallback "no checksum listed for $Asset" }

$Actual = Get-HlSha256Hex $TmpBin
if (-not $Actual) { Invoke-HlFallback 'no SHA-256 tool (Get-FileHash/certutil) available' }
if ($Actual -ne $Expected) { Invoke-HlFallback "checksum mismatch for $Asset (expected $Expected, got $Actual)" }

# Verified -- move it into place.
$OutDir = Split-Path -Parent $Out
if ($OutDir -and -not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
}
Move-Item -Force $TmpBin $Out
[Console]::Out.WriteLine("herdr-linear: installed prebuilt v$Version ($Triple), verified SHA-256.$AheadNote")
Remove-Item -Recurse -Force $script:TmpDir -ErrorAction SilentlyContinue
exit 0
