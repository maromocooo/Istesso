// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): a stand-in for the device on a desktop JVM.
package android.media;
public final class MediaFormat {
    public static final String MIMETYPE_VIDEO_HEVC = "video/hevc";
    public static final String KEY_LOW_LATENCY = "low-latency", KEY_PRIORITY = "priority", KEY_MAX_INPUT_SIZE = "max-input-size", KEY_FRAME_RATE = "frame-rate";
    public static MediaFormat createVideoFormat(String mime, int w, int h) { return new MediaFormat(); }
    public void setInteger(String k, int v) {}
}
