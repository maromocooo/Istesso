# 新しい窓の試作（Ubuntu ホスト）：U1 の手順

- 状態：**試作**。ADR 0027【提案】（ブランチ `design-new-windows`）の U1 を、Ubuntu のホストで。**`main` と試用版には、何も入れない**
- 日付：2026-11-20
- 関係：[docs/proto-new-windows.md](proto-new-windows.md)（Mac のホストと Air のビューアーの試作。言葉は、そこの 2 章と同じ）、[1 日の見極め（U0）](measurements/2026-11-20-new-windows-ubuntu.md)、[手順 1 の記録](measurements/2026-11-20-new-windows-ubuntu-on-commit.md)
- 持ち主の決め（11-20）：U0 の見極めが通ったので、作る。手順は、この順：1. 出力ごとに、コミットで描く　2. 窓の一覧（種類 6）を Ubuntu のホストから　3. 2 枚目の窓への入力とポップアップ　4. 本物のアプリ（GTK4 か Firefox）を 2 つ目の出力に

印：**（見た）** は、動かして、ログかコマンドの出力で見たこと。**（考えた）** は、コードや文書から考えただけのこと。

## 部品と、当てる順

コンポジター（gst-wayland-display、MIT）への試作の修正は、`scripts/patches/proto/` にある。**判定のビルドと試用のビルドには入らない**（`scripts/build-compositor.sh` は、どれも知らない）。計測の 4 つの修正（`scripts/patches/` の render-on-commit、cursor-messages、windows、dialogs）のあとに、この順で当てる：

| 順 | ファイル | 中身 |
|---|---|---|
| U0 | `gst-wayland-display-proto-second-output.patch` | 2 つ目の出力、2 つ目の要素（`output=1`）。親のない 2 枚目の窓が、そこに出る |
| 1 | `gst-wayland-display-proto-second-output-on-commit.patch` | 2 つ目の出力を、自分の窓のコミットで描く（自分の歩調）。描く合図は、自分の出力からだけ |
| 2 | `gst-wayland-display-proto-second-output-window-list.patch` | 2 つ目の出力の窓（題名、大きさ）を、`output=1` の要素が、バスの知らせ `wayland.second-window` で言う |

ホストの側のプログラム：`scripts/proto-windows-ubuntu.py`（`scripts/m1-step3-host.py` を変えずに包む）。`--second BIND --second-log FILE` で 2 本目の鎖と 2 つ目の `istesso-media` の口を足す。`--second-pacing commit|beat`（手順 1）。

## 手順 2：窓の一覧（種類 6）

- コンポジター：`output=1` の要素の `window-messages` を入にすると、2 つ目の出力の窓のことを、`wayland.second-window`（`present`、あれば `title`、`width`、`height`、`key`）で言う。言うのは、要素がつながったとき、窓がその出力に出たとき、題名か出力の大きさが変わったとき（その窓のコミットのたびに見る）、窓がなくなったとき。同じことは 2 度続けて言わない。**題名は、ログに書かない。** `output=0` の要素は、いまのまま（窓の数 `wayland.windows`。2 枚目の窓も数える）。
- `scripts/proto-windows-ubuntu.py`：その知らせを、`istesso-media` の種類 6 の一覧にして、**1 枚目の窓の接続**で送る（`istesso_media_host_set_windows`）。窓の番号は 2、`port` は 2 つ目の口のポート、`width`・`height` は 2 つ目の出力の大きさ（画素）、`modal` と `attention` は偽、`points` は付けない（このホストには、点の大きさがない。ビューアーは、自分の決まりで決める）。題名は 256 バイトまでで、文字の切れ目で切る（`scripts/proto_windows_list.py`、テストは `scripts/tests/proto-windows-list-test.py`）。通し番号は 1 から。送った記録（番号、窓の数、結果。題名なし）を 2 本目のフレームの記録に書く。
- 1 枚目の窓の口（`m1-step3-host.py` が作る）を、そのスクリプトを変えずに見つけるため、`ctypes.CDLL` を包み、メディアのライブラリーの `istesso_media_host_listen` と `istesso_media_host_listen_closed` が返した最初の口を覚える。2 つ目の口は、包まない `ctypes.CDLL` で作る。
- `--second` のときだけ送る。**手順 2 では、ビューアーが「窓ごとに見せられる」（`windows`）と言ったかどうかを見ずに送る**（試作の近道。手順 3 で、入力の道に `windows` が来てから送る形にする）。
- ビューアーの道具：`picture stay ADDR --windows` が、一覧が来るたびに 1 行（番号と、窓ごとの番号、ポート、大きさ、`key`、`modal`、`attention`、あれば `points`）を書く。**題名は書かない**（長さも）。

確かめたこと（見た）：

- コンポジターのテスト：71 本、全部通った（手順 1 までの 68 本と、新しい 3 本：つながる前・あとの知らせ、窓が出る・題名が変わる・閉じる、変わらないコミットでは言わない、出力の大きさが変わる、1 つ目の出力の窓の数は変わらない）。3 つの修正が、計測の 4 つの修正の木に、この順で当たり、テストした木と同じになる。
- Air の側：`cargo fmt --check`、`cargo clippy -p istesso-media --all-targets -- -D warnings`、`picture` の例のテスト、`proto-windows-list-test.py` が通った。
- 実物（13:16、ホストで 2 つの窓の回を 1 回。Air で `picture stay … --windows` を 1 本目のポートに）：1 本目の接続で、番号 1（窓 0 枚）、2 秒ほどあとに番号 2（`id=2 port=26931 1720x720 key=false`）が届いた。同じ Air の、2 本目のポートの `picture stay` は、その回の 2 本目の流れを受けた（899 枚）。ホストの記録では、2 つとも送った結果が 0（成功）。

まだのこと：

- 窓が閉じたときの空の一覧は、テストでは見たが、実物の回の終わりでは届かなかった（回の終わりは、テストアプリを止めてすぐパイプラインを止めるので、知らせが処理される前に終わる、と考えた）。
- 入力（ポインター、キー、`focus`、`close`）とポップアップ（手順 3）。本物のアプリ（手順 4）。
