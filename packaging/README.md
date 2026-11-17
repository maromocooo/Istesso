# packaging/

M9（配布と公開）の下ごしらえです。設計は [docs/m9-plan.md](../docs/m9-plan.md) にあります。ここにあるものは、どれも何も入れず、公開もしません。

## 使い方

| ファイル | 何か | 使い方 |
|---|---|---|
| `macos/build-pkg.sh` | ビルド済みの `.app` から `.pkg` を作る | `packaging/macos/build-pkg.sh <Istesso.app> --version 0.1.0 --out dist` |
| `macos/dev.istesso.agent.plist` | launchd のエージェント（ログイン時に `istesso agent` を起動） | `build-pkg.sh` が `/Library/LaunchAgents/` に入る形で包む |
| `macos/pkg-scripts/` | `.pkg` の前後の処理（エージェントを止める、起動する） | `build-pkg.sh` が使う |
| `macos/uninstall.sh` | 入れたものを外す | `packaging/macos/uninstall.sh`（何をするか表示するだけ）。実行は `--yes` |
| `ubuntu/build-deb.sh` | ビルド済みのプログラムから `.deb` を作る | `packaging/ubuntu/build-deb.sh --version 0.1.0 --out dist target/release/istesso` |
| `ubuntu/systemd/istesso-agent.service` | systemd のユーザーユニット | `.deb` が `/usr/lib/systemd/user/` に入れる。有効にはしない |
| `ubuntu/etc/40-istesso-limits.conf` | グループ `istesso` にリアルタイム優先度と nice だけを許す | `.deb` が `/etc/security/limits.d/` に入れる |
| `ubuntu/etc/70-istesso-usb.rules` | 対応 USB 機器をグループ `plugdev` に開く udev の決まり | `.deb` が `/usr/lib/udev/rules.d/` に入れる |
| `ubuntu/debian/` | `.deb` の control、postinst、postrm | `build-deb.sh` が使う |

## macOS の署名（環境変数）

何も設定しないと、署名なしで作り、そう表示します。署名するときは、次の環境変数で証明書を指します。証明書の名前には持ち主の名前が入るので、スクリプトにも文書にもログにも書きません。**ハッシュ（40 桁の 16 進数）で指してください。**

| 変数 | 中身 |
|---|---|
| `ISTESSO_SIGN_APP_IDENTITY` | Developer ID Application 証明書。バンドル全体に署名する |
| `ISTESSO_SIGN_INSTALLER_IDENTITY` | Developer ID Installer 証明書。`.pkg` に署名する |
| `ISTESSO_ENTITLEMENTS` | 任意。entitlements のファイル |
| `ISTESSO_NOTARY_PROFILE` | `notarytool store-credentials` で作った、キーチェーンのプロファイル名。あれば公証してステープルまで行う |

署名は持ち主の Mac で、持ち主の鍵で行います。鍵と証明書は、リポジトリにも CI にも置きません。手順は m9-plan.md の 3 にあります。

## Ubuntu

- `build-deb.sh` は `dpkg-deb` のある Ubuntu か Debian で動かします（Mac には入っていません）。
- `ISTESSO_DEB_DEPENDS` で `Depends` を指定できます。リリースでは `dpkg-shlibdeps` の結果を入れます。
- `.deb` 自体には署名しません。署名は、パッケージのリポジトリで行います（m9-plan.md の 2）。

## 決まり

- スクリプトの中に `sudo` はありません。root が要るところは、そう表示して止まります。
- 消すのは、スクリプトが作った作業用フォルダーの中だけです（消す前に、その場所かを確かめます）。
- アンインストールは、何を外すかを表示するだけです。`--yes` を付けたときだけ実行します。

## 確かめたこと（2026-11-17、Mac 上のみ）

- 全スクリプトに `bash -n`。plist に `plutil -lint`。`shellcheck` は Mac になかったので実行していません。
- `build-pkg.sh` を、ダミーのバンドルで署名なしのまま動かし、`.pkg` ができて、内容（`/Applications`、`/Library/LaunchAgents`、`/usr/local/bin` のリンク、前後の処理、再配置しない設定）を `pkgutil` で見ました。インストールはしていません。署名と公証の枝は、動かしていません。
- `build-deb.sh` は、`dpkg-deb` の代わりに中身を書き出すだけの偽物を使って、作業用フォルダーの中身と `control` を確かめました。本物の `.deb` は作っていません。
- `.pkg` の中身の一覧に `._*` が出ます。ファイルの属性（`com.apple.provenance`）が原因とみています。実際の入り方は、署名つきのビルドで確かめます。
