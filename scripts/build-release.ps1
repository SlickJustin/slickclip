[CmdletBinding()]
param(
  [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-RequiredEnvironmentValue([string]$Name) {
  $value = [Environment]::GetEnvironmentVariable($Name, "Process")
  if ([String]::IsNullOrWhiteSpace($value)) {
    throw "Release input $Name is required."
  }
  return $value
}

function Assert-HttpsUrl([string]$Name, [string]$Value) {
  $parsed = $null
  if (-not [Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$parsed) -or
      $parsed.Scheme -ne "https" -or
      [String]::IsNullOrWhiteSpace($parsed.Host) -or
      -not [String]::IsNullOrEmpty($parsed.UserInfo) -or
      -not [String]::IsNullOrEmpty($parsed.Fragment)) {
    throw "$Name must be an absolute HTTPS URL without credentials or a fragment."
  }
}

$updaterEndpoint = Get-RequiredEnvironmentValue "SLICKCLIP_UPDATER_ENDPOINT"
$updaterPublicKey = Get-RequiredEnvironmentValue "SLICKCLIP_UPDATER_PUBLIC_KEY"
$artifactUrl = Get-RequiredEnvironmentValue "SLICKCLIP_UPDATER_ARTIFACT_URL"
$windowsSignCommand = Get-RequiredEnvironmentValue "SLICKCLIP_WINDOWS_SIGN_COMMAND"
$null = Get-RequiredEnvironmentValue "TAURI_SIGNING_PRIVATE_KEY"

Assert-HttpsUrl "SLICKCLIP_UPDATER_ENDPOINT" $updaterEndpoint
Assert-HttpsUrl "SLICKCLIP_UPDATER_ARTIFACT_URL" $artifactUrl
if ($updaterPublicKey.Trim().Length -lt 32) {
  throw "SLICKCLIP_UPDATER_PUBLIC_KEY does not contain a plausible updater public key."
}
if (-not $windowsSignCommand.Contains("%1")) {
  throw "SLICKCLIP_WINDOWS_SIGN_COMMAND must contain Tauri's %1 file placeholder."
}

if ($ValidateOnly) {
  Write-Host "SlickClip release inputs are present and structurally valid."
  exit 0
}

$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$tauriCommand = Join-Path $projectRoot "node_modules\.bin\tauri.cmd"
if (-not (Test-Path -LiteralPath $tauriCommand -PathType Leaf)) {
  throw "The local Tauri CLI is missing. Run npm install before building a release."
}

$package = Get-Content -Raw -LiteralPath (Join-Path $projectRoot "package.json") | ConvertFrom-Json
$version = [string]$package.version
if ($version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
  throw "package.json contains an invalid release version."
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("slickclip-release-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  $releaseConfigPath = Join-Path $temporaryRoot "tauri.release.json"
  $releaseConfig = [ordered]@{
    bundle = [ordered]@{
      createUpdaterArtifacts = $true
      windows = [ordered]@{
        signCommand = $windowsSignCommand
      }
    }
    plugins = [ordered]@{
      updater = [ordered]@{
        pubkey = $updaterPublicKey
        endpoints = @($updaterEndpoint)
        windows = [ordered]@{
          installMode = "passive"
        }
      }
    }
  }
  [IO.File]::WriteAllText(
    $releaseConfigPath,
    ($releaseConfig | ConvertTo-Json -Depth 8),
    [Text.UTF8Encoding]::new($false)
  )

  & npm.cmd run prepare:ffmpeg
  if ($LASTEXITCODE -ne 0) { throw "Pinned FFmpeg preparation failed." }
  & $tauriCommand build --bundles nsis --config $releaseConfigPath
  if ($LASTEXITCODE -ne 0) { throw "The signed SlickClip release build failed." }

  $releaseRoot = Join-Path $projectRoot "src-tauri\target\release"
  $application = Get-Item -LiteralPath (Join-Path $releaseRoot "SlickClip.exe")
  $installer = Get-Item -LiteralPath (Join-Path $releaseRoot "bundle\nsis\SlickClip_${version}_x64-setup.exe")
  $updaterSignature = Get-Item -LiteralPath ($installer.FullName + ".sig")
  foreach ($artifact in @($application, $installer)) {
    $signature = Get-AuthenticodeSignature -LiteralPath $artifact.FullName
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
      throw "Authenticode verification failed for '$($artifact.FullName)': $($signature.Status)."
    }
  }
  $signatureText = (Get-Content -Raw -LiteralPath $updaterSignature.FullName).Trim()
  if ([String]::IsNullOrWhiteSpace($signatureText)) {
    throw "The updater signature is empty."
  }

  $releaseNotes = [Environment]::GetEnvironmentVariable("SLICKCLIP_RELEASE_NOTES", "Process")
  if ([String]::IsNullOrWhiteSpace($releaseNotes)) {
    $releaseNotes = "SlickClip $version"
  }
  $manifest = [ordered]@{
    version = $version
    notes = $releaseNotes
    pub_date = [DateTimeOffset]::UtcNow.ToString("o")
    url = $artifactUrl
    signature = $signatureText
  }
  $manifestPath = Join-Path $installer.DirectoryName "latest.json"
  [IO.File]::WriteAllText(
    $manifestPath,
    ($manifest | ConvertTo-Json -Depth 4),
    [Text.UTF8Encoding]::new($false)
  )

  $applicationHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $application.FullName).Hash
  $installerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer.FullName).Hash
  Write-Host "Signed SlickClip release artifacts verified."
  Write-Host "Application: $($application.FullName) SHA-256 $applicationHash"
  Write-Host "Installer: $($installer.FullName) SHA-256 $installerHash"
  Write-Host "Updater signature: $($updaterSignature.FullName)"
  Write-Host "Release feed manifest: $manifestPath"
}
finally {
  $resolvedTemporary = [IO.Path]::GetFullPath($temporaryRoot)
  $resolvedTempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedTemporary.StartsWith($resolvedTempBase, [StringComparison]::OrdinalIgnoreCase) -and
      ([IO.Path]::GetFileName($resolvedTemporary)).StartsWith("slickclip-release-")) {
    Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force -ErrorAction SilentlyContinue
  }
}
