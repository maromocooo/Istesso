// Shows the video of a GStreamer pipeline in a window placed where asked.
//
// Step 3 of the M1 plan receives the stream with ready-made GStreamer elements. Their own
// window cannot be placed, and it always opens on the main display. For a measurement the
// relayed picture has to sit next to a local window on a chosen display, so this program opens
// the window itself and hands its view to the pipeline's video sink.
//
// Usage: gst-viewer X Y WIDTH HEIGHT TITLE PIPELINE [FRAME_LOG]
//   X Y            top left corner of the window, in points from the top left of the main display
//   WIDTH HEIGHT   size of the picture area, in points
//   PIPELINE       a gst-launch style description ending in a video sink, for example
//                  "udpsrc port=5004 caps=... ! rtph265depay ! h265parse ! avdec_h265 ! glimagesink sync=false"
//   FRAME_LOG      optional: a file to write the way of every frame through the pipeline to
//
// Decoding with VideoToolbox. GStreamer's own VideoToolbox element holds frames back, so the
// pipeline may instead end in an application sink called "vt" that hands over H.265 access
// units:
//                  "udpsrc name=net ... ! rtph265depay ! h265parse !
//                   video/x-h265,stream-format=hvc1,alignment=au ! appsink name=vt sync=false"
// This program then decodes them with VideoToolbox, one by one as they arrive, and gives each
// decoded picture to the window's layer without copying it. This is what the own client of
// step 4 will do; here it is measured with everything else left as it is.
//
// The frame log says where the time on the client goes. It has one JSON object per line, with
// times in nanoseconds on the clock the screen probe uses (mach absolute time), so its lines can
// be set against the probe's log directly. The pipeline names the elements to watch; any of the
// three may be left out.
//
//   name=net   an RTP source     {"kind":"arrived","rtp":R,"first_ns":T,"last_ns":T,"packets":N,"bytes":N}
//                                one line per RTP timestamp, written when the packet with the
//                                marker bit arrives: first and last packet of the frame
//   name=dec   the decoder       {"kind":"decoded","pts":P,"t_ns":T,"luma":Y}
//                                a frame left the decoder; Y is the luma in its middle
//   name=sink  the video sink    {"kind":"sink","pts":P,"t_ns":T}    the frame reached the sink
//                                {"kind":"drawn","pts":P,"t_ns":T}   the sink drew it (glimagesink only)
//   name=vt    VideoToolbox      the same three lines: "decoded" when VideoToolbox returns the
//                                picture, "sink" at once after it, "drawn" when the picture has
//                                been handed to the window's layer
//
// {"kind":"clock","mono_ns":T,"unix_ns":U} lines pair that clock with the time of day.
//
// Only for measurements: the own client of step 4 replaces it. GStreamer is linked dynamically.

#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#import <VideoToolbox/VideoToolbox.h>
#include <gst/app/gstappsink.h>
#include <gst/gst.h>
#include <gst/video/video.h>
#include <gst/video/videooverlay.h>
#include <time.h>

static FILE *frame_log = NULL;

