# Studio の次の入れ替え（studio-next）

- 状態：**試用版**。ブランチ `studio-next`（`m4-host-inline` の 56246d8 から）。`main` には何も入れない
- 日付：2026-11-20
- 関係：`docs/m4-daily-host.md`（試用版のホスト）、`docs/proto-new-windows.md`（新しい窓の試作）、ADR 0017（Mac のホストの許可）

この文書は、Studio のホストのプログラムを、次の版に入れ替える手順と、戻し方を書く。入れ替えるのは、持ち主がいるとき（今夜の試用のあと）。

印：**（見た）** は、Air の上で動かして、出力で見たこと。**（読んだ）** は、コードと資料から考えただけのこと。Studio の上では、まだ何も動かしていない。

## 1. 何が変わるか（読むだけ）

包みは 1 つ。Studio に行くのは、ホストのプログラム（`Istesso Dev Daily Host.app`）と、ホストのエージェント（`bin/istesso`）の 2 つだけ。

| 中身 | ブランチ | Studio で変わること |
|---|---|---|
| ホストの守りの直し | `host-hardening` 89ffdfd | `m4-daily-host.md` の「次の入れ替えで変わること」のとおり。最後の窓を閉じても止まらず、⌘N だけが通る。2 倍の画面へ移し直す（1 つの窓に 5 回まで）。入力と絵の細かい直し。入力の権利の見直し（7a）は「入れ替えてよい」。低い指摘が 9 つあり、今夜は直さない |
| 新しい窓（Mac のホストの分） | `proto-new-windows` 24c2bbd | **切り替え（`--windows`）を付けないかぎり、いまと同じ**（読んだ。`windows.swift` のどの道も `settings.windows` で閉じている）。エージェントは「窓の番号を通す」と答えるようになる。古いリレーとビューアーは、その答えを読み飛ばす |
| 「あと N 日」の印 | `proto-capture-notice` 7c64dfd | **Studio では何も変わらない。** 印は Air の窓の題名に出る（メニューバーではない）。Air の見張り（`scripts/daily-mac.sh`）の試作で、既定は切 |
| 組み合わせの直し | `studio-next` f59e86e | 新しい窓の 2 枚目からの窓でも、絵の外を指す場所を、その窓の絵の端に収める（守りの直しは、最初の窓だけを収めていた） |

ほかに、Air の側だけの小さな足し：`macos/daily-host/build.sh` が `CARGO_TARGET_DIR` の下のライブラリをつなぐ、`DAILY_CAPTURE_NOTICE_DAYS`（印を何日前から出すか。既定 7）、入れ替えの台本 `scripts/mac-host-swap.sh`。

**入力の道に手が入っている**（キー、ポインター、⌘N の通し方）。**遅延は測っていない**（Air の上の確かめだけ）。

## 2. 用意したもの（Air の上。Studio にはまだ写していない）

置き場所は、Air の外付けの SSD の、名前が `.noindex` で終わるフォルダー（以下 `P`。正確な場所は、とりまとめ役の手元の写しに書いた）。開いていない、登録していない、`~/Applications` には何も置いていない。

| もの | 中身 |
|---|---|
| `P/Istesso Dev Daily Host.app` | コミット af0bd09 から、`macos/daily-host/build.sh --sign` で作った。CDHash `a4cafd0d1a28fc3cc5ca8c4496f1743c15960694`。署名の証明書は `~/.config/istesso/sign.env` のもの（葉の SHA-1 `3b9456d4c30db0a11001f7c061f72cd2b0b3c46b`。Studio のいまのホストの決まり（designated requirement）にある葉と同じ。だから、画面収録とアクセシビリティの許可は、そのまま続く見込み（読んだ）） |
| `P/bin/istesso` | 同じコミットの `cargo build --release --locked -p istesso-cli`。SHA-256 `d8c40d99686430e701dfd5494a38b345cfd446b6528afe51ef0571bae56bfc06`（リンカーのアドホックの署名だけ。いまの `bin/istesso` と同じ作り方） |
| `P/air/gst-viewer` | Air のビューアー。**新しい窓を試すときだけ**使う（シェルから起動する。束は作らない）。Studio には写さない |
| `P/package.txt` | 名前（`studio-next`）、CDHash、SHA-256。入れ替えの台本が、写す前と写したあとに、これと比べる |

