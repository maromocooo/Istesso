<!-- translation-of: README.md @ blob 1f6e2c134bed3bf58252a9a0f86f648a5f34c7db (2026-11-20) -->
> 本文根据 2026-11-20 的[英文 README](README.md) 翻译，以英文版为准。本文由机器翻译，另一个 AI 已对照原文复核，尚未经中文母语者审校。

# Istesso

[English](README.md) · [日本語](README.ja.md) · **简体中文** · [한국어](README.ko.md) · [Español](README.es.md)

Istesso 通过自己的加密通道，把其他电脑上运行的应用显示为眼前这台电脑上的普通窗口。它只有一条标准：只要能感觉到是在远程操作，哪怕只是一点，也算失败。

<!-- 待添加的图片（docs/images/）：截取屏幕的一部分，展示三个外观相同的窗口。
     三个应用分别在本机、Linux 主机和 Mac 主机上运行，只在图外标注。
     只使用 Istesso 自己的测试应用和虚构的内容。 -->

> [!IMPORTANT]
> Istesso 目前处于试用阶段。作者每天都在使用它，看看还有哪些不足。许多相关改动仍在开发分支上，尚未合入 `main`。目前没有可供下载的二进制文件，需要从源码构建。下表列出了现在哪些功能可以在哪些机器上使用。

## Istesso 是什么

Istesso 的设计目标是让每项工作都在延迟最小的地方完成，依次考虑以下三种方式：

1. 能在眼前这台电脑上运行的，就在本机运行。只与其他电脑同步状态。
2. 纯文本工作通过终端直接连接。
3. 余下的是运行在其他机器上、又需要图形界面的应用。只有这些应用才通过视频转发，每个应用对应一个窗口，指针在本地绘制。

这条标准可以用实验检验：你应该分不出哪个窗口来自远程。测试时，在同一块屏幕上，测量并比较转发的窗口与同一应用在本地运行的窗口，再让人尝试用肉眼辨认哪个是转发的。这条标准适用于通过家庭有线网络相连的机器。Wi-Fi 和互联网连接不在范围内。

名字取自音乐术语 *l'istesso tempo*，意思是即使拍子改变，速度也保持不变。

## 目前可用的功能

每列都标明应用在哪里运行，以及你在哪台设备上看到它。