// Nanoseconds of mach absolute time: the clock ScreenCaptureKit stamps frames with.
static guint64 now_ns(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

static void log_clock(void) {
    guint64 before = now_ns();
    struct timespec day;
    clock_gettime(CLOCK_REALTIME, &day);
    guint64 after = now_ns();
    fprintf(frame_log, "{\"kind\":\"clock\",\"mono_ns\":%llu,\"unix_ns\":%llu}\n",
            (unsigned long long)(before + (after - before) / 2),
            (unsigned long long)day.tv_sec * 1000000000ULL + (unsigned long long)day.tv_nsec);
}

// The frame whose packets are arriving. Only touched on the streaming thread of the source.
static struct {
    gboolean open;
    guint32 rtp;
    guint64 first_ns, last_ns, bytes;
    guint packets;
} arriving;

static void close_arriving(void) {
    if (!arriving.open) {
        return;
    }
    fprintf(frame_log, "{\"kind\":\"arrived\",\"rtp\":%u,\"first_ns\":%llu,\"last_ns\":%llu,\"packets\":%u,\"bytes\":%llu}\n",
            arriving.rtp, (unsigned long long)arriving.first_ns, (unsigned long long)arriving.last_ns,
            arriving.packets, (unsigned long long)arriving.bytes);
    arriving.open = FALSE;
}

static gboolean on_packet(GstBuffer **buffer, guint index, gpointer user_data) {
    guint64 t = *(guint64 *)user_data;
    guint8 header[12];
    if (gst_buffer_extract(*buffer, 0, header, sizeof header) != sizeof header) {
        return TRUE;
    }
    guint32 rtp = ((guint32)header[4] << 24) | ((guint32)header[5] << 16) | ((guint32)header[6] << 8) | header[7];
    if (arriving.open && arriving.rtp != rtp) {
        close_arriving();  // The packet with the marker bit never came.
    }
    if (!arriving.open) {
        arriving.open = TRUE;
        arriving.rtp = rtp;
        arriving.first_ns = t;
        arriving.packets = 0;
        arriving.bytes = 0;
    }
    arriving.last_ns = t;
    arriving.packets += 1;
    arriving.bytes += gst_buffer_get_size(*buffer);
    if (header[1] & 0x80) {
        close_arriving();
    }
    return TRUE;
}

static GstPadProbeReturn on_arrival(GstPad *pad, GstPadProbeInfo *info, gpointer user_data) {
    guint64 t = now_ns();
    if (GST_PAD_PROBE_INFO_TYPE(info) & GST_PAD_PROBE_TYPE_BUFFER_LIST) {
        gst_buffer_list_foreach(GST_PAD_PROBE_INFO_BUFFER_LIST(info), on_packet, &t);
    } else {
        GstBuffer *buffer = GST_PAD_PROBE_INFO_BUFFER(info);
        on_packet(&buffer, 0, &t);
    }
    return GST_PAD_PROBE_OK;
}

// Luma in the middle of a frame whose first plane holds one byte of luma per pixel
// (I420, NV12 and their relatives). -1 when the frame cannot be read.
static int middle_luma(GstPad *pad, GstBuffer *buffer) {
    gsize offset;
    GstVideoMeta *meta = gst_buffer_get_video_meta(buffer);
    if (meta != NULL) {
        offset = meta->offset[0] + (gsize)(meta->height / 2) * (gsize)meta->stride[0] + meta->width / 2;
    } else {
        GstCaps *caps = gst_pad_get_current_caps(pad);
        GstVideoInfo video;
        gboolean known = caps != NULL && gst_video_info_from_caps(&video, caps);
        if (caps != NULL) {
            gst_caps_unref(caps);
        }
        if (!known) {
            return -1;
        }
        offset = GST_VIDEO_INFO_PLANE_OFFSET(&video, 0)
            + (gsize)(GST_VIDEO_INFO_HEIGHT(&video) / 2) * (gsize)GST_VIDEO_INFO_PLANE_STRIDE(&video, 0)
            + GST_VIDEO_INFO_WIDTH(&video) / 2;
    }
    guint8 luma;
    return gst_buffer_extract(buffer, offset, &luma, 1) == 1 ? luma : -1;
}

static GstPadProbeReturn on_decoded(GstPad *pad, GstPadProbeInfo *info, gpointer user_data) {
    guint64 t = now_ns();
    GstBuffer *buffer = GST_PAD_PROBE_INFO_BUFFER(info);
    fprintf(frame_log, "{\"kind\":\"decoded\",\"pts\":%llu,\"t_ns\":%llu,\"luma\":%d}\n",
            (unsigned long long)GST_BUFFER_PTS(buffer), (unsigned long long)t, middle_luma(pad, buffer));
    return GST_PAD_PROBE_OK;
}

static GstPadProbeReturn on_sink(GstPad *pad, GstPadProbeInfo *info, gpointer user_data) {
    guint64 t = now_ns();
    fprintf(frame_log, "{\"kind\":\"sink\",\"pts\":%llu,\"t_ns\":%llu}\n",
            (unsigned long long)GST_BUFFER_PTS(GST_PAD_PROBE_INFO_BUFFER(info)), (unsigned long long)t);
    return GST_PAD_PROBE_OK;
}

// glimagesink's "client-draw": emitted just before the sink draws a frame. Returning FALSE
// leaves the drawing to the sink.
static gboolean on_draw(GstElement *sink, gpointer context, GstSample *sample, gpointer user_data) {
    guint64 t = now_ns();
    GstBuffer *buffer = sample != NULL ? gst_sample_get_buffer(sample) : NULL;
    if (buffer != NULL) {
        fprintf(frame_log, "{\"kind\":\"drawn\",\"pts\":%llu,\"t_ns\":%llu}\n",
                (unsigned long long)GST_BUFFER_PTS(buffer), (unsigned long long)t);
    }
    return FALSE;
}

// ---- Decoding with VideoToolbox ----

static VTDecompressionSessionRef vt_session = NULL;
static CMVideoFormatDescriptionRef vt_format = NULL;
static GstCaps *vt_caps = NULL;
static CALayer *vt_layer = nil;
// Nothing can be decoded before the first key frame, nor after a frame that failed.
static gboolean vt_needs_key_frame = TRUE;

// Luma in the middle of a picture whose first plane is luma (420v and its relatives).
static int middle_luma_of_picture(CVPixelBufferRef picture) {
    int luma = -1;
    if (CVPixelBufferLockBaseAddress(picture, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        return luma;
    }
    if (CVPixelBufferIsPlanar(picture)) {
        const uint8_t *plane = CVPixelBufferGetBaseAddressOfPlane(picture, 0);
        size_t stride = CVPixelBufferGetBytesPerRowOfPlane(picture, 0);
        size_t width = CVPixelBufferGetWidthOfPlane(picture, 0), height = CVPixelBufferGetHeightOfPlane(picture, 0);
        if (plane != NULL) {
            luma = plane[(height / 2) * stride + width / 2];
        }
    }
    CVPixelBufferUnlockBaseAddress(picture, kCVPixelBufferLock_ReadOnly);
    return luma;
}

// VideoToolbox has decoded a frame. Called before the call that asked for it returns.
static void on_picture(void *context, void *frame, OSStatus status, VTDecodeInfoFlags flags, CVImageBufferRef picture,
                       CMTime time, CMTime duration) {
    guint64 decoded = now_ns();
    unsigned long long pts = (unsigned long long)(uintptr_t)frame;
    if (status != noErr || picture == NULL) {
        return;
    }
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"decoded\",\"pts\":%llu,\"t_ns\":%llu,\"luma\":%d}\n", pts,
                (unsigned long long)decoded, middle_luma_of_picture(picture));
        fprintf(frame_log, "{\"kind\":\"sink\",\"pts\":%llu,\"t_ns\":%llu}\n", pts, (unsigned long long)now_ns());
    }
    // The layer takes the decoder's own surface: nothing is copied or converted here.
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    vt_layer.contents = (__bridge id)CVPixelBufferGetIOSurface(picture);
    [CATransaction commit];
    [CATransaction flush];
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"drawn\",\"pts\":%llu,\"t_ns\":%llu}\n", pts, (unsigned long long)now_ns());
    }
}

