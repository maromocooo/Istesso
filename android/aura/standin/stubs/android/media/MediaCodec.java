// PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): a stand-in for the device on a desktop JVM.
package android.media;
import java.nio.ByteBuffer;
import java.util.concurrent.ArrayBlockingQueue;
/** A decoder that decodes nothing: every queued frame comes out at once, in order. */
public final class MediaCodec {
    public static final int BUFFER_FLAG_KEY_FRAME = 1;
    public static int queuedKeys, queued;
    private final ByteBuffer[] buffers = new ByteBuffer[4];
    private final ArrayBlockingQueue<Integer> free = new ArrayBlockingQueue<>(4);
    private final ArrayBlockingQueue<long[]> out = new ArrayBlockingQueue<>(64);
    public static MediaCodec createByCodecName(String name) { return new MediaCodec(); }
    private MediaCodec() { for (int i = 0; i < 4; i++) { buffers[i] = ByteBuffer.allocateDirect(8 << 20); free.add(i); } }
    public void configure(MediaFormat f, android.view.Surface s, Object c, int flags) {}
    public void start() {}
    public void stop() {}
    public void release() {}
    public int dequeueInputBuffer(long us) { try { Integer i = free.poll(us, java.util.concurrent.TimeUnit.MICROSECONDS); return i == null ? -1 : i; } catch (InterruptedException e) { return -1; } }
    public ByteBuffer getInputBuffer(int i) { return buffers[i]; }
    public void queueInputBuffer(int i, int off, int len, long ptsUs, int flags) { queued++; if ((flags & BUFFER_FLAG_KEY_FRAME) != 0) queuedKeys++; free.add(i); out.offer(new long[] {ptsUs}); }
    public int dequeueOutputBuffer(BufferInfo info, long us) { try { long[] o = out.poll(us, java.util.concurrent.TimeUnit.MICROSECONDS); if (o == null) return -1; info.presentationTimeUs = o[0]; return 0; } catch (InterruptedException e) { return -1; } }
    public void releaseOutputBuffer(int i, boolean render) {}
    public static final class BufferInfo { public long presentationTimeUs; }
}
