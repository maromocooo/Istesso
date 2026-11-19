// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): a stand-in for the device on a desktop JVM.
package com.maromoco.istesso.dev.aura;
import android.util.Log;
/** Stand-in for AURA's panel on a desktop JVM: the app's own Picture and Native, the real Rust library, no Android. */
public final class StandIn implements Picture.Told {
    static String t() { return Log.Stamp.now(); }
    public void connecting() { System.out.println(t() + " panel: つないでいます…"); }
    public void connected(int w, int h, int fps, int chroma) { System.out.println(t() + " panel: connected " + w + "x" + h + " " + fps + " fps " + chroma); }
    public void cut(long cuts) { System.out.println(t() + " panel: つなぎ直しています…（切れたのは " + cuts + " 回目）"); }
    public void reconnecting(int why, long waitMs, int attempt) { System.out.println(t() + " panel: つなぎ直しています… why=" + why + " wait=" + waitMs + " ms attempt=" + attempt); }
    public void decoder(String name, boolean low) {}
    public void over(String why) { System.out.println(t() + " panel: over: " + why); }
    public static void main(String[] a) throws Exception {
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
}
