$ErrorActionPreference = 'Stop'
$repo = 'erenmentes/zetsu'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\zetsu'
$exe = Join-Path $dir 'zetsu.exe'

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
New-Item -ItemType Directory -Force -Path $dir | Out-Null

Get-Process zetsu -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Stop-Process -Force

Write-Host 'Downloading zetsu...'
$url = "https://github.com/$repo/releases/latest/download/zetsu-windows-x86_64.exe"
Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
if (-not (($userPath -split ';') -contains $dir)) {
    [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $dir).TrimStart(';'), 'User')
}

$shell = New-Object -ComObject WScript.Shell
$wt = Get-Command wt.exe -ErrorAction SilentlyContinue
$places = @(
    [Environment]::GetFolderPath('Desktop'),
    [Environment]::GetFolderPath('Programs')
)
foreach ($p in $places) {
    $lnk = $shell.CreateShortcut((Join-Path $p 'Zetsu IDE.lnk'))
    if ($wt) {
        $lnk.TargetPath = $wt.Source
        $lnk.Arguments = "--title `"Zetsu IDE`" `"$exe`""
    } else {
        $lnk.TargetPath = $exe
    }
    $lnk.IconLocation = "$exe,0"
    $lnk.WorkingDirectory = $env:USERPROFILE
    $lnk.Description = 'Zetsu IDE'
    $lnk.Save()
}

Write-Host ''
Write-Host 'zetsu installed.'
Write-Host '  Desktop shortcut: Zetsu IDE'
Write-Host '  Terminal command: zetsu  (open a new terminal first)'
