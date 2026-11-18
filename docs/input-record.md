# 入力の記録：ホストに何が届いたかを、推測せずに見る

- 日付：2026-11-18（書いたのは夜中。**ホストでは、まだ一度も動かしていない**）
- 目的：修飾キー（Shift、Control、Option、Command）がアプリで効かない、という不具合の場所を、記録を見て決める
- 対象：手で試す経路（`scripts/try.sh` の `TRY=channel`）。ゲートのビルドと、計測の道具（`istesso-flash`、`istesso-measure`）には、入っていない。Rust のコードは、1 行も変えていない

## 何を書き留めて、何を書き留めないか

書き留めるもの：

- 修飾キーを押した、離した。どの修飾キーか（左右の Shift、Control、Alt、Super、Caps Lock、Num Lock）
- ポインターのボタン。どのボタンか、押したか離したか
- スクロール。縦か横か、量
- それぞれの行に、その瞬間に押されている修飾キー（記録する側が見てきた、押す・離すの結果）

書き留めないもの：

- **修飾キーでないキーが、どのキーだったか。** 行には `"key"` とだけ書く。打った文字は、記録から読み戻せない
- ポインターの位置。数だけ数える（各行の `moves` が、前の行からの数）

この決まりは、テストで確かめている（`scripts/test_input_record.py`。修飾キーでない 758 個の番号のどれを渡しても、記録が同じになること）。

注意：Mac の Command は、Control として送られる（`ISTESSO_COMMAND_KEY=super` のときは Super）。記録には `ctrl_l` と出る。Option は `alt_l`（右は `alt_r`）。

## 記録する場所は 3 つ

| 場所 | ファイル（ホストの `~/.cache/istesso-try/`） | 何が分かるか | 状態 |
|---|---|---|---|
| パイプライン（エージェントのあと、コンポジターの前） | `input-record.jsonl` | Mac から、中継とエージェントを通って、**届いたもの** | 動く。テストあり。ホストでは未実行 |
| アプリが告げられたこと | `input-record-application.jsonl` | コンポジターが、アプリに**何を告げたか**。キー、修飾キーの状態（`wl_keyboard.modifiers`）、キーボードが来た・去った | 動く。テストあり。ホストでは未実行 |
| コンポジターの中 | `input-record-compositor.jsonl` | `keyboard_input` が受け取ったものと、そのあとの修飾キーの状態（depressed、latched、locked） | **パッチだけ。ビルドしていない** |

パイプラインを選んだ理由：ここは、Mac の側（ビューアー、中継、エージェント）と、ホストの側（コンポジター、アプリ）の、ちょうど境目です。ここに届いていなければ Mac の側、届いていればホストの側、と半分に切れます。Python のスクリプトなので、ビルドが要りません。

2 つ目は、アプリ自身の Wayland ライブラリーが出す記録（`WAYLAND_DEBUG=1`）を使います。これは、すべてのキーの番号を出します。そこで、**ファイルに落ちる前に、フィルターを通します**（`scripts/input_record.py --wayland-debug`）。フィルターは、キーボードと、ポインターのボタン・スクロール・出入りだけを残し、修飾キーでないキーは `"key"` に変え、ほかは捨てます。フィルターを通す前のものは、ディスクに残りません。この回は、アプリのほかの出力（`app.log`）も捨てられます。

3 つ目は、上の 2 つで決まらないときのためのものです。コンポジターのパッチ（`scripts/patches/gst-wayland-display-input-record.patch`）を、**別のフォルダー**でビルドして、`ISTESSO_HOST_PLUGIN` で選びます。ゲートや計測が使うプラグインのフォルダーには、当てません。手順は、パッチの頭に書いてあります。

## スイッチ

`scripts/try.sh` を起動するときの、環境変数です。

| `ISTESSO_INPUT_RECORD` | すること |
|---|---|
| なし（既定） | 何も書き留めない。パイプラインは `input_record.py` を読み込みもしない |
| `1` | パイプラインの記録だけ |
| `2` | パイプラインの記録と、アプリが告げられたことの記録 |

パイプラインを直接起動するときは、`m1-step3-host.py --input-record FILE` です。

## 行の形

1 行が、1 つの JSON です。`t_ns` は、1970 年からのナノ秒です。

