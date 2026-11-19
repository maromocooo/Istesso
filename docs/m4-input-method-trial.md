# Mac ホスト：変換中の文字をアプリの中に出す入力ソース（試作、試用だけ）

2026-11-19。決めたことと理由は [ADR 0023](adr/0023-mac-inline-input-source.md)（切り替え、言語の宣言、順番、1 回だけの確定、つなぎ方）。対象：Mac Studio をホストにして、Air の変換中の文字（下線つき）を、ホストのアプリの中に、アプリの字で出す。候補の窓は Air の入力方式が、アプリのキャレットのすぐ下に出す（[ADR 0020](adr/0020-japanese-input.md) の道 d、[ADR 0007](adr/0007-control-channel.md) の `preedit`、[ADR 0015](adr/0015-media-transport.md) の種類 5）。

**試用だけ。** この入力ソースは、どのアプリにも文字を入れられるので、`main` に向かう前に Fable の見直しを通す。金曜までに確かめられなければ、Studio は帯のまま。

印：**見た**＝実機で自分で見た（どう見たかを書く）。**結論**＝Apple の文書やヘッダーから読んだだけで、実機では見ていない。

## 1. 調べた答え：入力ソースのほかに、公開された道はあるか

**ない（結論。Apple の文書と SDK のヘッダーだけで調べた）。** 別のプロセスから、アプリに「変換中の文字」（marked text）を渡せる公開の口は、入力ソース（InputMethodKit）だけ。

- InputMethodKit：入力ソースのプロセスが、`IMKTextInput` の `setMarkedText:selectionRange:replacementRange:`、`insertText:replacementRange:` で、前面のアプリの、いま有効な文字の欄に渡す。場所は `firstRectForCharacterRange:actualRange:`（大域の座標）と `attributesForCharacterIndex:lineHeightRectangle:` で読める。
- アクセシビリティ：`AXSelectedText`、`AXValue` を書けるのは、確定した文字だけ。変換中の文字を作る属性はない。WebKit の `AXTextInputMarkedTextMarkerRange` は、変換中の範囲を「読む」もの。
- `CGEventKeyboardSetUnicodeString`（いまの帯のあとの確定）：確定した文字だけ。
- `NSTextInputClient`：アプリの中（同じプロセス）の口。外からは呼べない。
- Text Services Manager の Carbon イベント（`kEventTextInputUpdateActiveInputArea` など）：アプリの中のもの。外から送る公開の口はない。

## 2. 試作の形

場所：`macos/input-method/`（`build.sh`、`main.swift`、`wire.swift`、`probe/main.swift`、`icon.swift`）。

- **Istesso Dev Input**（`com.maromoco.istesso-dev.inputmethod.trial`）：入力ソースの束。
  - Studio のキーボードは取らない：`recognizedEvents` は空、`handle` はいつも「受けない」。キーは、入力ソースがないときと同じく、アプリへ行く（結論）。
  - 文字は、ホストのプログラムからだけ来る。ホストが持つソケットにつなぐ（毎秒、つながるまで試す）。
  - `mark`（文字の全体と、選んでいる範囲。ADR 0007 と同じく UTF-8 のバイトで数える）→ 選んでいる文節は太い下線、残りは細い下線で `setMarkedText`。空なら消す。`commit` → `insertText`。`clear` → 消す。どれのあとも、キャレットの四角を返す。
  - アプリが自分で変換を終えたとき（ほかの所を押したなど）：入力方式のふつうのとおり、出している文字を確定し、ホストに `ended` を送る（Air の側も変換を終えるため）。
  - 文字（変換中も確定も）は、どこにも書かない。ログは文字数だけ（`os.Logger`、subsystem は束の識別子）。
- **istesso-ime-probe**（`com.maromoco.istesso-dev.ime-probe`）：ホストの側の試しのプログラム。あとで、Mac ホスト（`macos/daily-host`）が同じことをする。
  - `status`（読むだけ）、`register`、`serve [手順のファイル]`、`select`、`restore <ID>`、`secure`、`self-test`。
  - 試しの手順：`macos/input-method/trial-textedit.txt`（TextEdit の自分の書類だけで）。
- **試しのための読み戻し（`check`）は試作だけ。** `build.sh --trial-check` で作ったときだけ入る。TextEdit の中だけで、変換中の文字の範囲だけのハッシュを返し、試しのプログラムは「一致したか」だけを出す。**製品の入力ソースには入れない**（アプリの文字を読める部品になるため）。ハッシュそのものは、どこにも書かない（短い文字のハッシュは元に戻せる）。
- **つなぎ方**：ホストが、この利用者の一時フォルダー（`confstr(_CS_DARWIN_USER_TEMP_DIR)`、この利用者だけのもの）の中に、自分のフォルダー（0700）と、ソケット（0600。umask 0177 で作るので、作った瞬間から 0600）を作る。入力ソースは、つなぐ前に、フォルダーとソケットの持ち主と権限を確かめる（リンクは不可）。
- **相手の確かめ**：両方とも、つないだ相手の audit token（プロセス番号ではない）から、署名を確かめる。同じ利用者で、自分と同じ葉の証明書で署名され、識別子が決まった一覧にあること（入力ソースが受けるのは `ime-probe` と `daily-host`、ホストが受けるのは入力ソースだけ）。自分がアドホック署名なら、だれも受けない。
- **キャレットの四角**：入力ソースは、大域の点（AppKit の向き：主画面の左下が原点）で返す。`top` は上からの y（主画面の高さ −（y + 高さ））。ホストは、窓の左上（CGWindow の点）を引き、窓の点あたりの絵のピクセル数を掛けて、種類 5 の「絵のピクセル」にする。`scale` は、その場所の画面の倍率（参考）。

