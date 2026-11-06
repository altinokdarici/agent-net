# Installs agent-net on Windows: irm https://raw.githubusercontent.com/altinokdarici/agent-net/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$bin = Join-Path $HOME '.agent-net\bin'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $bin) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$bin", 'User')
}
if (($env:Path -split ';') -notcontains $bin) { $env:Path = "$env:Path;$bin" }
$exe = Join-Path $env:TEMP 'agent-net-setup.exe'
Invoke-WebRequest 'https://github.com/altinokdarici/agent-net/releases/latest/download/agent-net-x86_64-pc-windows-msvc.exe' -OutFile $exe
& $exe setup
if ($LASTEXITCODE -ne 0) { throw "agent-net setup failed with exit code $LASTEXITCODE." }
