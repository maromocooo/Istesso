// PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
package com.maromoco.istesso.dev.aura;

import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.media.MediaFormat;
import android.os.Process;
import android.util.Log;
import android.view.Surface;

import java.nio.ByteBuffer;

/**
 * Receives the host's picture and decodes it in hardware onto a surface.
 *
 * <p>One thread receives: it holds an input buffer of the decoder, has the Rust library copy the
 * next frame straight into it (one copy), and queues it. A second thread takes the decoded
 * pictures out and shows each at once. Only the receiving thread touches the connection, and it
 * closes it when it ends.
 */
final class Picture {
    private static final String TAG = "istesso-aura";
    private static final String HEVC = MediaFormat.MIMETYPE_VIDEO_HEVC;
    /** The low-latency hardware decoder seen on AURA (docs/m8-aura-groundwork.md, 1.1). */
    private static final String PREFERRED = "c2.qti.hevc.decoder.low_latency";
    private static final int CODEC_H265 = 1;
    private static final int CHROMA_420 = 420;

    /** What the screen is told. Called on the picture's threads. */
    interface Told {
        void connecting();
        void connected(int width, int height, int fps, int chroma);
        void cut(long cuts);
        void decoder(String name, boolean lowLatency);
        void over(String why);
    }

    private final String configDir;
    private final String address;
    private final Surface surface;
    private final Told told;
    private volatile boolean stop;
    private Thread receiver;

    private MediaCodec codec;
    private Thread drainer;
    private volatile boolean draining;
    private int width;
    private int height;

    Picture(String configDir, String address, Surface surface, Told told) {
        this.configDir = configDir;
        this.address = address;
        this.surface = surface;
        this.told = told;
    }

    void start() {
        receiver = new Thread(this::receive, "istesso-picture");
        receiver.start();
    }

