# ランチャーの窓の試作：どの機械のアプリも、1 つの一覧から開く（【提案】）

- 作成：2026-11-20。状態：**試作の計画。決まっていない。** 決めるのは、持ち主
- ブランチ：`proto-launcher-window`（土台は `proto-w1-projects`。W1 のプロジェクトの一覧を、そのまま使うため）
- 関係：[m6-plan.md](m6-plan.md) の「プロジェクトの一覧（W1 の試作）」。ランチャーの画面の提案（`m3-launcher` ブランチの `docs/m3-launcher.md`）。試用の開き方（`trial-daily-next` ブランチの `scripts/daily.sh`）。ADR 0025 と ADR 0026、作業場の段の進め方（`design-workspace-stage` ブランチ）

## 1. ねらい

Air の窓 1 つに、次を 1 つの一覧で出す。検索して、Return で開く。

1. Ubuntu のアプリ（試用で開けるもの）
2. Studio のアプリ（試用で開けるもの）
3. Air のアプリ
4. W1 のプロジェクト（どの機械の持ち物か、答えているか）

M3 のランチャーの提案（自前のクライアントに組み込む、セッションの頼みを使う）の、手前の試作である。製品の作りは、提案のまま変えない。

## 2. いちばん小さい切り口

| 入れる | 入れない（あと） |
|---|---|
| 検索欄と、1 本の一覧（見出しは `Apps` と `Projects`） | 同じアプリを 1 つのアイコンにまとめること（M3 の提案の 3.2.1） |
| 行ごとに、名前と、どの機械か（`on Ubuntu` など） | アイコンの絵。いま動いているもの（`Running`） |
| アプリ：Return かダブルクリックで開く | 大きさを選ぶ画面。起こす操作。近道のキー。メニューバー |
| プロジェクト：シェルを開くコマンド（`ish 名前`）を写す。答えない機械には `!`、名前が重なると `?` | プロジェクトを、窓から直接開くこと（W2b、W3 のあと） |
| 窓の下の 1 行に、いまの様子（開いている、開けなかった、写した） | 持ち主のアプリ（Safari、メモ、Finder、プレビュー、Dia）と、画面全体（`desktop`）。いままでどおり、持ち主の近道とターミナルから |

## 3. 一覧の出どころ（どれも、いまあるもの）

| 行 | 出どころ | 開き方 |
|---|---|---|
| Ubuntu のアプリ | 試用の `scripts/daily.sh apps` の出力（`名前<TAB>表示名`） | 試用の `scripts/daily.sh open 名前 --host ubuntu` |
| Studio のアプリ | 試用の `macos/daily-shortcuts/mac-apps.txt`（近道と同じ一覧） | 試用の `scripts/daily.sh open 名前 --host mac`（中で、Mac ホストの `daily-mac.sh` を呼ぶ） |
| Air のアプリ | `istesso-catalog` の、この Mac の一覧（`/Applications` など）。試用の近道（`com.maromoco.istesso-dev.` で始まるもの）は除く | `/usr/bin/open -a <その束>` |
| プロジェクト | W1 の `istesso projects --json` | 開かない。`ish 名前`（重なるときは `ish 機械:名前`）を写すだけ |

- 機械の表示名は、試用の近道と同じ `~/.config/istesso/daily-machine-labels.txt` を読む（読むだけ）。なければ `Ubuntu`、`Studio`。
- **ほかの機械で何かを始める、新しい道は作らない。** 窓が使うのは、試用の `daily.sh` と、手元の `open` だけ。

## 4. 作り

- **Rust（`istesso-cli`）**：`istesso launcher apps [--trial DIR] [--json]` が、上の 3 つのアプリの一覧を出す。`istesso launcher open ID [--trial DIR] [--dry-run]` が、ID から、決まった形の引数を組んで開く。
  - ID は、その場で作り直した一覧の中から探す。**頼みの文字を、そのままコマンドにしない**（近道の `handler.m` と同じ考え）。`--dry-run` は、組んだ引数を出すだけ。
  - 子に渡す環境から `DAILY_OWNER` と `DAILY_OPEN_HANDLER` を除く。標準入出力は閉じる（`handler.m` と同じ）。結果は、終了コードだけ。