```json
{"kind":"header","record":"pipeline","version":1,"t_ns":…,"mono_ns":…,"note":"keys other than modifiers are counted, never identified: …"}
{"t_ns":…,"kind":"modifier","name":"shift_l","state":"down","held":["shift_l"],"moves":0}
{"t_ns":…,"kind":"key","name":"key","state":"down","held":["shift_l"],"moves":0}
{"t_ns":…,"kind":"button","name":"left","state":"down","held":["alt_l"],"moves":4}
{"t_ns":…,"kind":"scroll","name":"vertical","dx":0,"dy":-15,"held":["shift_l"],"moves":0}
{"t_ns":…,"kind":"summary","modifiers":2,"keys":1,"buttons":1,"scrolls":1,"moves":4,"other":0,"held":[]}
```

- `held`：その行の変化のあとに、押されている修飾キー
- `odd`：押されているはずのものが、もう一度押された（`already down`）、押されていないものが離された（`not down`）
- 最初の行（`header`）が、「修飾キーでないキーは数えるだけ」と言う。記録は、同じファイルに足されていく。回ごとに `header` が入る

アプリが告げられたことの記録（`t_ms` は、アプリの側の時計）：

```json
{"t_ms":…,"kind":"modifier","name":"shift_l","state":"down"}
{"t_ms":…,"kind":"modifiers","name":"state","depressed":1,"latched":0,"locked":0,"held":["shift"]}
{"t_ms":…,"kind":"key","name":"key","state":"down"}
{"t_ms":…,"kind":"focus","name":"keyboard","state":"enter"}
```

## 手で試す手順

始める前に、計測が走っていないことを確かめます（`~/link-proj/scripts/measuring.sh status` が `free`）。前の試用が動いていたら、止めます。

### 1. ホストに写す

試用の写し（`~/.cache/istesso-media-wip/src`）には、触れません。自分のフォルダーを作り、ビルド済みのものは、リンクで借ります（ビルドはしません）。

```sh
cd ~/link-proj-inputlog
ssh istesso-ubuntu 'bash -s' <<'EOF'
mkdir -p ~/.cache/istesso-inputlog-wip/src/scripts
ln -sfn ~/.cache/istesso-media-wip/src/target ~/.cache/istesso-inputlog-wip/src/target
EOF
scp scripts/m1-step3-host.py scripts/m1_encoder.py scripts/input_record.py \
    istesso-ubuntu:.cache/istesso-inputlog-wip/src/scripts/
```

### 2. 始める

Mac の側のプログラム（ビューアーと中継）は、昨夜と同じものを使います（`~/link-proj-media` でビルドしてあるもの）。この作業ツリーでは、何もビルドしません。

```sh
cd ~/link-proj-inputlog
ISTESSO_BUILT=~/link-proj-media \
ISTESSO_HOST_SRC='~/.cache/istesso-inputlog-wip/src' \
ISTESSO_INPUT_RECORD=2 TRY=channel \
scripts/try.sh env 'XDG_CONFIG_HOME=$HOME/.cache/istesso-try-blender/config' '$HOME/.local/opt/blender/blender'
```

`host: input that arrives is written down in …` と出れば、記録が入っています。

### 3. 押す

窓を 1 回クリックしてから、次を順に。**1 つごとに、3 秒ほど空けます**（記録の中で、切れ目が分かるように）。テスト用のキーだけを押します。文章や、パスワードは打ちません。

1. Shift だけを押して、1 秒おいて、離す
2. A を押して、離す
3. Shift を押したまま A。A を離して、Shift を離す
4. Option を押したまま、左ボタンでドラッグ。ボタンを離して、Option を離す
5. Shift を押したまま、スクロール。Shift を離す

### 4. 止めて、読む

```sh
scripts/try.sh stop
ssh istesso-ubuntu 'cat ~/.cache/istesso-try/input-record.jsonl'
ssh istesso-ubuntu 'cat ~/.cache/istesso-try/input-record-application.jsonl'
```

### 5. 読み方

| 見えること | 意味 |
|---|---|
| 1 で、パイプラインの記録に `modifier` の行がない | 修飾キーが、ホストに届いていない。Mac の側（ビューアーの `flagsChanged`、中継、エージェント）を調べる |
| 3 で、`shift_l down`、`key down`、`shift_l up` が、ほぼ同じ時刻に並ぶ | 修飾キーは、キーに「貸し出す」形でしか送られていない（`macos/gst-viewer/keys.h`）。押し続けた状態は送られていないので、4 と 5 には付かない |
| 4 と 5 で、`button` や `scroll` の行の `held` が空 | 上と同じ。または、修飾キーが先に離されている |
| パイプラインの記録は正しい（`held` に名前がある）。アプリの記録に、`shift_l` のキーはあるが、`modifiers` の行がない、または `depressed` が 0 | コンポジターが、状態を告げていない。コンポジターの記録（3 つ目）で確かめる |
| パイプラインの記録も、アプリの記録も正しい（`modifiers` の `depressed` が 0 でない） | 届いて、告げられている。アプリの側（Blender の扱い）を調べる。別のアプリ（テキストエディター）で、大文字が打てるかを比べる |
| アプリの記録に、修飾キーとキーの間で `focus` の `leave` と `enter` がある | キーボードが、いったん窓を離れている。クリックのたびに起きていないかを見る |

