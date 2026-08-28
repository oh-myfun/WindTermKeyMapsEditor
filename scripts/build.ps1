CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG=false
# 本地 gnullvm 构建（无需 rustup；已通过 .cargo/config.toml 指定链接器与 crt-static）
$env:PATH="$PSScriptRoot\..\.toolchain\root\bin;$env:PATH"
$env:CARGO_HOME="$PSScriptRoot\..\.toolchain\cargo-home"
# 说明：cargo(gnullvm 构建) 的 host 即 gnullvm，故无需设置 CARGO_BUILD_TARGET。
Write-Output "Rust 工具链："
rustc --version
cargo --version
Write-Output "开始构建（后台请见 job 日志）..."
cargo build --release