- **Swift（`macos/launcher/`、SwiftPM）**：
  - `LauncherCore`：一覧の読み取り、検索、選択、何をするかの決め。窓に触らない。
  - `LauncherUI`：AppKit の画面（検索欄、表、下の 1 行）。
  - `istesso-launcher`：窓を出す本体。**今日は、作るだけで、起動しない。**
  - `launcher-check`：`LauncherCore` の確かめ（束を作らない、ふつうの実行ファイル）。
  - `launcher-render`：作った一覧の見本で、画面を窓なしで PNG に描く（ふつうの実行ファイル。窓も、束も、作らない）。
- 権利、ペアリング、入力、遅延の道には、触らない。

## 5. 確かめ方

- 今日（Air だけ。画面には何も出さない）：`cargo test -p istesso-cli`、`launcher-check`、`launcher-render` の PNG を目で見る。描いたあとで、システムの記録に、新しいアプリの登録が出ていないことを見る。
- 夜（持ち主）：ターミナルから 1 行で、窓を出す。束にしないので、アプリの登録は増えない見込み（描く道具では、増えなかった。窓を出す本体では、まだ見ていない）。試すこと：検索、Ubuntu と Studio のアプリを 1 つずつ開く、Air のアプリを 1 つ開く、プロジェクトのコマンドを写す。

### 試作の使い方

```sh
cargo build --release -p istesso-cli
swift build -c release --package-path macos/launcher
macos/launcher/.build/release/istesso-launcher --istesso target/release/istesso --trial <試用の checkout>
```

- `--trial` には、試用の近道が使っているのと同じ checkout を渡す。
- 窓は、束にしていない実行ファイルから出る。Dock には、実行ファイルの名前で出るはず（推測。まだ見ていない）。
- Esc で検索を消す。もう一度 Esc で窓を閉じる（窓を閉じると、終わる）。↑↓ で選ぶ。Return で開く。

### 2026-11-20 に見たこと（Air。画面には何も出していない）

- `istesso launcher apps --json` を、試用の `daily.sh` の写し（自分のフォルダーに置き、状態のフォルダーも自分のもの）に向けて動かした。Ubuntu が 4、Studio が 1、この Mac が 103 で、試用の近道は 0 だった。`--dry-run` で、`daily.sh open firefox --host ubuntu`、`daily.sh open TextEdit --host mac`、`/usr/bin/open -a <束>` が組まれた。
- `launcher-render` の絵（見本の一覧。名前は作ったもの）：[明るい表示](img/launcher-window/light.png)、[暗い表示](img/launcher-window/dark.png)。見出し、行、右端の印、下の 1 行が出た。最初の絵では、表が空だった（窓の外では、表に大きさがない）。描く前に、表の大きさを決めるようにした。次の絵では、印のある行だけ、説明の文字が少し切れていた。印の幅を決め、説明がその手前まで広がる形にして、直った。選んでいる行の色は、窓の外の絵には出ない（窓で確かめる）。絵の埋め込みの情報（色の設定、撮った機械など）は、落とした。
- `launcher-render` を動かした前後のシステムの記録：プロセスごとに `registering self` のあと `Failed to register: -10811`（登録されない）。入力の監視の確かめ（`preflight`。問いは出ない）が 1 回。ネットワークの決まりの作り直し（`apps installed`）と、接続の切断は、なかった。

## 6. 持ち主に決めてほしいこと

1. 窓を、今夜ターミナルから試してよいか。
2. 同じアプリを 1 つにまとめる（M3 の提案の決まり）のを、次の段で入れるか。
3. プロジェクトを選んだときに写すコマンドを、`ish 名前` にしてよいか（名前で開く `ish` は、`proto-ish-projects` ブランチにある。いまの `~/.local/bin/ish` は、フォルダーの形だけ）。
