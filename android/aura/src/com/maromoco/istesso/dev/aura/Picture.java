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
import java.util.concurrent.atomic.AtomicLong;

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
        /** `from`: the picture that connected, so that an earlier one of the panel is told apart. */
        void connected(Picture from, int width, int height, int fps, int chroma);
        void cut(long cuts);
        /**
         * The host said that its picture is over for now (its pipeline ended, it was put aside for
         * a measurement, or it is shown to another device): a new connection is tried in `waitMs`.
         * `why` is one of the OVER_* numbers.
         */
        void reconnecting(int why, long waitMs, int attempt);
        void decoder(String name, boolean lowLatency);
        void over(String why);
    }

    /** istesso_media.h, ISTESSO_MEDIA_OVER_*: why watchNext said that it is over. */
    static final int OVER_ENDED = 1;
    static final int OVER_NOT_SHOWN = 2;
    static final int OVER_SET_ASIDE = 3;

    /**
     * How long before the next try after the host said that the picture is over, the first time
     * and at most: doubled each time in between, and back to the first once a frame has come.
     * (Within one connection, a cut is taken up again by the library: at once, then every
     * quarter of a second for ten seconds, then every second.)
     */
    static final long AGAIN_FIRST_MS = 250;
    static final long AGAIN_MOST_MS = 5000;

    /** The time before try `attempt` (1, 2, ...): 0.25 s, 0.5 s, 1 s, 2 s, 4 s, then 5 s. */
    static long againAfter(int attempt) {
        long wait = AGAIN_FIRST_MS << Math.min(Math.max(attempt - 1, 0), 10);
        return Math.min(wait, AGAIN_MOST_MS);
    }

    /**
     * Not one of the host's reasons: no connection was made for NOT_YET_AFTER_MS (the host does
     * not answer: its pipeline is not there, it is held still for a measurement, the network is
     * away). The connection is let go and tried again after the wait, so that a host that stays
     * away is knocked at every five seconds or so, not every second.
     */
    static final int NOT_YET = 100;
    static final long NOT_YET_AFTER_MS = 3000;

    /** Whether the reason is one that passes, so that the panel tries again. */
    static boolean passes(int why) {
        return why == OVER_ENDED || why == OVER_NOT_SHOWN || why == OVER_SET_ASIDE || why == NOT_YET;
    }

    private final String configDir;
    private final String address;
    /** The paired host's id: the connection is for that device only. */
    private final String peer;
    private final Surface surface;
    private final Told told;
    private volatile boolean stop;
    private Thread receiver;

    private MediaCodec codec;
    private Thread drainer;
    private volatile boolean draining;
    /** The decoder's input buffer this thread holds, or -1; kept from one connection to the next. */
    private int held = -1;

    // ---- counters for the once-a-second log line (sizes and times only, nothing of the content) ----
    private final AtomicLong framesIn = new AtomicLong();
    private final AtomicLong bytesIn = new AtomicLong();
    private final AtomicLong frameMaxBytes = new AtomicLong();
    private final AtomicLong queuedWindow = new AtomicLong();
    private final AtomicLong renderedWindow = new AtomicLong();
    private final AtomicLong queuedTotal = new AtomicLong();
    private final AtomicLong renderedTotal = new AtomicLong();
    private final AtomicLong dropped = new AtomicLong();
    private final AtomicLong inputWaitNs = new AtomicLong();
    private final AtomicLong lagSumNs = new AtomicLong();
    private final AtomicLong lagCount = new AtomicLong();
    private final AtomicLong lagMaxNs = new AtomicLong();
    private final AtomicLong gapMaxNs = new AtomicLong();
    private final AtomicLong cuts = new AtomicLong();
    private volatile long lastArrivalNs;

    // ---- the one-way delay's increase (TRIAL; docs/m8-aura-prototype.md, 7.5) ----
    // d = arrival here (this device's monotonic clock) - the host's encoder end (the host's
    // clock). The clocks are not aligned, so d itself means nothing; its increase over a
    // baseline does. The baseline is the least d of the first 2 s after each connection, then the
    // least d of each 60 s window, taken over at the window's end (the clocks drift apart slowly).
    private static final long WARM_NS = 2_000_000_000L;
    private static final long REBASE_NS = 60_000_000_000L;
    private long baseline = Long.MAX_VALUE;
    private boolean baselineReady;
    private long baselineFrom;
    private long windowMin = Long.MAX_VALUE;
    private long windowFrom;
    private final AtomicLong onewaySumNs = new AtomicLong();
    private final AtomicLong onewayCount = new AtomicLong();
    private final AtomicLong onewayMaxNs = new AtomicLong();

    /** On the receiving thread, for every frame. */
    private void oneWay(long arrivedNs, long encodedNs) {
        if (encodedNs == 0) {
            return;
        }
        long d = arrivedNs - encodedNs;
        if (!baselineReady) {
            if (baselineFrom == 0) {
                baselineFrom = arrivedNs;
            }
            baseline = Math.min(baseline, d);
            if (arrivedNs - baselineFrom >= WARM_NS) {
                baselineReady = true;
                windowFrom = arrivedNs;
                windowMin = Long.MAX_VALUE;
            }
            return;
        }
        windowMin = Math.min(windowMin, d);
        if (arrivedNs - windowFrom >= REBASE_NS) {
            baseline = windowMin;
            windowMin = Long.MAX_VALUE;
            windowFrom = arrivedNs;
        }
        long up = Math.max(0, d - baseline);
        onewaySumNs.addAndGet(up);
        onewayCount.incrementAndGet();
        max(onewayMaxNs, up);
    }

    /** A new connection: a new baseline (the host's pipeline clock may have started anew). */
    private void newBaseline() {
        baseline = Long.MAX_VALUE;
        baselineReady = false;
        baselineFrom = 0;
    }
    /** When each queued frame had arrived, by its presentation time, for the time to its render. */
    private final long[] ringPtsUs = new long[128];
    private final long[] ringArrivalNs = new long[128];
    private int ringNext;

    private void remember(long ptsUs, long arrivalNs) {
        synchronized (ringPtsUs) {
            ringPtsUs[ringNext] = ptsUs;
            ringArrivalNs[ringNext] = arrivalNs;
            ringNext = (ringNext + 1) % ringPtsUs.length;
        }
    }

    private long arrivalOf(long ptsUs) {
        synchronized (ringPtsUs) {
            for (int i = 0; i < ringPtsUs.length; i++) {
                if (ringPtsUs[i] == ptsUs && ringArrivalNs[i] != 0) {
                    long at = ringArrivalNs[i];
                    ringArrivalNs[i] = 0;
                    return at;
                }
            }
        }
        return 0;
    }

    private static void max(AtomicLong m, long v) {
        m.accumulateAndGet(v, Math::max);
    }

    /**
     * The counters of the last window as one line, and a new window. Frames and bytes received,
     * frames queued to the decoder and rendered, frames in the decoder, time spent waiting for a
     * decoder input buffer, time from a frame's arrival (whole, on the monotonic clock) to its
     * release for rendering, frames dropped here, the longest time without a frame, cuts so far.
     */
    String statsLine(long windowNs) {
        long now = System.nanoTime();
        long frames = framesIn.getAndSet(0);
        long bytes = bytesIn.getAndSet(0);
        long q = queuedWindow.getAndSet(0);
        long r = renderedWindow.getAndSet(0);
        long n = lagCount.getAndSet(0);
        long sum = lagSumNs.getAndSet(0);
        long lmax = lagMaxNs.getAndSet(0);
        long gap = gapMaxNs.getAndSet(0);
        long ow = onewayCount.getAndSet(0);
        long owSum = onewaySumNs.getAndSet(0);
        long owMax = onewayMaxNs.getAndSet(0);
        long last = lastArrivalNs;
        if (last != 0) {
            gap = Math.max(gap, now - last);
        }
        double seconds = windowNs / 1e9;
        return String.format(java.util.Locale.ROOT,
                "frames=%d mbit=%.1f frame_bytes_max=%d oneway_up_ms_max=%s oneway_up_ms_mean=%s quickack_rearms=%d queued=%d rendered=%d in_decoder=%d input_wait_ms=%.1f render_ms_max=%.1f render_ms_mean=%.1f dropped=%d gap_ms_max=%.0f cuts=%d",
                frames, bytes * 8 / 1e6 / seconds, frameMaxBytes.getAndSet(0),
                ow == 0 ? "-" : String.format(java.util.Locale.ROOT, "%.1f", owMax / 1e6),
                ow == 0 ? "-" : String.format(java.util.Locale.ROOT, "%.1f", owSum / 1e6 / ow),
                Native.quickackRearms(), q, r, queuedTotal.get() - renderedTotal.get(),
                inputWaitNs.getAndSet(0) / 1e6, lmax / 1e6, n == 0 ? 0 : sum / 1e6 / n,
                dropped.getAndSet(0), gap / 1e6, cuts.get());
    }
    private int width;
    private int height;

    Picture(String configDir, String address, String peer, Surface surface, Told told) {
        this.configDir = configDir;
        this.address = address;
        this.peer = peer;
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
        if (peer == null || peer.trim().isEmpty()) {
            // Never a connection to whichever paired device answers there.
            told.over("no-peer");
            return;
        }
        int attempt = 0;
        try {
            while (!stop) {
                long watch = Native.watchOpen(configDir, address, peer);
                if (watch == 0) {
                    String why = Native.lastError(Native.ERROR_PAIRING);
                    told.over(why.isEmpty() ? Native.lastError(Native.ERROR_PICTURE) : why);
                    return;
                }
                if (attempt == 0) {
                    told.connecting();
                }
                int why;
                try {
                    why = watch(watch);
                } finally {
                    Native.watchClose(watch);
                }
                if (why == WATCH_DONE) {
                    return; // asked to stop, or over for good (told already)
                }
                // After a connection that brought frames, the waits begin again at the first.
                attempt = framesCame ? 1 : attempt + 1;
                long wait = againAfter(attempt);
                Log.i("IstessoAura", "picture: " + (why == NOT_YET ? "no connection for " + NOT_YET_AFTER_MS + " ms"
                        : "the host said it is over (" + why + ")") + "; trying again in " + wait + " ms");
                told.reconnecting(why, wait, attempt);
                long until = System.nanoTime() + wait * 1_000_000L;
                while (!stop && System.nanoTime() < until) {
                    try {
                        Thread.sleep(50);
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        return;
                    }
                }
            }
        } finally {
            closeCodec();
        }
    }

    /** watch(): asked to stop, or over for good (the screen has been told). */
    private static final int WATCH_DONE = 0;
    /** Whether the last watch() brought any frame. Only the receiving thread uses it. */
    private boolean framesCame;

    /**
     * Takes the frames of one lasting connection until it is over. Returns WATCH_DONE, or the
     * host's reason (OVER_*) when it is one that passes.
     */
    private int watch(long watch) {
        long[] meta = new long[7];
        boolean waitForKey = true;
        framesCame = false;
        boolean connected = false;
        long waitingSince = System.nanoTime();
        while (!stop) {
            if (!connected && System.nanoTime() - waitingSince > NOT_YET_AFTER_MS * 1_000_000L) {
                cuts.incrementAndGet();
                return NOT_YET;
            }
            if (codec != null && held < 0) {
                long askedAt = System.nanoTime();
                try {
                    held = codec.dequeueInputBuffer(10_000);
                    inputWaitNs.addAndGet(System.nanoTime() - askedAt);
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
            if (stop) {
                // The panel has gone while this waited (a first connection may take up to 3 s,
                // longer than stop() waits): nothing more is opened or told, and the connection
                // is closed by the caller.
                return WATCH_DONE;
            }
            switch (got) {
                case Native.NEXT_FRAME: {
                    // When the whole frame had been read: istesso_media_now_ns(), which on Android is the
                    // monotonic clock, the one System.nanoTime() reads.
                    long arrived = meta[5];
                    framesIn.incrementAndGet();
                    bytesIn.addAndGet(meta[0]);
                    max(frameMaxBytes, meta[0]);
                    if (lastArrivalNs != 0) {
                        max(gapMaxNs, arrived - lastArrivalNs);
                    }
                    lastArrivalNs = arrived;
                    oneWay(arrived, meta[6]);
                    framesCame = true;
                    if (codec == null || held < 0) {
                        dropped.incrementAndGet();
                        break;
                    }
                    boolean key = meta[3] != 0;
                    if (waitForKey && !key) {
                        dropped.incrementAndGet();
                        break; // keep the buffer for the next frame
                    }
                    waitForKey = false;
                    int length = (int) meta[0];
                    long ptsUs = meta[2] / 1000;
                    remember(ptsUs, arrived);
                    codec.queueInputBuffer(held, 0, length, ptsUs, key ? MediaCodec.BUFFER_FLAG_KEY_FRAME : 0);
                    queuedWindow.incrementAndGet();
                    queuedTotal.incrementAndGet();
                    held = -1;
                    break;
                }
                case Native.NEXT_CONNECTED: {
                    connected = true;
                    newBaseline();
                    int[] info = Native.watchInfo(watch);
                    if (info == null) {
                        break;
                    }
                    told.connected(this, info[1], info[2], info[3], info[4]);
                    if (info[0] != CODEC_H265 || info[4] != CHROMA_420) {
                        told.over("unsupported:" + info[4]);
                        return WATCH_DONE;
                    }
                    if (codec == null || info[1] != width || info[2] != height) {
                        closeCodec();
                        held = -1;
                        if (!openCodec(info[1], info[2], info[3])) {
                            return WATCH_DONE;
                        }
                    }
                    // The host begins every connection with a key frame; nothing before it is decoded.
                    waitForKey = true;
                    break;
                }
                case Native.NEXT_CUT: {
                    cuts.incrementAndGet();
                    Log.i("IstessoAura", "picture: cut, connecting again");
                    long[] state = Native.watchState(watch);
                    told.cut(state == null ? 0 : state[0]);
                    waitForKey = true;
                    connected = false;
                    waitingSince = System.nanoTime();
                    break;
                }
                case Native.NEXT_TOO_BIG:
                    dropped.incrementAndGet();
                    Log.w(TAG, "a frame did not fit the decoder's buffer; a key frame is asked for");
                    waitForKey = true;
                    Native.watchAskKeyFrame(watch);
                    break;
                case Native.NEXT_OVER: {
                    long[] state = Native.watchState(watch);
                    int why = state == null ? 0 : (int) state[3];
                    if (passes(why)) {
                        cuts.incrementAndGet();
                        return why;
                    }
                    told.over(Native.lastError(Native.ERROR_PICTURE));
                    return WATCH_DONE;
                }
                default:
                    break; // nothing, or the cursor (not used: the host draws it)
            }
        }
        return WATCH_DONE;
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
                    long at = arrivalOf(info.presentationTimeUs);
                    if (at != 0) {
                        long lag = System.nanoTime() - at;
                        lagSumNs.addAndGet(lag);
                        lagCount.incrementAndGet();
                        max(lagMaxNs, lag);
                    }
                    renderedWindow.incrementAndGet();
                    renderedTotal.incrementAndGet();
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
        // What was in the old decoder is gone with it.
        queuedTotal.set(renderedTotal.get());
    }
}