    /** Asks the threads to end and waits for them (at most about a second). */
    void stop() {
        stop = true;
        if (receiver != null) {
            try {
                receiver.join(1500);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
    }

    private static void urgent() {
        try {
            Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY);
        } catch (RuntimeException e) {
            Log.w(TAG, "the thread's priority could not be raised");
        }
    }

    private void receive() {
        urgent();
        long watch = Native.watchOpen(configDir, address);
        if (watch == 0) {
            told.over(Native.lastError(Native.ERROR_PICTURE));
            return;
        }
        told.connecting();
        long[] meta = new long[6];
        int held = -1;
        boolean waitForKey = true;
        try {
            while (!stop) {
                if (codec != null && held < 0) {
                    try {
                        held = codec.dequeueInputBuffer(10_000);
                    } catch (IllegalStateException e) {
                        Log.w(TAG, "the decoder failed; it is made again");
                        closeCodec();
                        held = -1;
                        waitForKey = true;
                        Native.watchAskKeyFrame(watch);
                        continue;
                    }
                    if (held < 0) {
                        continue;
                    }
                }
                ByteBuffer into = null;
                if (codec != null && held >= 0) {
                    into = codec.getInputBuffer(held);
                    if (into != null) {
                        into.clear();
                    }
                }
                if (into == null) {
                    // No decoder yet: room enough for a frame that is dropped anyway.
                    into = scratch();
                }
                int got = Native.watchNext(watch, 100, into, meta);
                switch (got) {
                    case Native.NEXT_FRAME:
                        if (codec == null || held < 0) {
                            break;
                        }
                        boolean key = meta[3] != 0;
                        if (waitForKey && !key) {
                            break; // keep the buffer for the next frame
                        }
                        waitForKey = false;
                        int length = (int) meta[0];
                        long ptsUs = meta[2] / 1000;
                        codec.queueInputBuffer(held, 0, length, ptsUs, key ? MediaCodec.BUFFER_FLAG_KEY_FRAME : 0);
                        held = -1;
                        break;
                    case Native.NEXT_CONNECTED: {
                        int[] info = Native.watchInfo(watch);
                        if (info == null) {
                            break;
                        }
                        told.connected(info[1], info[2], info[3], info[4]);
                        if (info[0] != CODEC_H265 || info[4] != CHROMA_420) {
                            told.over("unsupported:" + info[4]);
                            return;
                        }
                        if (codec == null || info[1] != width || info[2] != height) {
                            closeCodec();
                            held = -1;
                            if (!openCodec(info[1], info[2], info[3])) {
                                return;
                            }
                        }
                        waitForKey = true;
                        break;
                    }
                    case Native.NEXT_CUT:
                        long[] state = Native.watchState(watch);
                        told.cut(state == null ? 0 : state[0]);
                        waitForKey = true;
                        break;
                    case Native.NEXT_TOO_BIG:
                        Log.w(TAG, "a frame did not fit the decoder's buffer; a key frame is asked for");
                        waitForKey = true;
                        Native.watchAskKeyFrame(watch);
                        break;
                    case Native.NEXT_OVER:
                        told.over(Native.lastError(Native.ERROR_PICTURE));
                        return;
                    default:
                        break; // nothing, or the cursor (not used: the host draws it)
                }
            }
        } finally {
            closeCodec();
            Native.watchClose(watch);
        }
    }

    private ByteBuffer scratch;

    private ByteBuffer scratch() {
        if (scratch == null) {
            scratch = ByteBuffer.allocateDirect(8 << 20);
        }
        scratch.clear();
        return scratch;
    }

    private boolean openCodec(int w, int h, int fps) {
        MediaCodec made = null;
        boolean lowLatency = false;
        String name = null;
        try {
            made = MediaCodec.createByCodecName(PREFERRED);
            name = PREFERRED;
            lowLatency = true;
        } catch (Exception e) {
            name = pickDecoder();
            if (name != null) {
                try {
                    made = MediaCodec.createByCodecName(name);
                    lowLatency = isLowLatency(name);
                } catch (Exception e2) {
                    made = null;
                }
            }
        }
        if (made == null) {
            told.over("no-decoder");
            return false;
        }
        MediaFormat format = MediaFormat.createVideoFormat(HEVC, w, h);
        format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1);
        format.setInteger(MediaFormat.KEY_PRIORITY, 0); // real time
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, Math.max(1 << 20, w * h * 3 / 2));
        if (fps > 0) {
            format.setInteger(MediaFormat.KEY_FRAME_RATE, fps);
        }
        try {
            made.configure(format, surface, null, 0);
            made.start();
        } catch (RuntimeException e) {
            Log.w(TAG, "the decoder could not be started");
            made.release();
            told.over("no-decoder");
            return false;
        }
        codec = made;
        width = w;
        height = h;
        told.decoder(name, lowLatency);
        draining = true;
        drainer = new Thread(this::drain, "istesso-decoded");
        drainer.start();
        return true;
    }

    /** A hardware HEVC decoder, one with the low-latency feature first. Never a software one. */
    private static String pickDecoder() {
        String any = null;
        for (MediaCodecInfo info : new MediaCodecList(MediaCodecList.REGULAR_CODECS).getCodecInfos()) {
            if (info.isEncoder() || !info.isHardwareAccelerated() || info.isSoftwareOnly()) {
                continue;
            }
            for (String type : info.getSupportedTypes()) {
                if (!type.equalsIgnoreCase(HEVC)) {
                    continue;
                }
                MediaCodecInfo.CodecCapabilities caps = info.getCapabilitiesForType(type);
                if (caps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)) {
                    return info.getName();
                }
                if (any == null && !info.getName().endsWith(".secure")) {
                    any = info.getName();
                }
            }
        }
        return any;
    }

    private static boolean isLowLatency(String name) {
        for (MediaCodecInfo info : new MediaCodecList(MediaCodecList.REGULAR_CODECS).getCodecInfos()) {
            if (info.getName().equals(name)) {
                try {
                    return info.getCapabilitiesForType(HEVC)
                            .isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency);
                } catch (IllegalArgumentException e) {
                    return false;
                }
            }
        }
        return false;
    }

    /** Shows every decoded picture as soon as it is there. */
    private void drain() {
        urgent();
        MediaCodec c = codec;
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
        while (draining) {
            int index;
            try {
                index = c.dequeueOutputBuffer(info, 10_000);
            } catch (IllegalStateException e) {
                return;
            }
            if (index >= 0) {
                try {
                    c.releaseOutputBuffer(index, true);
                } catch (IllegalStateException e) {
                    return;
                }
            }
        }
    }

    private void closeCodec() {
        if (codec == null) {
            return;
        }
        draining = false;
        if (drainer != null) {
            try {
                drainer.join(500);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            drainer = null;
        }
        try {
            codec.stop();
        } catch (RuntimeException ignored) {
            // it may have failed already
        }
        codec.release();
        codec = null;
    }
}