## 3. 始める前（とりまとめ役と持ち主、5 分）

以下は Air の Terminal。`W` はこのブランチの作業ツリー。

1. 計測の合図が、2 台とも `free`：`~/link-proj-m4-daily/scripts/measuring.sh status`（Air）。Studio の分は、台本が読む。
2. `ssh studio` のタブが開いている：`ssh -O check istesso-studio` が `Master running`。
3. 夕方の試用が終わっている。Studio のアプリの窓を Air で閉じてから：`~/link-proj-m4-daily/scripts/daily-mac.sh stop`（最後が `here: windows, relays and the guard are stopped`。Studio のアプリは開いたまま）。
4. いまの版を読む（読むだけ）：`"$W/scripts/mac-host-swap.sh" show`。`usual place: CDHash=…` が出る（記録では 542ad7c の `521e9fd8…`）。書き留める。
5. **署名した人が同じか**（読むだけ。台本は CDHash しか比べないので、手で）：Studio のいつもの場所の束と、包みの束の「決まり」（designated requirement）を 1 行ずつ出す。**2 行がまったく同じときだけ、4 章（入れ替え）へ進む。** 違えば、そこでやめる（違う証明書の束を入れると、画面収録とアクセシビリティの許可が外れ、Studio の画面に確認が出る）。

   ```sh
   ssh istesso-studio 'codesign -d -r- "$HOME/.cache/istesso-daily-mac/Istesso Dev Daily Host.app" 2>&1 | grep "^designated"'
   codesign -d -r- "$P/Istesso Dev Daily Host.app" 2>&1 | grep "^designated"
   ```

   包みの行は `designated => identifier "com.maromoco.istesso-dev.daily-host" and certificate leaf = H"3b9456d4c30db0a11001f7c061f72cd2b0b3c46b"`（見た）。
6. 今夜決めること 4 が「はい」のときだけ：5-3 の 1 の写しを、**ここで一度**取る（入れ替えの前の「確認を出した時刻」。A1 のあとにもう一度取って比べる）。

## 4. 入れ替え（1 分）

```sh
"$W/scripts/mac-host-swap.sh" in "$P"
```

- 最後が `usual place: CDHash=a4cafd0d1a28fc3cc5ca8c4496f1743c15960694`。その前の行が `kept: replaced/before-studio-next-Istesso Dev Daily Host.app (CDHash=…), replaced/before-studio-next-istesso`（前の版は消さずに、`~/.cache/istesso-daily-mac/replaced/` に残る）。
- 台本がすること：Air で、包みが `package.txt` のとおりか確かめる → Studio で、合図が `free`、ホストのプログラムとエージェントが動いていない、`before-studio-next` がまだない、を確かめる → いつもの場所の隣の `incoming-studio-next/` に写し、もう一度確かめる → いまの束を `replaced/` に動かし、包みの束を**いつもの場所**（`~/.cache/istesso-daily-mac/Istesso Dev Daily Host.app`）に置く → `bin/istesso` を写しで残し、包みのものに置き換える。何も開かない、起動しない、消さない。
- 断られたとき（何も変わっていない）：`a latency measurement is running` → 合図が `free` になってから。`a host program or an agent of the trial runs there` → 3 の `stop` から。`is kept there already` → 前の入れ替えが戻されていない（6 の戻し方を先に）。`not reached` → `ssh studio` を開き直す。

## 5. 入れ替えのあとの確かめ（持ち主、20 分ほど）

いつもの開き方（Spotlight、Alfred、`scripts/daily.sh open APP --host mac`）でよい。Air の側は何も変えていない。

