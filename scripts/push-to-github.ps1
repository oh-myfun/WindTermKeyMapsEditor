# 在「能联网的终端」运行的推送脚本：创建公开仓库并推送 main，可选打 tag 触发 CI Release。
# 前置：已安装 gh 且已认证（gh auth login），或在系统有 git 凭证基础上使用。
# 用法（在项目根目录执行）：.\scripts\push-to-github.ps1
# 可选参数：-Tag v0.1.0 推送并打 tag 触发自动发布；-Repo windtermkeymapseditor 指定仓库名（默认小写化）。

param(
    [string]$Repo = "WindTermKeyMapsEditor",
    [string]$Tag = ""
)

$ErrorActionPreference = "Stop"
Push-Location (Split-Path $PSScriptRoot -Parent)

Write-Host ">>> 用 gh 创建公开仓库 $Repo 并推送 main ..." -ForegroundColor Cyan
gh repo create "$Repo" --public --source=. --remote=origin --push
if ($LASTEXITCODE -ne 0) { throw "gh repo create/push 失败（请先 gh auth login）" }

Write-Host ">>> main 已推送。若 spec 分支残留则忽略。"

if ($Tag -ne "") {
    Write-Host ">>> 打 tag $Tag 触发 GitHub Actions 自动构建 + Release ..." -ForegroundColor Cyan
    git tag -a $Tag -m "release $Tag"
    git push origin $Tag
    if ($LASTEXITCODE -ne 0) { throw "tag 推送失败" }
    Write-Host ">>> 已推送 $Tag，可在 https://github.com/oh-myfun/$Repo/actions 查看流水线。"
} else {
    Write-Host ">>> 完成。想要自动发布，可再跑： .\scripts\push-to-github.ps1 -Tag v0.1.0"
}

Pop-Location