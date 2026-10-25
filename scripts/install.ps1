# Installs the traceqa binary from a GitHub release, on Windows (PowerShell 5.1 or 7).
#
#   irm https://raw.githubusercontent.com/thearpankumar/TraceQA/master/scripts/install.ps1 | iex
#
# With options:
#
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/thearpankumar/TraceQA/master/scripts/install.ps1))) -Version v0.2.0
#
#   -Version TAG       $env:TRACEQA_VERSION          A release tag such as v0.2.0. Default: the newest release.
#   -InstallDir DIR    $env:TRACEQA_INSTALL_DIR      Where traceqa.exe goes. Default: ~\.local\bin, as uv's installer.
#   -NoModifyPath      $env:TRACEQA_NO_MODIFY_PATH=1 Leave your user PATH alone; only say what to add.
#
# TRACEQA_DOWNLOAD_BASE and TRACEQA_GITHUB_API point it at a mirror (and at a local server in the tests).
#
# The archive is checked against the release's SHA256SUMS.txt before anything is installed. Everything runs
# inside functions called on the last lines, so a download cut off halfway runs nothing, and the script never
# calls exit when it is run with iex, which would close your PowerShell window. On Linux and macOS, use
# scripts/install.sh.

param(
    [string]$Version = $env:TRACEQA_VERSION,
    [string]$InstallDir = $env:TRACEQA_INSTALL_DIR,
    [switch]$NoModifyPath
)

function Write-TraceQAStep([string]$Message) {
    Write-Host "traceqa-installer: $Message"
}

# HTTPS only. Plain http is allowed for this machine alone, which is what the tests serve from.
function Assert-TraceQAUrl([string]$Url) {
    if ($Url -match '^https://' -or $Url -match '^http://(127\.0\.0\.1|localhost)[:/]') {
        return
    }
    throw "refusing to download from '$Url': only https addresses are allowed"
}

function Get-TraceQAFile([string]$Url, [string]$Path) {
    Assert-TraceQAUrl $Url
    # The progress bar makes Invoke-WebRequest many times slower on PowerShell 5.1.
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing
}

function Get-TraceQAPlatform {
    if ($PSVersionTable.PSVersion.Major -ge 6 -and -not $IsWindows) {
        throw 'this installer is for Windows; on Linux and macOS use scripts/install.sh'
    }
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw 'there is no 32-bit build of traceqa'
    }
    $arch = $null
    try {
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    } catch {
        # Older .NET Framework builds have no RuntimeInformation.
        $arch = $env:PROCESSOR_ARCHITEW6432
        if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    }
    switch -Regex ($arch) {
        '^(X64|AMD64)$' { return 'windows-x86_64' }
        '^(Arm64|ARM64)$' {
            Write-Warning 'There is no ARM64 build yet: installing the x86-64 one, which Windows 11 runs under emulation.'
            return 'windows-x86_64'
        }
        default { throw "there is no prebuilt traceqa for Windows on $arch; build it from source: https://github.com/thearpankumar/TraceQA#installation" }
    }
}

# Until 1.0 every release is a pre-release, which GitHub's /releases/latest leaves out: ask for the newest
# release of any kind instead.
function Resolve-TraceQAVersion([string]$Repo, [string]$Wanted) {
    if (-not $Wanted) {
        $apiBase = if ($env:TRACEQA_GITHUB_API) { $env:TRACEQA_GITHUB_API } else { 'https://api.github.com' }
        $api = "$apiBase/repos/$Repo/releases?per_page=1"
        Assert-TraceQAUrl $api
        try {
            $releases = Invoke-RestMethod -Uri $api -UseBasicParsing -Headers @{ Accept = 'application/vnd.github+json' }
        } catch {
            throw "could not ask GitHub for the newest release (offline, or rate limited?); name one with -Version v0.2.0. $($_.Exception.Message)"
        }
        $list = @($releases)
        if ($list.Count -eq 0) {
            throw 'TraceQA has no release yet; build it from source: https://github.com/thearpankumar/TraceQA#installation'
        }
        $Wanted = [string]$list[0].tag_name
    }
    if ($Wanted -notmatch '^v') { $Wanted = "v$Wanted" }
    if ($Wanted -notmatch '^[A-Za-z0-9._-]+$') { throw "'$Wanted' is not a release tag" }
    return $Wanted
}

function Get-TraceQAExpectedHash([string]$SumsFile, [string]$Name) {
    foreach ($line in Get-Content -LiteralPath $SumsFile) {
        $parts = $line.Trim() -split '\s+', 2
        if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $Name) {
            return $parts[0]
        }
    }
    throw "SHA256SUMS.txt has no line for $Name"
}

