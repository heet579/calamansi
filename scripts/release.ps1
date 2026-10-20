# Build Calamansi, package the ready-to-run zip and publish it as a GitHub release.
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1            (v0.1.0)
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 v0.2.0
# Needs: Rust, Git, and the GitHub CLI signed in (gh auth login).
param([string]$Version = "v0.1.0")
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

Write-Host "1/5  Stopping any running Calamansi..."
taskkill /IM Calamansi.exe /F 2>$null | Out-Null
Start-Sleep -Milliseconds 500

Write-Host "2/5  Building (this takes a minute)..."
cmd /c "cargo build --release -p calamansi-win -p calamansi-cli"
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

Write-Host "3/5  Packaging..."
$name  = "Calamansi-$Version-windows-x64"
$stage = Join-Path "target\dist" $name
$zip   = "target\dist\$name.zip"
$sums  = "target\dist\SHA256SUMS.txt"
if (Test-Path "target\dist") { Remove-Item "target\dist" -Recurse -Force }
New-Item -ItemType Directory $stage -Force | Out-Null
Copy-Item "target\release\Calamansi.exe", "target\release\calamansi-cli.exe", "LICENSE", "THIRD_PARTY_NOTICES.md" $stage
Copy-Item "docs\START-HERE.txt" (Join-Path $stage "START HERE.txt")
Compress-Archive -Path $stage -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $name.zip" | Set-Content $sums -Encoding ascii

Write-Host "4/5  Pushing code and tag $Version..."
git push origin HEAD
if ($LASTEXITCODE -ne 0) { throw "git push failed" }
git rev-parse -q --verify "refs/tags/$Version" | Out-Null
if ($LASTEXITCODE -ne 0) { git tag $Version }
git push origin $Version
if ($LASTEXITCODE -ne 0) { throw "pushing the tag failed" }

Write-Host "5/5  Creating the GitHub release..."
gh release create $Version $zip $sums --title "Calamansi $Version (preview)" --notes-file "docs\RELEASE-NOTES.md" --prerelease
if ($LASTEXITCODE -ne 0) { throw "gh release create failed (is gh signed in? run: gh auth login)" }

Write-Host ""
Write-Host "Done. Ready-to-run app: $((Resolve-Path $stage).Path)\Calamansi.exe"
gh release view $Version --web
explorer.exe (Resolve-Path $stage).Path