### 5-1. 許可が残ったこと（3 分）

| # | すること | 見るもの | ちがうとき |
|---|---|---|---|
| A1 | `TextEdit (Studio)` を開く | 絵が来る。**Studio の画面に、画面収録の確認が出ない**。Air の窓の題名に「Studio で確認が出ています」が出ない | 確認が出たら、許可せず、文面を控えて、6 の戻し方 |
| A2 | 文字を打つ（英字、日本語を数行）、⌘C／⌘V、クリック | どれも届く（アクセシビリティが残った） | 届かなければ、`stop` して、とりまとめ役に |
| A3 | とりまとめ役：下の 1 行（読むだけ） | `"screen_recording":true,"accessibility":true` | false があれば、戻す |
| A4 | 変換中の文字をアプリの中に出す形（`DAILY_INLINE=1`）を使っているとき | 変換中の文字が、前と同じにアプリの中に出る（入力ソースとホストは、同じ証明書どうしで確かめ合う。証明書は変えていない） | 帯に戻ったら、知らせる（文字は落ちない） |

A3 の 1 行：

```sh
ssh istesso-studio 'grep -h "\"permissions\"" ~/.cache/istesso-daily-mac/run/*/host.log | tail -2'
```

### 5-2. 守りの直しの、手でしか見られない 3 つ（見直し 7a から、10 分）

| # | すること | 見るもの |
|---|---|---|
| H1 | ショートカットのアプリごとに（TextEdit、Safari、メモ、Finder、プレビュー、Dia など、いつも開くもの）：Air の窓で、そのアプリの最後の窓を閉じる（⌘W。書きかけのないもので）→ Air で ⌘N | 新しい窓が出て、それが Air の窓に映る。**⌘N の前に**打った文字、⌘V、クリックは、何も起こさない。アプリごとに、通ったかを 1 行ずつ書く。**プレビューは通らない見込み**（プレビューの ⌘N は「クリップボードから新規作成」で、窓が出ないことがある） |
| H2 | ① Air の窓を閉じて（赤いボタン）すぐ開き直し、絵が戻った直後に、Studio をロックする（⌃⌘Q）② Air の窓の端を引いて離した直後（絵の大きさが変わる途中）に、Studio をロックする | ロックのあいだは絵が来ない（Air の題名に「Studio はロック中」）。Studio でロックを外すと、絵が戻る。①②とも |
| H3 | Studio で、映しているアプリの窓を、Studio の自分の画面へ移す（持ち主の慣れた方法で。移すあいだは Studio のマウスを使うので、移さない）。そのあと Studio のキーボードとマウスから手を離す | 手を離して数秒のうちに、窓が 2 倍の画面へ戻る（Air の絵がくっきりに戻る）。Studio で打ったり動かしたりしているあいだは、戻さない。とりまとめ役：下の 1 行で、窓 1 つにつき 1 以下 |

H3 の 1 行（読むだけ。アプリごとの数）：

```sh
ssh istesso-studio 'grep -c "did not go onto the virtual display" ~/.cache/istesso-daily-mac/run/*/host.log'
```

### 5-3. 「あと N 日」の印（今夜決めること 4 が「はい」のときだけ、5 分）

印は Air の窓の題名に出る。Air のいつもの `~/link-proj-m4-daily` には入っていないので、このブランチの作業ツリーから開く。

**始める前に**：`~/link-proj-m4-daily/scripts/daily-mac.sh stop`。5-1、5-2 の TextEdit のホストのプログラムや、いつもの起動口の見張りが残っていると、それが前の設定のまま使われ、印は出ない。