### 6. 片づける

記録は、読んだら消します。打った文字は入っていませんが、いつ何を押したかは入っています。

```sh
ssh istesso-ubuntu 'rm ~/.cache/istesso-try/input-record.jsonl ~/.cache/istesso-try/input-record-application.jsonl'
```

元の試用に戻すときは、`~/link-proj-media` で、いつもどおり `scripts/try.sh` を起動します。ホストの `~/.cache/istesso-inputlog-wip` は、要らなくなったら、フォルダーごと消せます。

## 2026-11-18 の朝に足したこと

- **何も止めない起動**：`ISTESSO_TRY_NO_STOP=1`。これがないと、`scripts/try.sh` の起動は、まず `stop` をします。`stop` は、ビューアー、入力の中継、ホストのパイプラインを、**名前で**止めます（誰が起動したものでも）。これを付けると、何も止めません。前の回が残っていれば（起動のときに書き留めたプログラム、ホストの 2 つの `tmux` のセッション）、そう言って、何もせずに終わります
- **2 つの記録を並べる要約**：`python3 scripts/input_record.py --summary パイプラインの記録 アプリの記録`。1.5 秒より長い間で区切って、区切りごとに、パイプラインに届いたものと、アプリが告げられたもの（`depressed` の値つき）を、1 行ずつ出します。最初の区切りは、窓をクリックしたときのものです
- **テキストエディターで試す形**：ホストの `~/.cache/istesso-inputlog-wip/trial/editor.sh`（リポジトリーには入っていない）が、このフォルダーの空のファイルを、このフォルダーの中の設定と状態で開きます。持ち主のエディターの状態（`~/.local/share/org.gnome.TextEditor/`）は、読みも書きもしません。起動の前に、その名前・大きさ・時刻だけを `owner-state-before.txt` に書きます

  ```sh
  cd ~/link-proj-inputlog
  ISTESSO_TRY_NO_STOP=1 ISTESSO_BUILT=~/link-proj-media \
  ISTESSO_HOST_SRC='~/.cache/istesso-inputlog-wip/src' \
  ISTESSO_INPUT_RECORD=2 TRY=channel \
  scripts/try.sh '$HOME/.cache/istesso-inputlog-wip/trial/editor.sh'
  ```

- ホストのエージェント（`tmux` の `istesso-agent`）は、あれば、そのまま使います。起動し直しません。記録が試すのは、**そのとき動いているエージェント**です

## もう 1 つの見方（スイッチなしで）

パイプラインは、キーを押すたびに `key_in` の行（時刻だけ。キーの番号はない）を、フレームのログに書きます。ただし、`scripts/try.sh` は、ログを捨てる設定（`--log -`）でパイプラインを起動するので、試用では出ません。

`WAYLAND_DEBUG=1` を、フィルターなしで使うこともできます（`scripts/try.sh env WAYLAND_DEBUG=1 コマンド`。出力は、ホストの `~/.cache/istesso-try/app.log`）。この出力には、**すべてのキーの番号が入ります。** 使うなら、テスト用のキーだけを押す短い回にして、読んだらすぐ `app.log` を消します。ふつうは、上の `ISTESSO_INPUT_RECORD=2` を使ってください。

## 確かめたこと、確かめていないこと

- 確かめた：`python3 scripts/test_input_record.py`（22 個。分類、行の形、「どのキーかを書かない」決まり、フィルター、2 つの記録を並べる要約）。Mac の、OS に付いている python3（3.9）と、ホストの python3（3.12）で通る。新しいパッケージは要らない
- 確かめた：コンポジターのパッチが、4 つの修正を当てた木に当たること。パッチの中の `input_record.rs` は、標準ライブラリーだけを使うので、単独でコンパイルして、3 個のテストを通した
- **確かめていない**：ホストでの実行すべて。パイプラインへの組み込み（`--input-record`）、`scripts/try.sh` のスイッチ、`WAYLAND_DEBUG` の出力の実際の形（ライブラリーの版で、少し違う。2 つの形に合わせてある）、コンポジターのパッチのビルド
- **測っていない**：遅延。スイッチを入れると、入力 1 つごとに、1 行を書く仕事が増える。スイッチを入れた回は、遅延の計測に使わない。スイッチを入れないときに増えるのは、入力 1 つごとの、1 回の比較だけ
- この夜は、ホストで音の試験が走っていて、何も起動できなかった。Mac でも、窓を出すものは動かしていない

