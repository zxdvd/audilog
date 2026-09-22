# audilog

[![CI](https://github.com/zxdvd/audilog/actions/workflows/ci.yml/badge.svg)](https://github.com/zxdvd/audilog/actions/workflows/ci.yml)

极简、local-first 的 **Audio Recording + Speech Transcription CLI**。

```text
$ audilog
● Recording… Ctrl+C to stop and transcribe.
^C
✓ Audio saved
✓ Transcript saved
```

默认把电脑当录音笔。音频与转写留在本机，不需要账号，不上传音频。
Whisper 引擎内嵌，模型首次下载后可离线使用。

## 安装

从 [Releases](https://github.com/zxdvd/audilog/releases/latest) 下载对应系统的压缩包：

| 平台 | 文件后缀 | v0.1 支持 |
| --- | --- | --- |
| macOS 14.6+ Apple Silicon | `aarch64-apple-darwin.tar.gz` | 麦克风、系统声音、Metal 转写；主要验收平台 |
| macOS 14.6+ Intel | `x86_64-apple-darwin.tar.gz` | 同上，CI 编译和测试；可 `--cpu` |
| Windows x64 | `x86_64-pc-windows-msvc.zip` | 麦克风、WASAPI loopback、CPU 转写；硬件待实测 |
| Linux x64（Ubuntu 22.04+） | `x86_64-unknown-linux-gnu.tar.gz` | ALSA 麦克风、CPU 转写；不支持 `--system` |

macOS / Linux 解压后，把 `audilog` 放到 PATH，例如：

```bash
mkdir -p ~/.local/bin
install -m 755 audilog ~/.local/bin/audilog
export PATH="$HOME/.local/bin:$PATH"  # 可加入 ~/.zshrc
```

Release 附 `SHA256SUMS`。macOS 包为 ad-hoc 签名，尚未做 Apple Developer ID 公证。
如果 macOS 拦截下载的程序，先校验 SHA-256，再在系统设置允许该程序运行。
Windows 解压后将所在文件夹加入 PATH；如提示缺少运行库，安装 Microsoft Visual C++ 2015–2022 x64 Redistributable。
Linux 需要系统 `libasound2`、`libstdc++6`。

## 明天就用

```bash
audilog model install                    # 默认模型，约 547 MiB；SHA-256 校验
audilog doctor                           # 说话并播放声音，实际探测输入
audilog --language zh                     # 录麦克风，Ctrl+C 后转写
audilog record --system --language zh     # 在线会议：麦克风 + 系统声音
```

首次直接 `audilog` 也会询问是否下载模型。非交互运行加 `--yes`。
转写时再按一次 Ctrl+C 会取消推理，保留录音以便重试。

macOS 第一次使用时，允许**启动 audilog 的终端应用**访问：

- 系统设置 → 隐私与安全性 → 麦克风
- 系统设置 → 隐私与安全性 → 屏幕与系统音频录制 → 仅系统音频

具体名称可能随系统语言/版本变化。授权后重启终端并再次运行 `doctor`。
系统权限必须由用户在 macOS 中授予。首次打开设备可能停在 Opening 等待系统弹窗响应，此时尚未开始保存录音。doctor 检测到声音才报告 signal detected；
静音可能是没有播放声音，也可能是权限问题，不会据此声称已获授权或被拒绝。
系统声音录的是系统输出，不按 App 过滤；建议在线会议戴耳机，避免远端声音又进入麦克风。

## 命令

```bash
audilog                                  # 默认麦克风
audilog record --system                   # 两条独立音轨
audilog record --system-only              # 仅系统声音
audilog devices
audilog --input "MacBook Air Microphone"
audilog --output ~/Work/recordings --language zh
audilog --no-transcribe                   # 只保存录音，不加载/下载模型
audilog --seconds 60                      # 自动停止；不指定则等待 Ctrl+C
audilog transcribe interview.wav
audilog transcribe interview.m4a          # 也支持 MP3、AAC、FLAC、Ogg/Vorbis
audilog transcribe ~/Documents/audilog/2026-09-23_09-32-15

audilog model list
audilog model install small
audilog model use small
audilog --model ~/models/model.bin
audilog --cpu                             # Metal 出问题时使用 CPU
```

`--output` 是新 session 的父目录。对已有 session 转写会在原目录重建 transcript；
原音频不修改。`--language` 支持 Whisper 语言代码（`zh`、`en` 等）或 `auto`。
M4A 支持 AAC；Opus、ALAC、受保护媒体及动态采样率文件暂不支持，可先转成 WAV。

## 数据

默认 `~/Documents/audilog/YYYY-MM-DD_HH-mm-ss/`，同秒录音自动加后缀，不覆盖。
使用平台标准 Documents/cache/config 目录；Linux 没有 Documents 时请传 `--output`。

```text
2026-09-23_09-32-15/
├── audio.wav          # 单麦克风或导入文件；双轨时是 mic.wav + system.wav
├── meta.json          # 设备、共享时钟起始偏移、时长、丢帧、状态、模型、语言
├── transcript.json    # canonical format
└── transcript.md      # 可读视图
```

录音统一为 mono / 16 kHz / PCM16 WAV，约 115 MB/小时/轨。
JSON 格式：

```json
{
  "version": 1,
  "segments": [
    {"start_ms": 12140, "end_ms": 15720, "source": "system", "text": "周三上线。"},
    {"start_ms": 16420, "end_ms": 19120, "source": "microphone", "text": "什么时候能测试？"}
  ]
}
```

`source` 为 `microphone`、`system` 或 `file`。不做说话人分离。
两条录音使用同一单调时钟；转写时间加上音轨起始偏移，再排序。
这是语音段落级对齐，不承诺长时间录制时不同硬件时钟的采样级同步。

模型存放在平台 cache 下的 `audilog/models`（macOS 为 `~/Library/Caches/audilog/models`），
默认 `large-v3-turbo-q5_0`；提供 `tiny`、`small`、`large-v3-turbo`。
模型来自 [whisper.cpp 官方模型仓库](https://huggingface.co/ggerganov/whisper.cpp)，
固定 SHA-256 校验通过后才将 `.part` 原子重命名为正式模型。
自带模型路径由用户管理。默认模型不包含在二进制包中。

## 可靠性与边界

- 音频 callback 只转换样本并入有界队列，不写磁盘、不重采样、不做推理。
- worker 下混音、带限重采样、写 WAV；队列溢出会计数并尽量补静音，避免后续时间轴前移。
- WAV 每约 5 秒刷新头部；正常 Ctrl+C 会排空队列并完成文件。强制杀进程/断电可能丢失末尾数据。
- 转写失败不删除音频；用 `audilog transcribe SESSION` 重试。
- 长录音每 10 分钟一块推理控制内存；块边界的句子可能被拆开。
- 推理错误可用 `AUDILOG_DEBUG=1 audilog …` 查看底层诊断日志。
- 模型输出可能有识别错误；静音轨跳过推理。极低音量、噪声、多人同时说话会影响效果。
- CI 不具备真实麦克风/系统权限，不把 CI 通过当成硬件验证。
- V1 不做 GUI、摘要、实时字幕、会议 Bot、账号、云同步、指定 App 录音、视频录制。

## 开发与发布

Rust stable（最低声明 1.88）、CMake、C/C++ 编译器、libclang。
macOS 安装 Xcode Command Line Tools + `brew install cmake`；
Linux 安装 `libasound2-dev libclang-dev cmake`；Windows 使用 MSVC + LLVM + CMake。

```bash
cargo build --release --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

`src/capture.rs` 的 `AudioCapture` 是平台边界，向上只发送 `AudioFrame`。
其余模块负责音频转换、session、模型和 Whisper，不引用 CoreAudio/WASAPI。
当前共用 CPAL backend，后续必要时才引入独立 native backend。

GitHub Actions 在 macOS ARM/Intel、Linux、Windows 上检查格式、Clippy、测试与 release 构建；
Linux 还会下载 tiny 模型，对公开 JFK 样本进行真实 CPU 转写断言。
更新 Cargo 版本、`CHANGELOG.md`、`RELEASE.md` 后推送 `vX.Y.Z` tag，
全部构建成功才发布压缩包和 SHA256SUMS。

上游：[CPAL](https://github.com/RustAudio/cpal)、[whisper-rs](https://docs.rs/whisper-rs/)、
[whisper.cpp](https://github.com/ggml-org/whisper.cpp)。MIT License。