## 3. パスワードの欄（持ち主、2026-11-19）

- 決まり：前面の欄がパスワードの欄（`AXSecureTextField`）のとき、入力ソースで変換中の文字を出さない。Air は、自分の入力方式を通さずに、ふつうのキーを送る（そして Air の側で Secure Event Input を入れる）。
- 入力ソース：Secure Event Input が入っているあいだ（`IsSecureEventInputEnabled()`）は、`mark` を断り（`"why":"secure"`）、何も出さない。どの答えにも `"secure"` を付ける。
- ホスト：アクセシビリティで、前面の欄のサブロールを見る（`istesso-ime-probe secure` が同じことをする。ホストはアクセシビリティの許可を持っている）。パスワードの欄なら、種類 5 の状態 2（「あるがパスワード」）を Air に送る。Ubuntu の側が text-input-v3 の purpose で言うのと同じ。
- macOS は、パスワードの欄では入力方式を呼ばない（Secure Event Input のあいだは英字の入力になる）と読める（結論）。それでも、ホストと入力ソースの両方で断る。

## 4. 切り替えの案（持ち主が決める）

| | A：Studio ではいつも Istesso の入力ソースを選んでおく | B：遠くから打つあいだだけ切り替えて、戻す |
|---|---|---|
| 仕組み | 持ち主が選んだまま | ホストが `TISSelectInputSource` で切り替え、前のものに戻す |
| Studio の前で打つ | 英字はそのまま通る（どの配列になるかは未確認）。**日本語は打てない**（自分で別のソースに切り替える） | ふだんのとおり |
| ほかの入力ソース | 使うたびに手で切り替え | 遠くから打つあいだだけ置き換わる |
| ずれ | 起きない | 切り替えに掛かる時間（未計測）。ホストが落ちると、選ばれたまま残る（起動のときに戻す） |
| 同時に打つ | — | 持ち主が Studio の前で打っているときに切り替えると、手元の入力を邪魔する（作業の一覧 0n と同じ守り：手元の入力が最近あれば切り替えない） |
| Secure Input | どちらも同じ：入っているあいだは、入力ソースに文字が来ない。ホストは帯（または状態 2）に戻す | 同左 |
| 「書類ごとに入力ソースを切り替える」の設定 | 影響なし | 入っていると、切り替えが窓ごとになる（未確認） |

提案：**B**（手元の入力が最近あれば切り替えない守りつき）。毎日の Mac なので、Studio の前での日本語入力を残したい。ただし、切り替えの時間と、戻すときの取りこぼしを測ってから決める。

## 5. 持ち主の手順

### 入れる

1. （エージェント、許可をもらってから）束を Studio の `~/Library/Input Methods/` に置く。Apple のヘッダーの説明（`TISRegisterInputSource`）で、利用者ごとの入力方式の置き場所はここ。
2. （持ち主）System Settings > Keyboard > Text Input の「Input Sources」の「Edit…」 > 左下の「+」 > 「日本語」（なければ「その他」）> 「Istesso Dev Input」> 「Add」。**手で足す。スクリプトでは足さない。**
3. 一覧に出てこなければ、ログアウトしてログインし直す（入力ソースの一覧の作り直し）。
4. 試すときだけ、メニューバーの入力メニューで「Istesso Dev Input」を選ぶ（または、エージェントが `istesso-ime-probe select`。試しのあとは `restore` で戻す）。

### 外す

1. System Settings > Keyboard > Text Input の「Input Sources」の「Edit…」で「Istesso Dev Input」を選び、左下の「−」。
2. 束を消す：Finder で「フォルダへ移動」に `~/Library/Input Methods` と入れ、「Istesso Dev Input.app」をゴミ箱へ。（または、ターミナルで `rm -rf ~/Library/Input\ Methods/Istesso\ Dev\ Input.app`）
3. 入力ソースのプロセスが残っていることがあるので、ログアウトしてログインし直す。
4. 試しのソケットのフォルダー（この利用者の一時フォルダーの `istesso-dev-input`）は、ホストのプログラムが終わるときに、ソケットを消す。フォルダーは空で残る（害はない）。

## 6. 見たこと・まだ見ていないこと

- 見た（Air と Studio で、`istesso-ime-probe self-test` の出力）：相手の確かめ（両方向）、知らない相手を断る、0600 でないソケットにはつながない、UTF-8 から UTF-16 への範囲の直し、長すぎ・制御文字・空の確定を断る。アドホック署名にした写しは、両方とも断られた（Air）。Studio の macOS でも同じ自己試験が通った。
- まだ見ていない（Studio に入れてから）：TextEdit に下線つきの文字が出るか、キャレットの四角が読めるか（値と向き）、自己署名の入力ソースを System Settings で足せるか、Studio のキーを本当に素通しするか、どの配列で通るか、切り替えに掛かる時間。
