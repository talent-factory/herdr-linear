# open-split-windows.ps1 -- Windows sibling of scripts/open-split.sh.
#
# Idempotent launcher for the Linear panel split pane. "Launch-or-focus, toggle on
# repeat", scoped to the current tab -- mirrors open-split.sh:
#   - no Linear pane in the current tab      -> open a split (focused)
#   - a Linear pane exists but isn't focused -> focus it
#   - the focused pane IS the Linear pane    -> close it (toggle off)
#
# WHY THIS DIVERGES FROM open-split.sh (mirrors herdr-file-viewer's verified-on-real-
# hardware Windows workaround): the unix launcher relies on herdr's manifest-based
# relative pane launching (`--entrypoint linear-panel`). That does NOT work on Windows:
# herdr passes the relative program name to CreateProcessW, which resolves it against
# herdr's OWN directory (not any cwd we pass), failing with ERROR_PATH_NOT_FOUND. herdr
# also stores the plugin root as a `\\?\` verbatim path. So on Windows we instead spawn
# the binary BY ABSOLUTE PATH: `pane split` an empty pane, `pane run` the absolute .exe
# into it, and `pane rename` it to "Linear" so the toggle (below) can find it again.
#
# The OPEN/FOCUS/CLOSE decision is computed in-process by the plugin binary itself
# (`herdr-linear.exe --launch-decision`, fed `pane list` JSON on stdin) -- unchanged from
# the unix launcher, so no Rust code was touched for this script.

$ErrorActionPreference = 'Continue'

# PowerShell 5.1 otherwise decodes herdr's UTF-8 JSON with the legacy console code page;
# non-ASCII pane titles or paths can corrupt the JSON and trigger the OPEN fallback.
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $Utf8NoBom
$OutputEncoding = $Utf8NoBom

$HerdrBin = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }

# Plugin root as a NORMAL absolute path (strip herdr's `\\?\` verbatim prefix).
# `$PSScriptRoot` is `<root>\scripts`, so the parent is the plugin root.
function Strip-Verbatim([string]$p) {
    if ($p -and $p.StartsWith('\\?\')) { return $p.Substring(4) }
    return $p
}
$PluginRoot = Strip-Verbatim (Split-Path -Parent $PSScriptRoot)
$LinearBin = Join-Path $PluginRoot 'target\release\herdr-linear.exe'

# The directory to root the panel at: the focused pane's cwd (the user's work pane) at
# invocation time. `pane list` prints JSON by default.
function Get-UserCwd {
    try {
        $focused = (& $HerdrBin pane list | ConvertFrom-Json).result.panes |
            Where-Object { $_.focused } | Select-Object -First 1
        if ($focused -and $focused.cwd) { return Strip-Verbatim $focused.cwd }
    } catch {}
    return $PluginRoot
}

# Extract the first `pane_id` from a herdr CLI JSON reply.
function Get-PaneId([string]$json) {
    return ([regex]'"pane_id":"([^"]+)"').Match($json).Groups[1].Value
}

# The plugin's config directory, to pass as HERDR_PLUGIN_CONFIG_DIR. herdr injects that
# variable only into a pane IT spawns from the manifest; this launcher spawns the binary
# itself (see the note at the top), so it must forward it explicitly, or herdr-linear.exe
# silently can't find its config.toml/API key on Windows. Empty on failure: the launch
# proceeds without it (falls back to LINEAR_API_KEY in the environment, if set).
function Get-ConfigDir {
    try {
        $d = (& $HerdrBin plugin config-dir herdr-linear | Out-String).Trim()
        if ($d) { return Strip-Verbatim $d }
    } catch {}
    return ''
}

function Open-Pane {
    $cwd = Get-UserCwd
    $splitArgs = @('pane', 'split', '--direction', 'right', '--cwd', $cwd, '--focus')
    $cfg = Get-ConfigDir
    if ($cfg) { $splitArgs += @('--env', "HERDR_PLUGIN_CONFIG_DIR=$cfg") }
    $out = (& $HerdrBin @splitArgs | Out-String)
    $np = Get-PaneId $out
    if ($np) {
        # Run the binary by ABSOLUTE path via the PowerShell CALL OPERATOR. herdr types
        # <command> into the pane's shell (PowerShell on Windows); a bare or plain-quoted
        # path splits on a space in the install path (e.g. C:\Users\First Last\...) and
        # the binary never starts. `& "<path>"` executes it; the `\"` escaping survives
        # Windows PowerShell 5.1's native-arg quote-stripping so herdr receives the
        # quotes intact.
        & $HerdrBin pane run $np "& `"$LinearBin`""
        # Label it so a later invocation's launch-decision recognises it (best-effort).
        & $HerdrBin pane rename $np Linear *> $null
    }
    exit 0
}

$Decision = 'OPEN'
if (Test-Path $LinearBin) {
    $panes = & $HerdrBin pane list 2>$null
    if ($LASTEXITCODE -ne 0) { $panes = $null }
    if ($panes) {
        $panesText = ($panes -join "`n")
        $Decision = ($panesText | & $LinearBin --launch-decision 2>$null)
        if ($LASTEXITCODE -ne 0 -or -not $Decision) { $Decision = 'OPEN' }
    }
}

if ($Decision -like 'FOCUS *') {
    $PaneId = $Decision.Substring(6)
    & $HerdrBin pane zoom $PaneId --on *> $null
    & $HerdrBin pane zoom $PaneId --off
    exit $LASTEXITCODE
} elseif ($Decision -like 'CLOSE *') {
    $PaneId = $Decision.Substring(6)
    & $HerdrBin pane close $PaneId
    exit $LASTEXITCODE
} else {
    Open-Pane
}