1. とりまとめ役：Studio の画面収録の記録から、ホストの束の 1 行だけを写す（Studio で読むだけ。全体は Air に置かない。写しは Air の状態フォルダーに 1 つできる）：

   ```sh
   home=$(ssh istesso-studio 'printf %s "$HOME"')
   ssh istesso-studio 'plutil -convert xml1 -o - "$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"' |
       "$W/scripts/studio-capture-notice.py" --keep-entry "$home/.cache/istesso-daily-mac/Istesso Dev Daily Host.app" \
       > "$HOME/Library/Caches/istesso-daily-mac/capture-approvals.json"
   "$W/scripts/studio-capture-notice.py" "$HOME/Library/Caches/istesso-daily-mac/capture-approvals.json" --bundle "Istesso Dev Daily Host.app" --json
   ```

   `"state":"upcoming"` と日付が出る（11-19 の記録では 12-19 11:47 ごろ）。`unknown` なら、印は出ない（推測はしない）。**入れ替えの前に一度、A1 のあとにもう一度**写すと、`kScreenCaptureApprovalLastAlerted` が変わっていないこと（＝確認が出なかったこと）も分かる。
2. 持ち主：上の `stop` のあとで、

   ```sh
   DAILY_CAPTURE_NOTICE=1 DAILY_CAPTURE_NOTICE_DAYS=40 DAILY_ISTESSO_BIN="$P/bin/istesso" "$W/scripts/daily-mac.sh" open TextEdit
   ```

   Air の窓の題名の最後に「— Studio はあと N 日（M/D HH:MM ごろ）で確認を出します」。いつもの形（`DAILY_CAPTURE_NOTICE_DAYS` なし）では、残りが 7 日以下になるまで出ない。終わったら `"$W/scripts/daily-mac.sh" close TextEdit`。

### 5-4. 新しい窓（任意。今夜するかは持ち主が決める、10 分）

`docs/proto-new-windows.md` の 5 章の S1〜S7 を、次のコマンドで（このブランチの作業ツリーの見張り、包みのリレーとビューアー）。**先に** `~/link-proj-m4-daily/scripts/daily-mac.sh stop`（TextEdit のホストのプログラムが残っていると「もう動いている。設定は始めたときのまま」となり、切り替えが使われない）：

```sh
~/link-proj-m4-daily/scripts/daily-mac.sh stop
DAILY_WINDOWS=1 DAILY_FRAME_LOGS=1 DAILY_ISTESSO_BIN="$P/bin/istesso" DAILY_VIEWER="$P/air/gst-viewer" "$W/scripts/daily-mac.sh" open TextEdit
```

開いたあと、切り替えが使われたかを読む（読むだけ）。1 以上なら使われている。0 なら、S の確かめをせずに `stop`：

```sh
ssh istesso-studio 'grep -c "\"windows\",\"declared\":true" ~/.cache/istesso-daily-mac/run/TextEdit/host.log'
```

**S3 のまえに知っておくこと**：新しい窓を付けた形（いつもの押し方、形 4）では、Studio の上で 2 枚の窓が重なっているところをクリックやスクロールすると、Air で指した窓ではなく、Studio で上にある窓に届くことがある（たとえば 2 枚目の書類が 1 枚目に重なっているとき、1 枚目をクリックすると 2 枚目にカーソルが入る）。思わないことが起きたら、最初のクリックで、やめる（`stop`）。

5 章の「入れ替え」と「戻す」は、この文書の 4 と 6 で置き換える（同じ束を 2 回入れない）。

**H1（最後の窓を閉じて ⌘N）は、`DAILY_WINDOWS` なしで確かめる。** 新しい窓を付けたまま最後の窓を閉じて ⌘N を押したとき、新しい窓は「映す窓」になる、と見直しで読んだ（実機では見ていない）。

## 6. 戻し方（前の版 542ad7c へ、1 分）

```sh
~/link-proj-m4-daily/scripts/daily-mac.sh stop
"$W/scripts/mac-host-swap.sh" back studio-next
```

最後が `usual place: CDHash=…`（3 の 4 で書き留めた CDHash）。前の束が、同じ場所、同じ証明書に戻るので、画面収録の確認は出ない見込み（読んだ。いつもの場所での置き換え）。包みの束も消さずに `replaced/<時刻>-studio-next-…` に残る。