| 功能 | Linux 主机 → Mac | Mac 主机 → Mac | Linux 主机 → Android XR 头显 |
|---|---|---|---|
| 用一次性配对码为两台机器配对 | `Verified` 2026-11-16（[记录](docs/milestones.md#m0-共通の土台) (日语)） | `Trial` | `Trial` |
| 在同一份列表中查看所有机器及其应用 | `Verified` 2026-11-16（[记录](docs/milestones.md#m0-共通の土台) (日语)） | `Trial` | `Planned` |
| 在主机上打开应用，在一个窗口里看画面、用键盘和指针操作、滚动内容 | `Trial` | `Trial` | `Trial` |
| 在该窗口与本机之间复制和粘贴文本 | `Trial` | `Trial` | `Planned` |
| 通过输入法输入日语 | `Trial` | `Trial` | `Trial` |
| 在本地听到主机的声音 | `Planned` | `Planned` | `Planned` |
| 从启动器窗口打开应用，无论它安装在哪台机器上 | `Planned` | `Planned` | `Planned` |
| 将 USB 设备交给另一台机器，让它看起来像是插在那台机器上 | `Out of scope` | `Out of scope` | `Out of scope` |

- `Verified`（已验证）：已从 `main` 构建，在担任相应角色的真实机器上按读者会使用的步骤运行过。一定会附上日期和记录链接。
- `Trial`（试用中）：在作者从开发分支构建的试用版上可以使用。目前还无法从 `main` 复现。
- `Planned`（已列入计划）：正在设计，或尚未开始。
- `Out of scope`（不在范围内）：不属于本次发布的内容。

头显通过 Wi-Fi 连接，不在延迟标准的适用范围内（该标准针对家庭有线网络）。头显一列中的 `Trial` 只表示该功能在试用版中可用，并不说明它是否达到延迟标准。

延迟的比较基准是同一应用在同一块屏幕上本地运行时的窗口。两台机器均使用有线连接，显示器刷新率为 85 Hz，画面为 3440×1440。转发所增加的延迟，中位数不得超过 12 ms，第 95 百分位数不得超过 24 ms，第 99 百分位数不得超过 47 ms。2026-11-17，使用原型查看器和专为测量搭建的主机，转发通过了这一阶段的测试。只有用 Istesso 自己的客户端和主机实测并作出最终判定后，才能验收（[阶段测试记录](docs/measurements/2026-11-17-m1-gate.md) (日语)）。

## 后续计划

- 将 Istesso 自己的客户端和主机合入 `main`，然后对延迟作最终判定。
- 将任意主机的声音送到你选定的一个输出设备，连续播放数小时而不中断。
- 提供跨机器的启动器，可以唤醒休眠中的主机。
- 在机器之间共享存储，并使用专用用户账号，将 Istesso 的应用和代理程序与你自己的文件隔离。
- 在适合的机器上运行构建和作业。
- 无需挪动设备即可跨机器使用。所有机器的设备都列在一起。需要某个设备的应用在插有该设备的机器上打开，你仍在本地操作应用窗口。首个直接控制的设备是可编程小键盘。如表中所示，将 USB 设备本身交给另一台机器不在范围内。
- 将 Android XR 头显客户端合入 `main`。
- 提供 macOS 和 Ubuntu 的签名软件包及更新，并对配对机制进行外部审查。
- 本次发布之后再支持 Windows。

各个阶段及其必须通过的测试见 [docs/milestones.md](docs/milestones.md) (日语)。

## 已知限制

- Istesso 无法防范同一台机器上以同一用户身份运行的其他程序。请在自己的机器上使用，并确保机器上只运行你信任的程序。
- 延迟标准适用于家庭有线网络。Wi-Fi 和互联网不在范围内（[关于 Wi-Fi 的说明](docs/wifi.md) (日语)）。
- Linux 主机需要配备带有 NVENC 硬件视频编码器的 NVIDIA GPU。
- Mac 主机重启后，必须先有人在这台 Mac 上登录一次，才能使用。
- Mac 作主机时，macOS 每隔 30 天就会在该 Mac 的屏幕上请求确认，询问是否允许应用继续在不使用系统选择界面的情况下捕获屏幕。这是 Apple 对采用这种捕获方式的应用的规定，并非 Istesso 增设的步骤。从源码构建的版本也会收到询问。目前不分发签名构建版本，Istesso 的分发方式仍未确定。
- 目前没有安装程序、自动更新或签名二进制文件。配对机制尚未经过外部审查。
- 目前的延迟数据来自原型查看器。是否验收，要等最终判定。
- 本次发布不支持 Windows。

## 与同类工具有何不同

| 工具类型 | 它能提供什么 | Istesso 的做法 | 何时这类工具更合适 |
|---|---|---|---|
| 远程桌面（VNC、RDP 等） | 可使用另一台机器的整个桌面，包括登录界面，也可通过互联网使用 | 在自己的桌面上，每个应用都是一个窗口，与本地窗口及来自其他机器的窗口并排显示 | 需要整个桌面、登录界面、通过互联网使用或使用 Windows |
| 通过 ssh 连接终端，配合终端复用器 | 通过任何链路以尽可能低的延迟处理文本，即使连接断开，会话也能保留 | Istesso 不取代它。通过终端处理文本是三层设计中的第二层，只有需要图形界面的应用才传视频 | 工作内容是文本 |
| 窗口转发（例如通过 ssh 转发 X11） | 通过 ssh 连接，将远程 X11 应用的窗口逐个显示在本地屏幕上 | 每个窗口都以硬件编码的视频传输，并与本地窗口对照测量 | 使用的是 X11 应用，只想配置 ssh 连接 |
| 游戏串流 | 支持全屏、鼠标相对位移、游戏手柄和通过互联网游玩 | 为应用窗口而设计，传递指针的绝对位置，光标在本地绘制。游戏不在本次发布范围内 | 想玩游戏 |

## 快速开始

以下步骤只用一台机器，只展示 Istesso 在这台机器上找到的内容：它可以担任的角色、已安装的应用和已接入的设备。连接两台机器并显示远程应用的步骤还没有在全新克隆的仓库中重新跑过，因此暂未列入本 README。

需要 `git`、Rust 1.85 或更新版本，以及 C 工具链（在 macOS 上为 Xcode Command Line Tools）。

```sh
git clone https://github.com/maromocooo/Istesso.git
cd Istesso
cargo build --release --locked -p istesso-cli
./target/release/istesso doctor
./target/release/istesso apps list
./target/release/istesso devices list
```

- `doctor` 输出这台机器可以担任的角色，例如是否配备硬件视频编码器、能否为 Apple 平台构建软件，以及是否适合作为常开主机。输出也包含机器名称和网络地址，粘贴到任何地方之前请先检查内容。
- `apps list` 列出这台机器上安装的应用。
- `devices list` 列出接入这台机器的设备。

上一次从全新克隆的仓库运行这些步骤是在 2026-11-19，使用的是 macOS 26.5（Apple silicon）（[记录](docs/readme-checks.md) (日语)）。

## 从源码构建

- 需要 Rust 1.85 或更新版本。可在 macOS 和 Linux 上构建。每次改动，CI 都会在这两个平台上构建并测试。
- 目前未发布签名二进制文件，也尚未决定 Istesso 的分发方式。

提交改动前，请运行以下命令。CI 运行相同的检查。

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

`cargo deny check` 需要 [cargo-deny](https://github.com/EmbarkStudios/cargo-deny)。

## 安全

- 配对时，一台机器显示由八个字符组成的一次性配对码，你在另一台机器上输入。两台机器使用这个码执行口令认证密钥交换（PAKE，采用 SPAKE2），然后交换并固定彼此的长期密钥（[ADR 0006](docs/adr/0006-pairing.md) (日语)）。
- 此后每次连接都通过固定的密钥进行双向身份验证，并使用 Noise 协议框架加密。画面也用同一组密钥加密传输（[ADR 0007](docs/adr/0007-control-channel.md)、[ADR 0015](docs/adr/0015-media-transport.md) (日语)）。
- 刚配对的机器只获得查看和控制权限。每项权限都可以在授予它的机器上单独撤销。

配对机制尚未经过外部审查，计划在发布前接受审查。Istesso 无法防范同一台机器上以同一用户身份运行的其他程序。

如需报告漏洞，请使用 GitHub 的私密漏洞报告功能，在 Security 选项卡中选择“Report a vulnerability”。请勿为此创建公开的 issue。

## 文档

详细文档以日语编写。

- [需求](docs/requirements.md) (日语)
- [里程碑及其验收测试](docs/milestones.md) (日语)
- [架构](docs/architecture.md) (日语)
- [设计决策记录（ADR）](docs/adr/) (日语)
- [测量记录](docs/measurements/) (日语)
- [组件及其许可证](docs/components.md) (日语)

## 参与贡献

- 开发者和编程代理的工作规则见 [AGENTS.md](AGENTS.md) (日语)。请在分支上工作，并为新行为添加测试。
- 不引入 GPL 或 AGPL 代码。使用外部代码之前，请对照 [docs/components.md](docs/components.md) (日语) 核查。借用规范或协议之前，除许可证外，还要检查其专利条款。不明确的就不采用。
- 如果改动涉及视频、输入或声音的传输路径，请在拉取请求中给出改动前后的测量结果。如果无法测量，请在请求中说明。
- 设备适配器的接口尚未定型，目前不接受添加此类适配器的拉取请求（[ADR 0012](docs/adr/0012-device-framework.md) (日语)）。

## 许可证

Istesso 采用 MIT License 或 Apache License 2.0，你可以任选其一。见 [LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)。

## 作者

[maromocooo](https://github.com/maromocooo)

## 商标

Apple、Mac、macOS 和 Xcode 是 Apple Inc. 的商标。Ubuntu 是 Canonical Ltd. 的商标。Linux 是 Linus Torvalds 的注册商标。NVIDIA 和 NVENC 是 NVIDIA Corporation 的商标。Android 是 Google LLC 的商标。Rust 是 Rust Foundation 的商标。Istesso 与上述商标权利人均无关联，也未获其背书。
