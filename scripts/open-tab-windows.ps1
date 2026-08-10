# open-tab-windows.ps1 -- Windows sibling of scripts/open-tab.sh.
#
# Idempotent launcher for the Linear panel in its own TAB. "Open-or-switch, toggle on
# repeat", scoped across the tabs of the CURRENT WORKSPACE -- mirrors open-tab.sh. A
# panel open in a different workspace is left alone; a fresh one opens here.
#
# Sibling of open-split-windows.ps1 -- see its header for WHY the Windows launchers spawn
# the binary by ABSOLUTE path (`tab create` + `pane run`) instead of the manifest's
# relative pane launching: herdr can't spawn relative program names on Windows and stores
# the plugin root as a `\\?\` verbatim path. The
# OPEN/SWITCHTAB/FOCUS/CLOSE decision is computed in-process by herdr-linear.exe itself
# (`--launch-decision-tab`, fed `pane list` JSON on stdin) -- unchanged from the unix
# launcher.

$ErrorActionPreference = 'Continue'

try {
    $Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [Console]::OutputEncoding = $Utf8NoBom
    $OutputEncoding = $Utf8NoBom
} catch {
    # Best-effort only -- never let a redirected/piped stdout abort the launcher.
}

$HerdrBin = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }

function Strip-Verbatim([string]$p) {
    if ($p -and $p.StartsWith('\\?\')) { return $p.Substring(4) }
    return $p
}
$PluginRoot = Strip-Verbatim (Split-Path -Parent $PSScriptRoot)
$LinearBin = Join-Path $PluginRoot 'target\release\herdr-linear.exe'

# Root the panel at the focused pane's cwd (the user's work pane). `pane list` prints
# JSON by default.
function Get-UserCwd {
    try {
        $focused = (& $HerdrBin pane list | ConvertFrom-Json).result.panes |
            Where-Object { $_.focused } | Select-Object -First 1
        if ($focused -and $focused.cwd) { return Strip-Verbatim $focused.cwd }
    } catch {}
    return $PluginRoot
}

function Get-PaneId([string]$json) {
    return ([regex]'"pane_id":"([^"]+)"').Match($json).Groups[1].Value
}

# See open-split-windows.ps1's Get-ConfigDir for why this is needed on Windows.
function Get-ConfigDir {
    try {
        $d = (& $HerdrBin plugin config-dir herdr-linear | Out-String).Trim()
        if ($d) { return Strip-Verbatim $d }
    } catch {}
    return ''
}

function Open-Tab {
    $cwd = Get-UserCwd
    # `tab create` makes a new tab with a shell pane (its root_pane); run the binary
    # into it by absolute path and label the pane "Linear" so a later launch-decision
    # recognises it.
    $createArgs = @('tab', 'create', '--cwd', $cwd, '--label', 'Linear', '--focus')
    $cfg = Get-ConfigDir
    if ($cfg) { $createArgs += @('--env', "HERDR_PLUGIN_CONFIG_DIR=$cfg") }
    $out = (& $HerdrBin @createArgs | Out-String)
    $np = Get-PaneId $out
    if ($np) {
        # Call operator + quoted absolute path so a spaced install path still launches --
        # see open-split-windows.ps1's Open-Pane for the full why.
        & $HerdrBin pane run $np "& \`"$LinearBin\`""
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
        $Decision = ($panesText | & $LinearBin --launch-decision-tab 2>$null)
        if ($LASTEXITCODE -ne 0 -or -not $Decision) { $Decision = 'OPEN' }
    }
}

if ($Decision -like 'SWITCHTAB *') {
    $TabId = $Decision.Substring(10)
    # If the target tab vanished between the pane-list snapshot and now (a race -- the
    # Linear tab was closed in between), fall back to opening a fresh tab rather than
    # leaving the keypress a silent no-op.
    & $HerdrBin tab focus $TabId
    if ($LASTEXITCODE -ne 0) { Open-Tab } else { exit 0 }
} elseif ($Decision -like 'FOCUS *') {
    $PaneId = $Decision.Substring(6)
    & $HerdrBin pane zoom $PaneId --on *> $null
    & $HerdrBin pane zoom $PaneId --off
    exit $LASTEXITCODE
} elseif ($Decision -like 'CLOSE *') {
    $PaneId = $Decision.Substring(6)
    & $HerdrBin pane close $PaneId
    exit $LASTEXITCODE
} else {
    Open-Tab
}
