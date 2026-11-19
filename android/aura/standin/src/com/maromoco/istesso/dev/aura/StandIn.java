// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): a stand-in for the device on a desktop JVM.
package com.maromoco.istesso.dev.aura;
import android.util.Log;
/** Stand-in for AURA's panel on a desktop JVM: the app's own Picture and Native, the real Rust library, no Android. */
public final class StandIn implements Picture.Told {
    static String t() { return Log.Stamp.now(); }
    public void connecting() { System.out.println(t() + " panel: つないでいます…"); }
    public void connected(Picture from, int w, int h, int fps, int chroma) { System.out.println(t() + " panel: connected " + w + "x" + h + " " + fps + " fps " + chroma); }
    public void cut(long cuts) { System.out.println(t() + " panel: つなぎ直しています…（切れたのは " + cuts + " 回目）"); }
    public void reconnecting(int why, long waitMs, int attempt) { System.out.println(t() + " panel: つなぎ直しています… why=" + why + " wait=" + waitMs + " ms attempt=" + attempt); }
    public void decoder(String name, boolean low) {}
    public void over(String why) { System.out.println(t() + " panel: over: " + why); }
    public static void main(String[] a) throws Exception {
        if (a.length >= 4 && a[3].equals("pair")) {
            // A test device joins the host showing a code. The code comes on standard input, from
            // the program that read it off the host, and is never printed. The host's id goes to
            // the file named third.
            String code = new java.io.BufferedReader(new java.io.InputStreamReader(System.in)).readLine();
            String[] joined = Native.pairJoin(a[0], a[1], code == null ? "" : code.trim(), "aura-standin-test");
            if (joined == null) {
                System.out.println(t() + " pairing failed: " + Native.lastError(Native.ERROR_PAIRING));
                System.exit(1);
            }
            java.nio.file.Files.writeString(java.nio.file.Path.of(a[2]), joined[0]);
            System.out.println(t() + " paired");
            System.exit(0);
        }
        if (a.length >= 4 && a[3].equals("text")) {
            text(a[0], a[1], a[2]);
            return;
        }
        Picture p = new Picture(a[0], a[1], a[2], null, new StandIn());
        p.start();
        int seconds = Integer.parseInt(a[3]);
        for (int i = 0; i < seconds; i++) {
            Thread.sleep(1000);
            String line = p.statsLine(1_000_000_000L);
            System.out.println(t() + " stats " + line.replaceAll(" oneway.*?queued=", " queued=").replaceAll(" in_decoder.*?cuts=", " cuts=") + " keys_queued=" + android.media.MediaCodec.queuedKeys);
        }
        p.stop();
        System.exit(0);
    }

    /**
     * The input's connection as the panel opens it, and committed text as TypingSurface hands it
     * over (made-up test text, never typed by a person): a key, text, a newline and a tab inside
     * text, and text longer than one piece.
     */
    static void text(String dir, String address, String peer) throws Exception {
        long input = Native.inputOpen(dir, peer, address);
        for (int i = 0; i < 50 && input != 0; i++) {
            long[] s = Native.inputState(input);
            if (s != null && s[1] != 0) {
                break;
            }
            Thread.sleep(100);
        }
        long[] s = Native.inputState(input);
        System.out.println(t() + " input open=" + (s == null ? "?" : s[1]) + " text=" + (s == null || s.length < 6 ? "?" : s[5]));
        Native.inputSend(input, new int[] {Native.INPUT_KEY, 30, 1, Native.INPUT_KEY, 30, 0}, 2);
        Native.inputText(input, "日本語テスト");
        Native.inputText(input, "abc\ndef\t");
        Native.inputText(input, "あ".repeat(300));
        Native.inputText(input, "😀漢字");
        Thread.sleep(1000);
        Native.inputClose(input);
        System.out.println(t() + " sent");
        System.exit(0);
    }
}
