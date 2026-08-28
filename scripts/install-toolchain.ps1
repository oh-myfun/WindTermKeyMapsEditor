# 本地 Rust 工具链安装（手动方式，规避 rustup 在受限/沙盒环境下行不通的情况）。
#
# 背景：本机无 MSVC，网络受限（仅 GitHub 与 rsproxy.cn 可达，rsproxy 稀疏索引不可用）。
# 采用官方发布包手动解压为 gnullvm 工具链，配合内置 rust-lld + crt-static，
# 无需 MSVC/mingw 即可产出单文件 exe。
#
# 用法：先下载下列组件到 $DownloadDir，再执行本脚本。
#   rustc-1.98.0-x86_64-pc-windows-gnullvm.tar.xz
#   cargo-1.98.0-x86_64-pc-windows-gnullvm.tar.xz
#   rust-std-1.98.0-x86_64-pc-windows-gnullvm.tar.xz
#   rust-mingw-1.98.0-x86_64-pc-windows-gnullvm.tar.xz
#
# 下载源：https://rsproxy.cn/dist/2026-08-20/<文件名>
# 常规环境请直接用 rustup： rustup toolchain install stable + rustup target add x86_64-pc-windows-gnullvm

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$ToolchainRoot = Join-Path $Root ".toolchain"
$X = Join-Path $ToolchainRoot "x"
$Out = Join-Path $ToolchainRoot "root"
$DownloadDir = Join-Path $ToolchainRoot "dists"

New-Item -ItemType Directory -Force -Path $X, $Out | Out-Null

$components = @(
    "rustc-1.98.0-x86_64-pc-windows-gnullvm.tar.xz",
    "cargo-1.98.0-x86_64-pc-windows-gnullvm.tar.xz",
    "rust-std-1.98.0-x86_64-pc-windows-gnullvm.tar.xz",
    "rust-mingw-1.98.0-x86_64-pc-windows-gnullvm.tar.xz"
)

foreach ($c in $components) {
    $arc = Join-Path $DownloadDir $c
    if (-not (Test-Path $arc)) { throw "缺少组件：$c（请先下载到 $DownloadDir）" }
    Write-Host "解压 $c"
    tar -xf $arc -C $X
}

# 各组件解压后的顶层目录名
$map = @{
    "rustc"     = "rustc-1.98.0-x86_64-pc-windows-gnullvm/rustc"
    "cargo"     = "cargo-1.98.0-x86_64-pc-windows-gnullvm/cargo"
    "rust-std"  = "rust-std-1.98.0-x86_64-pc-windows-gnullvm/rust-std-x86_64-pc-windows-gnullvm"
    "rust-mingw"= "rust-mingw-1.98.0-x86_64-pc-windows-gnullvm/rust-mingw"
}
foreach ($k in $map.Keys) {
    Copy-Item -Recurse -Force (Join-Path $X $map[$k]) $Out
}

Write-Host "工具链就绪：$Out"
& "$Out\bin\rustc.exe" --version
& "$Out\bin\cargo.exe" --version