// Sets VideoToolbox up for the stream the caps describe (H.265 with its parameter sets in "codec_data").
static gboolean vt_configure(GstCaps *caps) {
    if (vt_session != NULL) {
        VTDecompressionSessionInvalidate(vt_session);
        CFRelease(vt_session);
        vt_session = NULL;
    }
    if (vt_format != NULL) {
        CFRelease(vt_format);
        vt_format = NULL;
    }
    gst_caps_replace(&vt_caps, caps);
    vt_needs_key_frame = TRUE;

    GstStructure *stream = gst_caps_get_structure(caps, 0);
    int width = 0, height = 0;
    const GValue *value = gst_structure_get_value(stream, "codec_data");
    if (!gst_structure_get_int(stream, "width", &width) || !gst_structure_get_int(stream, "height", &height) || value == NULL) {
        fprintf(stderr, "error: the stream does not say its size and parameter sets; ask h265parse for stream-format=hvc1\n");
        return FALSE;
    }
    GstMapInfo sets;
    if (!gst_buffer_map(gst_value_get_buffer(value), &sets, GST_MAP_READ)) {
        return FALSE;
    }
    NSDictionary *extensions = @{
        (__bridge NSString *)kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms :
            @{@"hvcC" : [NSData dataWithBytes:sets.data length:sets.size]}
    };
    gst_buffer_unmap(gst_value_get_buffer(value), &sets);
    OSStatus status = CMVideoFormatDescriptionCreate(kCFAllocatorDefault, kCMVideoCodecType_HEVC, width, height,
                                                     (__bridge CFDictionaryRef)extensions, &vt_format);
    if (status != noErr) {
        fprintf(stderr, "error: VideoToolbox does not take the stream's parameter sets (%d)\n", (int)status);
        return FALSE;
    }
    NSDictionary *decoder = @{(__bridge NSString *)kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder : @YES};
    // Pictures the window system can show as they are: 4:2:0 on a surface it shares with the decoder.
    NSDictionary *pictures = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{}
    };
    VTDecompressionOutputCallbackRecord callback = {on_picture, NULL};
    status = VTDecompressionSessionCreate(kCFAllocatorDefault, vt_format, (__bridge CFDictionaryRef)decoder,
                                          (__bridge CFDictionaryRef)pictures, &callback, &vt_session);
    if (status != noErr) {
        fprintf(stderr, "error: VideoToolbox cannot decode this stream (%d)\n", (int)status);
        return FALSE;
    }
    VTSessionSetProperty(vt_session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue);
    return TRUE;
}

