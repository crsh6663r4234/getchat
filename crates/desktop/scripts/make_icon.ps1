Add-Type -AssemblyName System.Drawing

$bitmap = [System.Drawing.Bitmap]::new(64, 64, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$graphics.Clear([System.Drawing.Color]::Transparent)

$path = [System.Drawing.Drawing2D.GraphicsPath]::new()
$path.AddArc(0, 0, 30, 30, 180, 90)
$path.AddArc(34, 0, 30, 30, 270, 90)
$path.AddArc(34, 34, 30, 30, 0, 90)
$path.AddArc(0, 34, 30, 30, 90, 90)
$path.CloseFigure()
$graphics.FillPath([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(169, 149, 255)), $path)

$pen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(32, 27, 49), 6)
$pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$graphics.DrawArc($pen, 13, 13, 36, 36, 42, 286)
$graphics.DrawLine($pen, 31, 32, 48, 32)

$iconPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\icons\ghostchat.ico'))
$handle = $bitmap.GetHicon()
$icon = [System.Drawing.Icon]::FromHandle($handle)
$stream = [System.IO.File]::Create($iconPath)
try { $icon.Save($stream) } finally { $stream.Dispose(); $icon.Dispose(); $graphics.Dispose(); $bitmap.Dispose(); $pen.Dispose(); $path.Dispose() }
Write-Output "Created $iconPath"
