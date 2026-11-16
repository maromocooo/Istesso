# M2 を、2 台で動かして確かめる手順

`m2-audio` ブランチを `main` に入れる前に、自分の手で動かして確かめるための手順です。Ubuntu のアプリが仮想の出力先に流した音が、Mac まで届くことを見ます。10 分ほどで終わります。

- **音は出ません。** Mac の機器には無音を渡し、渡すはずだった音を調べます。ホストでは、テストの音（−60 dBFS の正弦波）を、仮想の出力先にだけ流します。
- **人のペアリングと鍵には、触りません。** この確認のための鍵を、作業用の場所に作ります。
- **ホストの音の設定は、前後で同じになります。** 手順の中で、それを確かめます。
- 計測の合図のファイルが busy のあいだは、始めません。1 回は 2 分以内です。

設計は [m2-plan.md](m2-plan.md)、これまでの結果は `docs/measurements/` の `m2-step4` の記録にあります。

## 1. ビルドする

```sh
# Mac
cargo build -p istesso-cli

# ホスト（libpipewire-0.3-dev と libclang-dev が要る）
cargo build -p istesso-cli --features pipewire
```

下の `istesso` は、こうしてできた `target/debug/istesso` のことです。

## 2. テスト用の鍵を作って、ペアリングする（最初の 1 回だけ）

両方の PC で、作業用の場所を決めて、環境変数に入れます。以下のコマンドは、どれも、この環境変数を付けた端末で実行します。

```sh
export ISTESSO_CONFIG_DIR=<作業用の場所>
```

```sh
# ホスト：コードが表示される
istesso pair host --port 47830 --name m2-test-source

# Mac：表示されたコードを入れる
istesso pair join <ホストの有線のアドレス>:47830 <コード> --name m2-test-exit

# ホスト：Mac に、音の権限を与える
istesso peers allow m2-test-exit audio
```

## 3. ホスト：前の状態を控える

```sh
wpctl status > before.txt
cp ~/.local/state/wireplumber/restore-stream restore-stream.before
pw-top -b -n 2 | awk '$1 == "R"'     # 何も出なければ、ホストでは何も鳴っていない
```

## 4. ホスト：音源を動かす

```sh
istesso audio serve --source pipewire --bind <ホストの有線のアドレス> --port 47822 \
    --seconds 100 --ledger sent.txt \
    --node-name istesso-audio-test --node-description "Istesso audio test" \
    --node-property device.routes=1
```

`device.routes=1` は、セッションマネージャー（WirePlumber 0.4）に、この出力先の音量を覚えさせないための、テスト用の指定です。

## 5. ホスト：テストの音を流す（別の端末で）

```sh
# 997 Hz、−60 dBFS、100 秒の正弦波を作る（1 回だけ）
python3 - <<'EOF'
import array, math, wave
rate, hz, peak = 48000, 997, 0.001 * (2**31 - 1)
second = array.array('i', [0]) * (rate * 2)
for n in range(rate):
    second[2 * n] = second[2 * n + 1] = int(round(peak * math.sin(2 * math.pi * hz * n / rate)))
with wave.open('tone.wav', 'wb') as out:
    out.setnchannels(2); out.setsampwidth(4); out.setframerate(rate)
    for _ in range(100):
        out.writeframes(second.tobytes())
EOF

# どこにも自動ではつながない形で再生を始める（つなぐまで、音は流れない）
pw-cat --playback --target 0 --latency 256 \
    -P '{ media.class = "Audio/Source" device.routes = 1 priority.session = 0 node.name = "istesso-audio-test-tone" node.description = "Istesso audio test tone" node.virtual = true }' \
    tone.wav &

# 仮想の出力先に、手でつなぐ
pw-link istesso-audio-test-tone:capture_FL istesso-audio-test:playback_FL
pw-link istesso-audio-test-tone:capture_FR istesso-audio-test:playback_FR

# つながった先が、仮想の出力先だけであることを確かめる
pw-link -l | grep -A1 '^istesso-audio-test-tone:'
```