// One access unit has arrived: decode it now, on the thread that received it.
static GstFlowReturn on_access_unit(GstAppSink *sink, gpointer user_data) {
    GstSample *sample = gst_app_sink_pull_sample(sink);
    if (sample == NULL) {
        return GST_FLOW_OK;
    }
    GstBuffer *buffer = gst_sample_get_buffer(sample);
    GstCaps *caps = gst_sample_get_caps(sample);
    if ((vt_caps == NULL || !gst_caps_is_equal(vt_caps, caps)) && !vt_configure(caps)) {
        gst_sample_unref(sample);
        return GST_FLOW_ERROR;
    }
    GstMapInfo unit;
    if ((vt_needs_key_frame && GST_BUFFER_FLAG_IS_SET(buffer, GST_BUFFER_FLAG_DELTA_UNIT)) ||
        !gst_buffer_map(buffer, &unit, GST_MAP_READ)) {
        gst_sample_unref(sample);
        return GST_FLOW_OK;
    }
    CMBlockBufferRef block = NULL;
    CMSampleBufferRef frame = NULL;
    const size_t size = unit.size;
    OSStatus status = CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, NULL, size, kCFAllocatorDefault, NULL, 0, size,
                                                         kCMBlockBufferAssureMemoryNowFlag, &block);
    if (status == noErr) {
        status = CMBlockBufferReplaceDataBytes(unit.data, block, 0, size);
    }
    if (status == noErr) {
        status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, vt_format, 1, 0, NULL, 1, &size, &frame);
    }
    gst_buffer_unmap(buffer, &unit);
    if (status == noErr) {
        // No flags: decoded here and now, and `on_picture` has run by the time this returns.
        status = VTDecompressionSessionDecodeFrame(vt_session, frame, 0, (void *)(uintptr_t)GST_BUFFER_PTS(buffer), NULL);
    }
    vt_needs_key_frame = status != noErr;
    if (frame != NULL) {
        CFRelease(frame);
    }
    if (block != NULL) {
        CFRelease(block);
    }
    gst_sample_unref(sample);
    return GST_FLOW_OK;
}

// Whether the pipeline ends in an application sink called "vt". If so, its access units go to
// VideoToolbox and the pictures to the layer of `view`.
static gboolean use_videotoolbox(GstElement *pipeline, NSView *view) {
    GstElement *sink = gst_bin_get_by_name(GST_BIN(pipeline), "vt");
    if (sink == NULL) {
        return FALSE;
    }
    if (!GST_IS_APP_SINK(sink)) {
        gst_object_unref(sink);
        return FALSE;
    }
    view.wantsLayer = YES;
    vt_layer = view.layer;
    vt_layer.contentsGravity = kCAGravityResize;
    vt_layer.backgroundColor = CGColorGetConstantColor(kCGColorBlack);
    GstAppSinkCallbacks callbacks = {0};
    callbacks.new_sample = on_access_unit;
    gst_app_sink_set_callbacks(GST_APP_SINK(sink), &callbacks, NULL, NULL);
    gst_object_unref(sink);
    return TRUE;
}

// Adds a probe for buffers to a pad of the element called `name`. Says so when there is none.
static void watch(GstElement *pipeline, const char *name, const char *pad_name, GstPadProbeType type,
                  GstPadProbeCallback callback) {
    GstElement *element = gst_bin_get_by_name(GST_BIN(pipeline), name);
    GstPad *pad = element != NULL ? gst_element_get_static_pad(element, pad_name) : NULL;
    if (pad == NULL) {
        fprintf(stderr, "note: no element with name=%s in the pipeline; the frame log leaves it out\n", name);
    } else {
        gst_pad_add_probe(pad, type, callback, NULL, NULL);
        gst_object_unref(pad);
    }
    if (element != NULL) {
        gst_object_unref(element);
    }
}