## 7. 気をつけること

- 入れ替えたままにするなら、**`~/link-proj-m4-daily` を `studio-next` へ進めるまで、そこで `scripts/daily-mac.sh setup` を打たない。** setup は、打ったチェックアウト（いまは 56246d8）から作り直して、いつもの場所の束と `bin/istesso` を、何も言わずに置き換える（守りの直しのない束になる。同じ証明書なので、確認は出ない見込み）。そのとき `replaced/` に残るのは包みの**束だけ**で、包みの `bin/istesso` は上書きされる（Air の SSD の `P/bin/istesso` には残る）。`~/link-proj-m4-daily` を進めるかどうかは、持ち主が決める（進めると、Air の起動口に新しい切り替え（既定は切）が入り、次の setup でリレーも新しくなる）。
- 新しい窓は、`DAILY_WINDOWS=1` を付けたときだけ。いつもの開き方では、いまと同じ。
- 束は、いつもの場所のほかでは起動しない（`replaced/` や `incoming-…` の中のものを `open` しない）。場所が変わると、Studio に画面収録の確認が出る。

## 8. Air の上で確かめたこと（2026-11-20 12:30〜13:00）

### 見た

- 計測の合図：ビルドと確かめの前に、毎回 `free`。
- Rust（`CARGO_TARGET_DIR` を SSD に）：`cargo fmt --all -- --check` 終了コード 0、`cargo clippy --workspace --all-targets -- -D warnings` 0、`cargo test --workspace` 0（621 通り、落ちたもの 0）。Ubuntu のホストの rustc では clippy を通していない。
- Swift：`build.sh` と同じ引数（`-warnings-as-errors`）で、ただの実行ファイルに組み立て、`--self-test` が `all passed`（ok 273 行。守りの直しだけのときは 233）。組み合わせの直し（f59e86e）の 3 行は、直しを外した写しで 2 行が落ちた（写しは消した）。署名した束も、`build.sh --sign` の中の `--self-test` が `all passed`。
- 署名：`codesign --verify --strict` が通る。葉の SHA-1 が、`sign.env` の身元の SHA-1 と同じ（名前は出さずに比べた）。
- ビューアー：`macos/gst-viewer/build.sh` と同じ確かめ（キー、文字、コピー、窓の一覧、窓の大きさ、スクロール）が全部通り、警告なしでつないだ。
- 入れ替えの台本：`scripts/tests/mac-host-swap-test.sh`（ssh の代わりに、手元の作り物の家で動かす。アドホックの署名の小さな束 2 つ）が全部通った。断る場合（中身が違う、ホストのプログラムが動いている、合図が busy、同じ名前がもう残っている、戻すものがない、変な名前とフォルダー）、`in`、`show`、`back`、前の失敗の残り。本物の包みでも、作り物の家に対して `in`、`show`、`back` が通った（写したあとも CDHash は同じ）。
- `scripts/studio_capture_notice_test.py` 17 件 OK。`DAILY_CAPTURE_NOTICE_DAYS` の変な値（abc、401、-1、12345）は、何も始めずに終了コード 2。

- 別の目の見直し（2026-11-20 12:53〜13:15、56246d8..02cda60）：「直し 7 と 11 のあとで準備よし」。7（署名した人を比べる）は 3 の 5、11（戻し方の番号）は直した。ほかの指摘（新しい窓の形 4 のクリック、台本の比べと動かす合間など）は、今夜の手順の注か、あとの直し。

### 見ていない

- Studio の上のこと全部（許可が残るか、⌘N、ロック、2 倍の画面、新しい窓、記録の写し）。5 章で確かめる。
- 遅延（実機の組で測っていない）。
- 守りの直しのうち、`--self-test` が届かないもの（`m4-daily-host.md` と見直し 7a のとおり）。
