# SD-90 の VST3 プラグインを .vst3 バンドルの形に組む。
#
# .vst3 は DLL 1 個ではなくフォルダ。中身は
#   SD-90.vst3\Contents\x86_64-win\SD-90.vst3   <- 拡張子 .vst3 の DLL
# DLL 単体を置いても拾うホストはあるが、規格はこちら。
#
#   .\install.ps1              ワークスペースの target\release にバンドルを組むだけ
#   .\install.ps1 -Install     共通の VST3 フォルダにコピーする (管理者権限)

param([switch]$Install)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

cargo build --release --manifest-path "$root\Cargo.toml"

# ワークスペースなので target はリポジトリのルートにできる。
$target = "$root\..\target\release"
$dll = "$target\sd90_vst.dll"
if (-not (Test-Path $dll)) { throw "$dll が無い" }

$bundle = "$target\SD-90.vst3"
$binDir = "$bundle\Contents\x86_64-win"
New-Item -ItemType Directory -Force $binDir | Out-Null
Copy-Item $dll "$binDir\SD-90.vst3" -Force
Write-Host "組んだ: $bundle"

if ($Install) {
    $dest = "$env:CommonProgramFiles\VST3"
    if (-not (Test-Path $dest)) { throw "$dest が無い" }
    Copy-Item $bundle $dest -Recurse -Force
    Write-Host "入れた: $dest\SD-90.vst3"
    Write-Host "DAW 側でプラグインを再スキャンすること。"
}