static gboolean start_frame_log(GstElement *pipeline, const char *path, gboolean videotoolbox) {
    frame_log = fopen(path, "wx");  // Never overwrite an earlier measurement.
    if (frame_log == NULL) {
        fprintf(stderr, "error: cannot create %s: %s\n", path, strerror(errno));
        return FALSE;
    }
    struct timespec day;
    clock_gettime(CLOCK_REALTIME, &day);
    fprintf(frame_log, "{\"kind\":\"header\",\"side\":\"client\",\"tool\":\"gst-viewer\",\"version\":\"0.0.1\",\"started_unix_ms\":%llu}\n",
            (unsigned long long)day.tv_sec * 1000ULL + (unsigned long long)day.tv_nsec / 1000000ULL);
    log_clock();

    watch(pipeline, "net", "src", GST_PAD_PROBE_TYPE_BUFFER | GST_PAD_PROBE_TYPE_BUFFER_LIST, on_arrival);
    if (!videotoolbox) {
        // With VideoToolbox, `on_picture` writes these lines itself.
        watch(pipeline, "dec", "src", GST_PAD_PROBE_TYPE_BUFFER, on_decoded);
        watch(pipeline, "sink", "sink", GST_PAD_PROBE_TYPE_BUFFER, on_sink);
    }
    GstElement *sink = videotoolbox ? NULL : gst_bin_get_by_name(GST_BIN(pipeline), "sink");
    if (sink != NULL) {
        if (g_signal_lookup("client-draw", G_OBJECT_TYPE(sink)) != 0) {
            g_signal_connect(sink, "client-draw", G_CALLBACK(on_draw), NULL);
        }
        gst_object_unref(sink);
    }

    // The log reaches the disk twice a second, and once more when the program is asked to stop.
    static dispatch_source_t timer, stop[2];
    timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 2), NSEC_PER_SEC / 2, NSEC_PER_SEC / 20);
    dispatch_source_set_event_handler(timer, ^{
        log_clock();
        fflush(frame_log);
    });
    dispatch_resume(timer);
    int signals[2] = {SIGTERM, SIGINT};
    for (int i = 0; i < 2; i++) {
        signal(signals[i], SIG_IGN);
        stop[i] = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, signals[i], 0, dispatch_get_main_queue());
        dispatch_source_set_event_handler(stop[i], ^{
            log_clock();
            fflush(frame_log);
            exit(0);
        });
        dispatch_resume(stop[i]);
    }
    return TRUE;
}

static GstBusSyncReply on_message(GstBus *bus, GstMessage *message, gpointer user_data) {
    if (gst_is_video_overlay_prepare_window_handle_message(message)) {
        NSView *view = (__bridge NSView *)user_data;
        gst_video_overlay_set_window_handle(GST_VIDEO_OVERLAY(GST_MESSAGE_SRC(message)), (guintptr)view);
        gst_message_unref(message);
        return GST_BUS_DROP;
    }
    if (GST_MESSAGE_TYPE(message) == GST_MESSAGE_ERROR) {
        GError *error = NULL;
        gst_message_parse_error(message, &error, NULL);
        fprintf(stderr, "error: %s\n", error ? error->message : "the pipeline failed");
        exit(1);
    }
    if (GST_MESSAGE_TYPE(message) == GST_MESSAGE_EOS) {
        exit(0);
    }
    return GST_BUS_PASS;
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        gst_init(&argc, &argv);
        if (argc != 7 && argc != 8) {
            fprintf(stderr, "usage: gst-viewer X Y WIDTH HEIGHT TITLE PIPELINE [FRAME_LOG]\n");
            return 2;
        }
        double x = atof(argv[1]), y = atof(argv[2]);
        double width = atof(argv[3]), height = atof(argv[4]);
        if (width < 1 || height < 1) {
            fprintf(stderr, "error: WIDTH and HEIGHT have to be positive\n");
            return 2;
        }

        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, width, height)
                                                       styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable)
                                                         backing:NSBackingStoreBuffered
                                                           defer:NO];
        window.title = [NSString stringWithUTF8String:argv[5]];
        NSView *view = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
        window.contentView = view;
        [window orderFrontRegardless];
        // Moved after it exists: at creation macOS pulls a window that is placed on another
        // display than the one in use back onto that one. Coordinates count from the top left
        // of the main display, which is the first screen.
        double mainHeight = NSScreen.screens.firstObject.frame.size.height;
        [window setFrameTopLeftPoint:NSMakePoint(x, mainHeight - y)];

        GError *error = NULL;
        GstElement *pipeline = gst_parse_launch(argv[6], &error);
        if (pipeline == NULL) {
            fprintf(stderr, "error: %s\n", error ? error->message : "cannot build the pipeline");
            return 1;
        }
        gboolean videotoolbox = use_videotoolbox(pipeline, view);
        if (argc == 8 && !start_frame_log(pipeline, argv[7], videotoolbox)) {
            return 1;
        }
        GstBus *bus = gst_element_get_bus(pipeline);
        gst_bus_set_sync_handler(bus, on_message, (__bridge void *)view, NULL);
        gst_object_unref(bus);
        if (gst_element_set_state(pipeline, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE) {
            fprintf(stderr, "error: the pipeline does not start\n");
            return 1;
        }
        [NSApp run];
    }
    return 0;
}
