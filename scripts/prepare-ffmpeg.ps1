[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$archiveName = "ffmpeg-N-125875-g5d4d3bdc61-win64-gpl.zip"
$releaseTag = "autobuild-2026-07-31-14-10"
$archiveSha256 = "68A5E966533002785C3E4B9A98327E21D5277802668BF889D94086CB6426CBB4"
$ffmpegSha256 = "DCEC5129F94A0E7338303A9BDB6548889D28238F57E1A2315884946C47FA1C40"
$ffprobeSha256 = "AD3DD773BD94CD86906EB451D83112181BC884DF7A5D57D15797BFAD1F093DA8"
$licenseSha256 = "8CEB4B9EE5ADEDDE47B31E975C1D90C73AD27B6B165A1DCD80C7C545EB65B903"
$downloadUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$releaseTag/$archiveName"

$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$binaryRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot "src-tauri\binaries"))
if (-not $binaryRoot.StartsWith($projectRoot, [StringComparison]::OrdinalIgnoreCase)) {
  throw "Refused to stage FFmpeg outside the SlickClip project."
}

$ffmpegTarget = Join-Path $binaryRoot "ffmpeg-x86_64-pc-windows-msvc.exe"
$ffprobeTarget = Join-Path $binaryRoot "ffprobe-x86_64-pc-windows-msvc.exe"
$licenseTarget = Join-Path $binaryRoot "FFmpeg-LICENSE.txt"
$sourceTarget = Join-Path $binaryRoot "FFmpeg-SOURCE.txt"

function Test-ExpectedHash([string]$Path, [string]$Expected) {
  return (Test-Path -LiteralPath $Path -PathType Leaf) -and
    ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -eq $Expected)
}

if ((Test-ExpectedHash $ffmpegTarget $ffmpegSha256) -and
    (Test-ExpectedHash $ffprobeTarget $ffprobeSha256) -and
    (Test-ExpectedHash $licenseTarget $licenseSha256) -and
    (Test-Path -LiteralPath $sourceTarget -PathType Leaf)) {
  Write-Host "Pinned SlickClip FFmpeg sidecars are ready."
  exit 0
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("slickclip-ffmpeg-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  $archivePath = Join-Path $temporaryRoot $archiveName
  Invoke-WebRequest -UseBasicParsing -Uri $downloadUrl -OutFile $archivePath
  if (-not (Test-ExpectedHash $archivePath $archiveSha256)) {
    throw "The downloaded FFmpeg archive did not match SlickClip's pinned SHA-256."
  }

  $extractRoot = Join-Path $temporaryRoot "extract"
  Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot
  $ffmpegSource = @(Get-ChildItem -LiteralPath $extractRoot -Recurse -File -Filter "ffmpeg.exe")
  $ffprobeSource = @(Get-ChildItem -LiteralPath $extractRoot -Recurse -File -Filter "ffprobe.exe")
  $licenseSource = @(Get-ChildItem -LiteralPath $extractRoot -Recurse -File -Filter "LICENSE.txt")
  if ($ffmpegSource.Count -ne 1 -or $ffprobeSource.Count -ne 1 -or $licenseSource.Count -ne 1) {
    throw "The pinned FFmpeg archive did not contain the expected single binary and license files."
  }
  if (-not (Test-ExpectedHash $ffmpegSource[0].FullName $ffmpegSha256) -or
      -not (Test-ExpectedHash $ffprobeSource[0].FullName $ffprobeSha256) -or
      -not (Test-ExpectedHash $licenseSource[0].FullName $licenseSha256)) {
    throw "An extracted FFmpeg file did not match SlickClip's pinned SHA-256."
  }

  New-Item -ItemType Directory -Force -Path $binaryRoot | Out-Null
  Copy-Item -LiteralPath $ffmpegSource[0].FullName -Destination ($ffmpegTarget + ".partial") -Force
  Copy-Item -LiteralPath $ffprobeSource[0].FullName -Destination ($ffprobeTarget + ".partial") -Force
  Copy-Item -LiteralPath $licenseSource[0].FullName -Destination ($licenseTarget + ".partial") -Force
  [IO.File]::WriteAllText(
    ($sourceTarget + ".partial"),
    "FFmpeg build source and corresponding build scripts:`r`nhttps://github.com/BtbN/FFmpeg-Builds/tree/$releaseTag`r`n`r`nPinned binary archive:`r`n$downloadUrl`r`n`r`nFFmpeg upstream source:`r`nhttps://ffmpeg.org/download.html`r`n"
  )
  Move-Item -LiteralPath ($ffmpegTarget + ".partial") -Destination $ffmpegTarget -Force
  Move-Item -LiteralPath ($ffprobeTarget + ".partial") -Destination $ffprobeTarget -Force
  Move-Item -LiteralPath ($licenseTarget + ".partial") -Destination $licenseTarget -Force
  Move-Item -LiteralPath ($sourceTarget + ".partial") -Destination $sourceTarget -Force
  Write-Host "Staged verified FFmpeg and ffprobe sidecars for SlickClip."
}
finally {
  $resolvedTemporary = [IO.Path]::GetFullPath($temporaryRoot)
  $resolvedTempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedTemporary.StartsWith($resolvedTempBase, [StringComparison]::OrdinalIgnoreCase) -and
      ([IO.Path]::GetFileName($resolvedTemporary)).StartsWith("slickclip-ffmpeg-")) {
    Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force -ErrorAction SilentlyContinue
  }
}
