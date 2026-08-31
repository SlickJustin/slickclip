[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

Add-Type -AssemblyName System.Drawing

$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$iconPath = Join-Path $projectRoot "src-tauri\icons\icon.png"
$configPath = Join-Path $projectRoot "src-tauri\tauri.conf.json"
$outputRoot = [IO.Path]::GetFullPath((Join-Path $projectRoot "src-tauri\icons\installer"))
if (-not $outputRoot.StartsWith($projectRoot, [StringComparison]::OrdinalIgnoreCase)) {
  throw "Refused to generate installer artwork outside the SlickClip project."
}
if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) {
  throw "The SlickClip source icon is missing."
}

$config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
$version = [string]$config.version
New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null

function New-RgbBitmap([int]$Width, [int]$Height) {
  return [Drawing.Bitmap]::new(
    $Width,
    $Height,
    [Drawing.Imaging.PixelFormat]::Format24bppRgb
  )
}

function Set-Quality([Drawing.Graphics]$Graphics) {
  $Graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $Graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $Graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $Graphics.TextRenderingHint = [Drawing.Text.TextRenderingHint]::ClearTypeGridFit
}

function Save-Bitmap([Drawing.Bitmap]$Bitmap, [string]$Path) {
  $Bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Bmp)
}

$icon = [Drawing.Image]::FromFile($iconPath)
try {
  $sidebar = New-RgbBitmap 164 314
  try {
    $graphics = [Drawing.Graphics]::FromImage($sidebar)
    try {
      Set-Quality $graphics
      $bounds = [Drawing.Rectangle]::new(0, 0, 164, 314)
      $background = [Drawing.Drawing2D.LinearGradientBrush]::new(
        $bounds,
        [Drawing.Color]::FromArgb(10, 9, 15),
        [Drawing.Color]::FromArgb(29, 18, 48),
        90
      )
      try { $graphics.FillRectangle($background, $bounds) } finally { $background.Dispose() }

      $violetGlow = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(38, 128, 78, 224))
      $violetPanel = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(48, 112, 65, 204))
      $purpleLine = [Drawing.Pen]::new([Drawing.Color]::FromArgb(150, 132, 87, 229), 1)
      $mutedLine = [Drawing.Pen]::new([Drawing.Color]::FromArgb(55, 196, 173, 255), 1)
      try {
        $graphics.FillEllipse($violetGlow, -15, 5, 194, 150)
        $graphics.FillPolygon($violetPanel, @(
          [Drawing.Point]::new(0, 248),
          [Drawing.Point]::new(50, 201),
          [Drawing.Point]::new(164, 201),
          [Drawing.Point]::new(164, 314),
          [Drawing.Point]::new(0, 314)
        ))
        $graphics.DrawLine($purpleLine, 0, 244, 53, 194)
        $graphics.DrawLine($purpleLine, 53, 194, 164, 194)
        $graphics.DrawLine($mutedLine, 18, 16, 146, 16)
        $graphics.DrawLine($mutedLine, 18, 297, 146, 297)
      }
      finally {
        $violetGlow.Dispose()
        $violetPanel.Dispose()
        $purpleLine.Dispose()
        $mutedLine.Dispose()
      }

      $graphics.DrawImage($icon, [Drawing.Rectangle]::new(38, 29, 88, 88))

      $center = [Drawing.StringFormat]::new()
      $center.Alignment = [Drawing.StringAlignment]::Center
      $center.LineAlignment = [Drawing.StringAlignment]::Center
      $titleFont = [Drawing.Font]::new("Segoe UI", 15, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
      $strapFont = [Drawing.Font]::new("Segoe UI", 8, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
      $detailFont = [Drawing.Font]::new("Segoe UI", 8, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
      $white = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(246, 244, 252))
      $purple = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(158, 113, 244))
      $muted = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(174, 168, 190))
      try {
        $graphics.DrawString("SLICKCLIP", $titleFont, $white, [Drawing.RectangleF]::new(0, 128, 164, 25), $center)
        $graphics.DrawString("CAPTURE  /  SAVE  /  REPLAY", $strapFont, $purple, [Drawing.RectangleF]::new(0, 154, 164, 18), $center)
        $graphics.DrawString("Made to capture the DAWGs`nworst moments.", $detailFont, $white, [Drawing.RectangleF]::new(12, 219, 140, 34), $center)
        $graphics.DrawString("VERSION $version", $strapFont, $muted, [Drawing.RectangleF]::new(0, 270, 164, 18), $center)
      }
      finally {
        $center.Dispose()
        $titleFont.Dispose()
        $strapFont.Dispose()
        $detailFont.Dispose()
        $white.Dispose()
        $purple.Dispose()
        $muted.Dispose()
      }
    }
    finally { $graphics.Dispose() }
    Save-Bitmap $sidebar (Join-Path $outputRoot "sidebar.bmp")
  }
  finally { $sidebar.Dispose() }

  $header = New-RgbBitmap 150 57
  try {
    $graphics = [Drawing.Graphics]::FromImage($header)
    try {
      Set-Quality $graphics
      $bounds = [Drawing.Rectangle]::new(0, 0, 150, 57)
      $background = [Drawing.Drawing2D.LinearGradientBrush]::new(
        $bounds,
        [Drawing.Color]::FromArgb(13, 12, 19),
        [Drawing.Color]::FromArgb(39, 23, 65),
        0
      )
      try { $graphics.FillRectangle($background, $bounds) } finally { $background.Dispose() }

      $accent = [Drawing.Pen]::new([Drawing.Color]::FromArgb(132, 87, 229), 2)
      try {
        $graphics.DrawLine($accent, 0, 55, 150, 55)
        $graphics.DrawLine($accent, 50, 0, 50, 57)
      }
      finally { $accent.Dispose() }

      $graphics.DrawImage($icon, [Drawing.Rectangle]::new(8, 8, 40, 40))
      $titleFont = [Drawing.Font]::new("Segoe UI", 12, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
      $detailFont = [Drawing.Font]::new("Segoe UI", 8, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
      $white = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(248, 247, 252))
      $purple = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(177, 139, 250))
      try {
        $graphics.DrawString("SLICKCLIP", $titleFont, $white, 58, 10)
        $graphics.DrawString("SETUP  /  $version", $detailFont, $purple, 59, 30)
      }
      finally {
        $titleFont.Dispose()
        $detailFont.Dispose()
        $white.Dispose()
        $purple.Dispose()
      }
    }
    finally { $graphics.Dispose() }
    Save-Bitmap $header (Join-Path $outputRoot "header.bmp")
  }
  finally { $header.Dispose() }
}
finally {
  $icon.Dispose()
}

Write-Host "Generated SlickClip NSIS artwork in $outputRoot"
