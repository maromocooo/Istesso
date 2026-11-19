// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): a stand-in for the device on a desktop JVM.
package android.media;
public final class MediaCodecInfo {
    public boolean isEncoder() { return false; }
    public boolean isHardwareAccelerated() { return true; }
    public boolean isSoftwareOnly() { return false; }
    public String getName() { return "stand-in"; }
    public String[] getSupportedTypes() { return new String[0]; }
    public CodecCapabilities getCapabilitiesForType(String t) { return new CodecCapabilities(); }
    public static final class CodecCapabilities { public static final String FEATURE_LowLatency = "low-latency"; public boolean isFeatureSupported(String f) { return true; } }
}