音源の側に `something plays into the sink` と出れば、音が流れています。

## 6. Mac：再生する

### いちばん手軽な形（端末から）

```sh
istesso audio play <ホストの有線のアドレス>:47822 --peer m2-test-source \
    --latency-ms 100 --seconds 85 \
    --ledger received.txt --report report.json
```

エージェントのシェルから起動すると、受信のスレッドが低い優先度で動きます。遅延と停止を測るときは、下の形を使います。

### テスト用の `.app` として（遅延と停止を測るとき）

```sh
APP=<作業用の場所>/IstessoAudioTest.app
mkdir -p "$APP/Contents/MacOS"
cp target/debug/istesso "$APP/Contents/MacOS/istesso-audio-test"
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>istesso-audio-test</string>
    <key>CFBundleIdentifier</key><string>dev.istesso.audio-test</string>
    <key>CFBundleName</key><string>Istesso Audio Test</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.0.1</string>
    <key>LSUIElement</key><true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Istesso plays the sound of a paired computer on this network.</string>
</dict>
</plist>
EOF
# 署名の名前を決めておく。macOS は、ローカルネットワークの許可を、この名前で覚える。
codesign --force --sign - --identifier dev.istesso.audio-test "$APP"

open -n -g -W "$APP" --env ISTESSO_CONFIG_DIR="$ISTESSO_CONFIG_DIR" \
    --stdout play.out --stderr play.log --args \
    audio play <ホストの有線のアドレス>:47822 --peer m2-test-source \
    --latency-ms 100 --seconds 85 \
    --ledger "$PWD/received.txt" --report "$PWD/report.json" --connect-for 180
```

最初の起動では、macOS が「ローカルネットワーク上のデバイスの検索および接続」の許可を求めます。「許可」を押すまで、プログラムは 1 秒おきに試し続けます（180 秒まで）。

## 7. 結果を見る

```sh
# ホストの sent.txt を Mac に持ってきて、突き合わせる
istesso audio compare sent.txt received.txt
```

| 見るところ | 期待する値 |
|---|---|
| `compare` の結果 | `different`、`unknown`、`missing` が 0 |
| `report.json` の `playout` | `starts` が 1、`underruns` と `skips` が 0（待ち時間 100 ms のとき） |
| 同 `arrivals` | `lost_packets`、`breaks`、`refused_frames` が 0 |
| 同 `capture_to_output_ms` | 中央値が、待ち時間に 1〜2 ms を足した値（100 ms なら約 101 ms） |
| 同 `would_have_played.tone` | 大きさが −60.0 dBFS、`dropouts` と `clicks` が 0 |
| ホストの `serve` の出力 | `dropped_frames`、`trimmed_frames`、`breaks` が 0 |

待ち時間を 40 ms にすると、遅延は約 41 ms になります。停止があれば途切れます（[記録](measurements/2026-11-16-m2-step4-real-capture.md)）。

## 8. ホスト：後の状態を確かめる

`serve` と `pw-cat` が終わってから、確かめます。

```sh
wpctl status | diff before.txt - && echo "audio state unchanged"
cmp restore-stream.before ~/.local/state/wireplumber/restore-stream && echo "state file unchanged"
pw-dump | grep -c istesso-audio-test      # 0 なら、ノードは残っていない
```

`wpctl status` の左端の番号が変わることがあります。出力先と入力の並びと、`*` の付いた既定の行が同じなら、変わっていません。

## うまくいかないとき

| 出るもの | 原因と、すること |
|---|---|
| `this build cannot capture from PipeWire` | ホストで `--features pipewire` を付けずにビルドしている |
| `No route to host`（Mac、`.app` で起動したとき） | ローカルネットワークの許可がまだ。確認の画面で「許可」を押す |
| `this device may not have that host's sound` | 手順 2 の `istesso peers allow` を、ホストの作業用の場所で実行していない |
| 音源の側が `nothing plays into the sink` のまま | 手順 5 の `pw-link` が通っていない。`pw-link -o`、`pw-link -i` で、ポートの名前を確かめる |
