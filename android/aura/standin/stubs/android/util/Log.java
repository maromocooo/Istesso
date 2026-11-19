// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): a stand-in for the device on a desktop JVM.
package android.util;
public final class Log {
    public static int i(String tag, String msg) { System.out.println(Stamp.now() + " log " + msg); return 0; }
    public static int w(String tag, String msg) { System.out.println(Stamp.now() + " warn " + msg); return 0; }
    public static final class Stamp { static final long T0 = System.nanoTime(); public static String now() { return String.format(java.util.Locale.ROOT, "%8.2f s", (System.nanoTime() - T0) / 1e9); } }
}