# Adds Dir to the user PATH in the registry, keeping entries such as %USERPROFILE%\bin unexpanded
# (setx and [Environment]::SetEnvironmentVariable would expand them, and setx cuts PATH at 1024 characters).
function Add-TraceQAToUserPath([string]$Dir) {
    $key = Get-Item -LiteralPath 'HKCU:\Environment'
    $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $wanted = $Dir.TrimEnd('\')
    foreach ($entry in ($raw -split ';')) {
        if ([Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\') -ieq $wanted) {
            return $false
        }
    }
    $new = if ($raw) { "$Dir;$raw" } else { $Dir }
    Set-ItemProperty -LiteralPath 'HKCU:\Environment' -Name 'Path' -Value $new -Type ExpandString
    # Setting and clearing a user variable through .NET tells open programs that the environment changed,
    # so new terminals see the new PATH without signing out.
    [Environment]::SetEnvironmentVariable('TRACEQA_INSTALLER_REFRESH', '1', 'User')
    [Environment]::SetEnvironmentVariable('TRACEQA_INSTALLER_REFRESH', $null, 'User')
    return $true
}

function Install-TraceQA([string]$Version, [string]$InstallDir, [bool]$NoModifyPath) {
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version 3
    $repo = 'thearpankumar/TraceQA'
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        # Windows PowerShell 5.1 may not offer TLS 1.2, which GitHub requires, by default.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
    if (-not $InstallDir) { $InstallDir = Join-Path $HOME '.local\bin' }

    $platform = Get-TraceQAPlatform
    $Version = Resolve-TraceQAVersion $repo $Version
    $archive = "traceqa-$Version-$platform.zip"
    $downloadBase = if ($env:TRACEQA_DOWNLOAD_BASE) { $env:TRACEQA_DOWNLOAD_BASE } else { "https://github.com/$repo/releases/download" }
    $base = "$downloadBase/$Version"

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("traceqa-installer-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp $archive
        $sums = Join-Path $tmp 'SHA256SUMS.txt'
        Write-TraceQAStep "downloading $archive"
        Get-TraceQAFile "$base/$archive" $zip
        Get-TraceQAFile "$base/SHA256SUMS.txt" $sums
        $expected = Get-TraceQAExpectedHash $sums $archive
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash
        if ($actual -ne $expected) {
            throw "checksum mismatch for $archive (expected $expected, got $actual); nothing was installed"
        }
        Write-TraceQAStep 'checksum verified'

        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -LiteralPath $zip -DestinationPath $unpacked -Force
        $exe = Join-Path $unpacked 'traceqa.exe'
        if (-not (Test-Path -LiteralPath $exe)) { throw "$archive holds no traceqa.exe" }

        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        $target = Join-Path $InstallDir 'traceqa.exe'
        try {
            Copy-Item -LiteralPath $exe -Destination $target -Force
        } catch {
            throw "could not write $target (is traceqa running?): $($_.Exception.Message)"
        }
        $installed = & $target --version
        if ($LASTEXITCODE -ne 0) { throw "the downloaded traceqa.exe does not run (exit code $LASTEXITCODE)" }
        Write-TraceQAStep "installed $installed to $target"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    $onPath = @($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') }).Count -gt 0
    if (-not $onPath) {
        if ($NoModifyPath) {
            Write-TraceQAStep "$InstallDir is not on your PATH; add it in Settings, Environment Variables, or run: `$env:Path = `"$InstallDir;`$env:Path`""
        } else {
            if (Add-TraceQAToUserPath $InstallDir) {
                Write-TraceQAStep "added $InstallDir to your user PATH; new terminals will find traceqa"
            }
            $env:Path = "$InstallDir;$env:Path"
        }
    }
    if (Get-Command uv -ErrorAction SilentlyContinue) {
        Write-TraceQAStep 'next, install a browser: traceqa install chromium   (then check with: traceqa doctor)'
    } else {
        Write-TraceQAStep 'next, install uv (https://docs.astral.sh/uv/getting-started/installation/), then: traceqa install chromium'
    }
}

try {
    $traceqaNoModifyPath = $NoModifyPath.IsPresent -or $env:TRACEQA_NO_MODIFY_PATH -eq '1'
    Install-TraceQA -Version $Version -InstallDir $InstallDir -NoModifyPath $traceqaNoModifyPath
} catch {
    Write-Host "traceqa-installer: error: $($_.Exception.Message)" -ForegroundColor Red
    # Run as this file (powershell -File install.ps1), a failure sets the exit code. Run with iex or as a
    # script block, exit would close the window or the caller's script, so the message is all.
    if ($MyInvocation.MyCommand.CommandType -eq 'ExternalScript' -and $MyInvocation.MyCommand.Name -eq 'install.ps1') {
        exit 1
    }
}