## 2026-11-18：Blender での自己試験（ホストだけ）

自己試験（パイプラインの入力の口に、テスト用のキーだけを書き込む）を、アプリを Blender 5.3（`--debug-events --debug-handlers`）にして回した。ホストの `~/.cache/istesso-inputlog-wip/selftest-blender/`。アプリの記録には、このために、`wl_keyboard`・`wl_pointer` の `enter`・`leave` の相手の面（`wl_surface` の番号）、`xdg_toplevel` の `set_parent`・`set_title`・`close`、キーボードの設定（keymap の大きさ、repeat_info）を足した（フィルターの写しで。リポジトリーのフィルターは変えていない）。

見たこと：

- **窓が 1 つのとき**：Shift+A、Ctrl+A、Alt+A、Shift・Ctrl＋スクロール、Alt＋左ボタンのどれも、Blender の `wmEvent` に修飾キーが付いていた（`modifier={SHIFT}`、`utf8:'A'` など）。300 ms の間でも、人の速さ（40／80 ms）でも同じ。テキストエディターの記録と、行ごとに同じ形だった。座席（`wl_seat`）もキーボードも 1 つ
- **昨夜の試用の設定には `userpref.blend` がない**。「Emulate 3 Button Mouse」は効いていなかった。Option＋左ドラッグで何も起きないのは、これで説明がつく。Blender 自身の `--python-expr` で設定を入れると、Alt＋左は `MIDDLEMOUSE` になった
- **Preferences を別の窓で開いたとき**（Blender 自身に開かせた。Edit メニューと同じ操作）：
  - 窓が出た瞬間に、キーボードは Preferences の面へ移った（main を `leave`、Preferences に `enter`）。**ポインターは main の面に `enter` した**。2 つの窓は、どちらも出力の大きさで、Preferences が上に重なっている
  - Preferences の toplevel に `set_parent` はなかった
  - ポインターを動かさずにクリックすると、ボタンは **main** に届き（Blender は 3D View のキーマップで扱った）、キーボードは Preferences のままだった（`leave`・`enter` なし）
  - その状態の Shift+A は、**Preferences の窓**に届いた（`{SHIFT}` つき。Preferences のキーマップで扱った）。Shift＋スクロールと Alt＋左は、main に届いて、`{SHIFT}`・`{ALT}` が付いていた
  - ポインターを動かすと、ポインターは Preferences に移り（main を `leave`、Preferences に `enter`）、そのあとはすべて Preferences に、修飾キーつきで届いた
  - main の窓を Preferences より上に上げることは、この試験ではできなかった（上に重なった窓しか、クリックで選べない）

結論：

- 修飾キーは、Mac からアプリまで、どこでも落ちていない。Blender も、窓が 2 つでも、修飾キーの状態を正しく持っている
- 落ちているのは **キーの行き先**。コンポジターは、新しい窓が出てもポインターの行き先を見直さない（動かすまで、下の窓のまま）。（ここから、パッチのソースを読んだこと）クリックのときは、キーボードを「ポインターの位置のいちばん上の窓」に渡し、ボタンは「ポインターがいま入っている窓」に渡す。この 2 つが食い違うと、キーボードとポインターが別の窓に分かれる。キーは見えていない方の窓に行き、スクロールとクリックは見えている方に行く
- 直すのは、コンポジターのパッチ（ゲートのビルドの外）：窓が出た・上がった・閉じたあとに、ポインターの行き先を、いまの位置で見直す（動きを 1 回送り直す）。クリックでは、ボタンが届く窓にキーボードを渡す
- 昨夜の「main が Preferences の上に上がった」ところは、まだ作れていない。手の試用で、そのときの `enter`・`leave` の面を記録で見るのが次の手順

ほかに分かったこと：

- スクロールは、パイプラインの `dy` が、そのまま 1/120 ノッチの数としてアプリに渡る（−15 は 1/8 ノッチ）。小さなスクロールでは、Blender の wheel のイベントにならない
- Chrome（Wayland で、自分のプロファイル）も、修飾キーを正しく受け取った。**`TMPDIR` を `~/.cache` の下のフォルダーにすると、Chrome は起動の直後に落ちる**（SIGTRAP）。試用の起動では、Chrome に `TMPDIR` を設定しない
