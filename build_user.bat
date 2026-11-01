@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

if not exist "Cargo.toml" (
    echo 没有找到 Cargo.toml，请把本脚本放在项目根目录下运行。
    pause
    exit /b 1
)

set "PLAT="
set "ARCH="
set "TRIPLE="
set "SUFFIX="
set "CCPROBE="
set "GUIDE="

cls
echo.
echo   Guobeng Music Server 构建
echo   --------------------------------------------------
echo   本机默认工具链：x86_64-pc-windows-gnu + MinGW-w64
echo.
echo   [1] Windows
echo   [2] Linux
echo   [3] macOS
echo.
set /p "PLAT=   目标平台 (1/2/3)："
if "%PLAT%"=="1" goto :ask_arch
if "%PLAT%"=="2" goto :ask_arch
if "%PLAT%"=="3" goto :ask_arch
echo.
echo   无效选择。
goto :fail

:ask_arch
echo.
echo   [1] x86_64  (Intel / AMD)
echo   [2] arm64   (Apple Silicon / 鲲鹏 / 骁龙)
echo.
set /p "ARCH=   目标架构 (1/2)："
if "%ARCH%"=="1" goto :map
if "%ARCH%"=="2" goto :map
echo.
echo   无效选择。
goto :fail

:map
if "%PLAT%"=="1" goto :map_win
if "%PLAT%"=="2" goto :map_linux
goto :map_mac

:map_win
set "SUFFIX=.exe"
if "%ARCH%"=="1" set "TRIPLE=x86_64-pc-windows-gnu"
if "%ARCH%"=="1" set "CCPROBE=x86_64-w64-mingw32-gcc"
if "%ARCH%"=="1" set "GUIDE=安装 MinGW-w64 并把 bin 目录加入 PATH（本机为 D:\App\MinGW\bin）"
if "%ARCH%"=="2" set "TRIPLE=aarch64-pc-windows-gnullvm"
if "%ARCH%"=="2" set "CCPROBE=clang"
if "%ARCH%"=="2" set "GUIDE=需要 LLVM：安装 clang 与 lld 并加入 PATH（Windows arm64 走 gnullvm 工具链）"
goto :ready

:map_linux
if "%ARCH%"=="1" set "TRIPLE=x86_64-unknown-linux-musl"
if "%ARCH%"=="1" set "CCPROBE=x86_64-linux-musl-gcc"
if "%ARCH%"=="2" set "TRIPLE=aarch64-unknown-linux-musl"
if "%ARCH%"=="2" set "CCPROBE=aarch64-linux-musl-gcc"
set "GUIDE=需要对应架构的 musl 交叉编译器（sqlx 内置的 SQLite 要用 C 编译器）；最省事是在目标机器上直接构建"
goto :ready

:map_mac
if "%ARCH%"=="1" set "TRIPLE=x86_64-apple-darwin"
if "%ARCH%"=="1" set "CCPROBE=o64-clang"
if "%ARCH%"=="2" set "TRIPLE=aarch64-apple-darwin"
if "%ARCH%"=="2" set "CCPROBE=aarch64-apple-darwin-clang"
set "GUIDE=需要 macOS SDK + osxcross，在 Windows 上一般无法构建，建议在 Mac 上直接 cargo build"
goto :ready

:ready
echo.
echo   目标三元组：%TRIPLE%
echo.

set "HAVE="
for /f "delims=" %%T in ('rustup target list --installed 2^>nul') do (
    if /i "%%T"=="%TRIPLE%" set "HAVE=1"
)
if defined HAVE goto :check_cc

echo   Rust 标准库还没装这个目标。
set /p "ADD=   现在安装吗？(Y/n)："
if /i "!ADD!"=="n" goto :fail
rustup target add %TRIPLE%
if errorlevel 1 (
    echo.
    echo   目标安装失败，请检查网络或 rustup 配置。
    goto :fail
)

:check_cc
where %CCPROBE% >nul 2>nul
if not errorlevel 1 goto :build
echo   [缺少编译工具] PATH 里找不到 %CCPROBE%
echo   %GUIDE%
echo.
set /p "GOON=   仍然尝试构建吗？(y/N)："
if /i not "!GOON!"=="y" goto :fail

:build
echo.
echo   开始构建 %TRIPLE% ...
echo.
cargo build --release --target %TRIPLE%
if errorlevel 1 goto :fail

set "OUT=target\%TRIPLE%\release\guobeng-music-server%SUFFIX%"
if not exist "%OUT%" (
    echo.
    echo   构建已结束，但没有找到产物：%OUT%
    goto :fail
)

echo.
echo   --------------------------------------------------
echo   构建成功：%OUT%
for %%F in ("%OUT%") do echo   文件大小：%%~zF 字节
echo   --------------------------------------------------
echo.
pause
exit /b 0

:fail
echo.
echo   构建未完成。
echo.
pause
exit /b 1
