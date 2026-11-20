param(
    [Parameter(Mandatory)][string]$StateDir,
    [Parameter(Mandatory)][string]$Library,
    [string]$ExpectedSha256
)
$ErrorActionPreference = 'Stop'
$selected = (Resolve-Path -LiteralPath $Library).Path
$extension = if ($IsWindows) { '.dll' } elseif ($IsLinux) { '.so' } else { throw 'Unsupported platform' }
if ([IO.Path]::GetExtension($selected) -ne $extension) { throw "Expected a $extension library" }
$sha256 = (Get-FileHash -LiteralPath $selected -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ExpectedSha256 -and $sha256 -ne $ExpectedSha256.ToLowerInvariant()) { throw "SHA-256 verification failed" }
$root = Join-Path $StateDir 'plugin-native/paperconnect'
$approval = Join-Path $root 'approved'
$version = Join-Path $root $sha256
[void][IO.Directory]::CreateDirectory($approval)
[void][IO.Directory]::CreateDirectory($version)
$name = if ($IsWindows) { 'module.dll' } else { 'libmodule.so' }
$destination = Join-Path $version $name
if (!(Test-Path -LiteralPath $destination)) {
    $temporary = Join-Path $version ([Guid]::NewGuid().ToString('N') + '.tmp')
    try {
        Copy-Item -LiteralPath $selected -Destination $temporary
        if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash -ne $sha256) { throw 'SHA-256 changed while copying' }
        [IO.File]::Move($temporary, $destination)
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    }
}
if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne $sha256) { throw 'Installed file does not match digest' }
[IO.File]::WriteAllText((Join-Path $approval $sha256), $sha256)
$marker = Join-Path $StateDir 'plugin-native/paperconnect.removed'
if (Test-Path -LiteralPath $marker) { Remove-Item -LiteralPath $marker }
Write-Output $sha256
