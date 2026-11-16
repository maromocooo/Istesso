// Shows the video of a GStreamer pipeline in a window placed where asked.
//
// Step 3 of the M1 plan receives the stream with ready-made GStreamer elements. Their own
// window cannot be placed, and it always opens on the main display. For a measurement the
// relayed picture has to sit next to a local window on a chosen display, so this program opens
// the window itself and hands its view to the pipeline's video sink.
//
// Usage: gst-viewer X Y WIDTH HEIGHT TITLE PIPELINE [FRAME_LOG [KEY_REQUEST_PORT [INPUT_SOCKET]]]
//   X Y            top left corner of the window, in points from the top left of the main display
//   WIDTH HEIGHT   size of the picture area, in points. 0 0: the whole display that X Y lies on,
//                  without a title bar (a verdict is measured with the relayed picture shown
//                  like that, one pixel of it to one of the display)
//   PIPELINE       a gst-launch style description ending in a video sink, for example
//                  "udpsrc port=5004 caps=... ! rtph265depay ! h265parse ! avdec_h265 ! glimagesink sync=false"
//   FRAME_LOG      optional: a file to write the way of every frame through the pipeline to;
//                  "-" for none
//   KEY_REQUEST_PORT  optional, with VideoToolbox: ask the sender for a key frame on this port
//                  whenever one is needed (see below)
//   INPUT_SOCKET   optional, with VideoToolbox: write what the person does in the window with
//                  keyboard and mouse to this local socket (see below)
// With ISTESSO_ON_TOP set in the environment the window stays above the others: a measuring tool
// that watches the middle of the window sees nothing once another window lies over it.
// The Command keys are passed on as Control, where applications on Linux expect what Command
// does on a Mac; with ISTESSO_COMMAND_KEY=super they are passed on as the Super keys.
//
// Decoding with VideoToolbox. GStreamer's own VideoToolbox element holds frames back, so the
// pipeline may instead end in an application sink called "vt" that hands over H.265 access
// units:
//                  "udpsrc name=net ... ! rtph265depay ! h265parse !
//                   video/x-h265,stream-format=hvc1,alignment=au ! appsink name=vt sync=false"
// This program then decodes them with VideoToolbox, one by one as they arrive, and gives each
// decoded picture to the window's layer without copying it. This is what the own client of
// step 4 will do; here it is measured with everything else left as it is. A stream that keeps
// all of its colour (4:4:4) is decoded to pictures that keep it, and one that says its picture
// is sRGB is shown the way the window system shows its own windows.
//
// Asking for key frames. A sender that makes key frames only when asked has to be asked: at the
// start, and whenever packets are missing, because every later frame builds on the one that
// was lost. With KEY_REQUEST_PORT this program sends a picture loss indication (RTCP, RFC 4585,
// as a packet on its own, RFC 5506) to that port of the address the stream comes from, and
// again every 50 ms until a key frame has been decoded. Until then the window keeps the last
// good picture. Not encrypted and not authenticated, like the stream itself.
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
//                                been handed to the window's layer; "decoded" says with "key":1
//                                that the picture was a key frame. Also, once per stream,
//                                {"kind":"decoder","hardware":0|1,"pictures":"420v"|"444v","stream_colours":C}
//                                C is what the stream says about its colours, in GStreamer's words
//                                {"kind":"colours","primaries":P,"transfer":T,"matrix":M}
//                                what the decoded pictures say about their colours
//   name=net with VideoToolbox   {"kind":"gap","t_ns":T,"missing":N}   N packets did not come
//                                {"kind":"key_request","t_ns":T,"sent":0|1}  a key frame was asked for
//
// {"kind":"clock","mono_ns":T,"unix_ns":U} lines pair that clock with the time of day.
//
// Input. With INPUT_SOCKET, keys, pointer positions, buttons and scrolling in the window are
// written to that Unix datagram socket, one event per datagram, as the JSON of
// istesso_proto::InputEvent. `istesso input relay` listens there and sends the events to the
// paired device over the control channel, encrypted; this program sends nothing of it over the
// network itself. Keys are named by their place on the keyboard, in Linux's numbers. The
// pointer's place is given in pixels of the picture.
//
// Only for measurements and first trials: the own client of step 4 replaces it. GStreamer is
// linked dynamically.

#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#import <VideoToolbox/VideoToolbox.h>
#import <IOSurface/IOSurface.h>
#include <gio/gio.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#include <gst/app/gstappsink.h>
#include <gst/gst.h>
#include <gst/net/gstnetaddressmeta.h>
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
    arriving.open = FALSE;
    if (frame_log == NULL) {
        return;
    }
    fprintf(frame_log, "{\"kind\":\"arrived\",\"rtp\":%u,\"first_ns\":%llu,\"last_ns\":%llu,\"packets\":%u,\"bytes\":%llu}\n",
            arriving.rtp, (unsigned long long)arriving.first_ns, (unsigned long long)arriving.last_ns,
            arriving.packets, (unsigned long long)arriving.bytes);
}

// ---- Keeping track of what can be decoded, and asking for a key frame ----
//
// All of it runs on the streaming thread of the source: packets come in there, and the access
// units they make up are handed to VideoToolbox there before the next packet is looked at.

// Nothing can be decoded before the first key frame, nor after a frame that failed or was lost.
static gboolean vt_needs_key_frame = TRUE;
// The frame that was arriving when packets were found missing. It may lack its beginning, so
// it is not decoded even if it says it is a key frame.
static gboolean damaged = FALSE;
static guint32 damaged_rtp = 0;
// RTP timestamp and sequence number of the packet looked at last.
static guint32 current_rtp = 0;
static gboolean have_sequence = FALSE;
static guint16 last_sequence = 0;
// Where key frames are asked for: 0 when they are not.
static guint16 key_request_port = 0;
static GSocket *key_request_socket = NULL;
static guint64 key_asked_ns = 0;

// Sends a picture loss indication to the sender of `packet`, at most every 50 ms.
static void ask_for_key_frame(GstBuffer *packet, const guint8 *header, guint64 t) {
    if (key_request_port == 0 || (key_asked_ns != 0 && t - key_asked_ns < 50 * 1000 * 1000ULL)) {
        return;
    }
    GstNetAddressMeta *from = gst_buffer_get_net_address_meta(packet);
    if (from == NULL || !G_IS_INET_SOCKET_ADDRESS(from->addr)) {
        return;
    }
    GInetAddress *sender = g_inet_socket_address_get_address(G_INET_SOCKET_ADDRESS(from->addr));
    if (key_request_socket == NULL) {
        key_request_socket = g_socket_new(g_inet_address_get_family(sender), G_SOCKET_TYPE_DATAGRAM, G_SOCKET_PROTOCOL_UDP, NULL);
        if (key_request_socket == NULL) {
            return;
        }
    }
    // Version 2, feedback message type 1 (picture loss), payload type 206 (payload-specific
    // feedback), two more words: who asks (nobody in particular) and the stream that is meant.
    guint8 request[12] = {0x81, 206, 0, 2, 0, 0, 0, 0, header[8], header[9], header[10], header[11]};
    GSocketAddress *to = g_inet_socket_address_new(sender, key_request_port);
    GError *error = NULL;
    gboolean sent = g_socket_send_to(key_request_socket, to, (const gchar *)request, sizeof request, NULL, &error) >= 0;
    g_object_unref(to);
    key_asked_ns = t;
    if (!sent) {
        // On macOS a program needs the user's leave to send to the local network; until it is
        // given, sending fails. Said once, so that a missing key frame does not stay a riddle.
        static gboolean said = FALSE;
        if (!said) {
            fprintf(stderr, "error: cannot ask the sender for a key frame: %s\n", error ? error->message : "unknown");
            said = TRUE;
        }
        g_clear_error(&error);
    }
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"key_request\",\"t_ns\":%llu,\"sent\":%d}\n", (unsigned long long)t, sent ? 1 : 0);
    }
}

static gboolean on_packet(GstBuffer **buffer, guint index, gpointer user_data) {
    guint64 t = *(guint64 *)user_data;
    guint8 header[12];
    if (gst_buffer_extract(*buffer, 0, header, sizeof header) != sizeof header) {
        return TRUE;
    }
    guint32 rtp = ((guint32)header[4] << 24) | ((guint32)header[5] << 16) | ((guint32)header[6] << 8) | header[7];
    guint16 sequence = (guint16)(((guint16)header[2] << 8) | header[3]);
    if (have_sequence && sequence != (guint16)(last_sequence + 1)) {
        // Packets are missing, or came out of order, which is as bad: the frame they belong to
        // cannot be decoded, nor any that builds on it.
        vt_needs_key_frame = TRUE;
        damaged = TRUE;
        damaged_rtp = rtp;
        if (frame_log != NULL) {
            fprintf(frame_log, "{\"kind\":\"gap\",\"t_ns\":%llu,\"missing\":%u}\n", (unsigned long long)t,
                    (unsigned)(guint16)(sequence - last_sequence - 1));
        }
    }
    have_sequence = TRUE;
    last_sequence = sequence;
    current_rtp = rtp;
    if (vt_needs_key_frame) {
        ask_for_key_frame(*buffer, header, t);
    }
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
// Whether the access unit VideoToolbox is decoding is a key frame.
static gboolean vt_decoding_key_frame = FALSE;
// Whether the stream says its picture is sRGB, as the content of any window is.
static gboolean vt_srgb = FALSE;

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
    if (vt_srgb) {
        // VideoToolbox hands the picture over as video (BT.709) whatever the stream says, and
        // the window system then shows it a little brighter than its own windows: a dark grey
        // of 32 comes out as 31 where a window's own comes out as 29. With the surface said to
        // be sRGB, it is shown like them. (Saying so on the picture alone changes nothing.)
        CVBufferSetAttachment(picture, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_sRGB,
                              kCVAttachmentMode_ShouldPropagate);
        static CFDataRef profile = NULL;
        if (profile == NULL) {
            CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            profile = CGColorSpaceCopyICCData(space);
            CGColorSpaceRelease(space);
        }
        IOSurfaceRef surface = CVPixelBufferGetIOSurface(picture);
        if (surface != NULL && profile != NULL) {
            IOSurfaceSetValue(surface, kIOSurfaceColorSpace, profile);
        }
    }
    static gboolean described = FALSE;
    if (!described && frame_log != NULL) {
        // What the picture says about its colours: this is what the window system goes by.
        described = TRUE;
        NSString *(^said)(CFStringRef) = ^NSString *(CFStringRef key) {
            CFTypeRef value = CVBufferCopyAttachment(picture, key, NULL);
            NSString *text = value != NULL && CFGetTypeID(value) == CFStringGetTypeID() ? [(__bridge NSString *)value copy] : @"-";
            if (value != NULL) {
                CFRelease(value);
            }
            return text;
        };
        fprintf(frame_log, "{\"kind\":\"colours\",\"primaries\":\"%s\",\"transfer\":\"%s\",\"matrix\":\"%s\"}\n",
                said(kCVImageBufferColorPrimariesKey).UTF8String, said(kCVImageBufferTransferFunctionKey).UTF8String,
                said(kCVImageBufferYCbCrMatrixKey).UTF8String);
    }
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"decoded\",\"pts\":%llu,\"t_ns\":%llu,\"luma\":%d,\"key\":%d}\n", pts,
                (unsigned long long)decoded, middle_luma_of_picture(picture), vt_decoding_key_frame ? 1 : 0);
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
// Size of the picture in its own pixels, once the stream has said it. Pointer positions are
// given in these.
static int picture_width = 0, picture_height = 0;

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
    picture_width = width;
    picture_height = height;
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
    // Pictures the window system can show as they are, on a surface it shares with the decoder,
    // with as much colour as the stream has: asked for 4:2:0, a 4:4:4 stream would lose it here.
    const char *colours = gst_structure_get_string(stream, "colorimetry");
    GstVideoColorimetry described;
    vt_srgb = colours != NULL && gst_video_colorimetry_from_string(&described, colours) && described.transfer == GST_VIDEO_TRANSFER_SRGB;
    const char *chroma = gst_structure_get_string(stream, "chroma-format");
    gboolean all_colour = chroma != NULL && strcmp(chroma, "4:4:4") == 0;
    NSDictionary *pictures = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey :
            @(all_colour ? kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange : kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
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
    CFBooleanRef hardware = NULL;
    VTSessionCopyProperty(vt_session, kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder, kCFAllocatorDefault, &hardware);
    gboolean in_hardware = hardware == kCFBooleanTrue;
    if (hardware != NULL) {
        CFRelease(hardware);
    }
    if (!in_hardware) {
        fprintf(stderr, "note: VideoToolbox decodes this stream in software\n");
    }
    if (frame_log != NULL) {
        const char *colorimetry = gst_structure_get_string(stream, "colorimetry");
        fprintf(frame_log, "{\"kind\":\"decoder\",\"hardware\":%d,\"pictures\":\"%s\",\"stream_colours\":\"%s\"}\n",
                in_hardware ? 1 : 0, all_colour ? "444v" : "420v", colorimetry != NULL ? colorimetry : "-");
    }
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
    gboolean key_frame = !GST_BUFFER_FLAG_IS_SET(buffer, GST_BUFFER_FLAG_DELTA_UNIT);
    // The frame that was arriving when packets went missing is left alone, whatever it is.
    gboolean whole = !(damaged && damaged_rtp == current_rtp);
    if (whole) {
        damaged = FALSE;
    }
    if (!whole || (vt_needs_key_frame && !key_frame) || !gst_buffer_map(buffer, &unit, GST_MAP_READ)) {
        gst_sample_unref(sample);
        return GST_FLOW_OK;
    }
    vt_decoding_key_frame = key_frame;
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

static void watch(GstElement *pipeline, const char *name, const char *pad_name, GstPadProbeType type,
                  GstPadProbeCallback callback);

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
    // Every packet is looked at: missing ones mean that a key frame is needed.
    watch(pipeline, "net", "src", GST_PAD_PROBE_TYPE_BUFFER | GST_PAD_PROBE_TYPE_BUFFER_LIST, on_arrival);
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

    if (!videotoolbox) {
        // With VideoToolbox the packets are watched already, and `on_picture` writes the other lines itself.
        watch(pipeline, "net", "src", GST_PAD_PROBE_TYPE_BUFFER | GST_PAD_PROBE_TYPE_BUFFER_LIST, on_arrival);
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

// ---- Passing on what the person does in the window ----

static int input_socket = -1;
static struct sockaddr_un input_to;

static void send_input(const char *format, ...) __attribute__((format(printf, 1, 2)));
static void send_input(const char *format, ...) {
    if (input_socket < 0) {
        return;
    }
    char text[128];
    va_list arguments;
    va_start(arguments, format);
    int length = vsnprintf(text, sizeof text, format, arguments);
    va_end(arguments);
    if (length > 0 && (size_t)length < sizeof text) {
        // Nobody listening is not an error: the relay may not have been started.
        sendto(input_socket, text, (size_t)length, 0, (const struct sockaddr *)&input_to, sizeof input_to);
    }
}

// Linux's number for a key (linux/input-event-codes.h), by macOS's number for the same place on
// the keyboard. 0 where there is no such key.
static const unsigned short linux_key[128] = {
    [0x00] = 30,  [0x01] = 31,  [0x02] = 32,  [0x03] = 33,  [0x04] = 35,  [0x05] = 34,  [0x06] = 44,  [0x07] = 45,
    [0x08] = 46,  [0x09] = 47,  [0x0A] = 86,  [0x0B] = 48,  [0x0C] = 16,  [0x0D] = 17,  [0x0E] = 18,  [0x0F] = 19,
    [0x10] = 21,  [0x11] = 20,  [0x12] = 2,   [0x13] = 3,   [0x14] = 4,   [0x15] = 5,   [0x16] = 7,   [0x17] = 6,
    [0x18] = 13,  [0x19] = 10,  [0x1A] = 8,   [0x1B] = 12,  [0x1C] = 9,   [0x1D] = 11,  [0x1E] = 27,  [0x1F] = 24,
    [0x20] = 22,  [0x21] = 26,  [0x22] = 23,  [0x23] = 25,  [0x24] = 28,  [0x25] = 38,  [0x26] = 36,  [0x27] = 40,
    [0x28] = 37,  [0x29] = 39,  [0x2A] = 43,  [0x2B] = 51,  [0x2C] = 53,  [0x2D] = 49,  [0x2E] = 50,  [0x2F] = 52,
    [0x30] = 15,  [0x31] = 57,  [0x32] = 41,  [0x33] = 14,  [0x35] = 1,   [0x36] = 126, [0x37] = 125, [0x38] = 42,
    [0x39] = 58,  [0x3A] = 56,  [0x3B] = 29,  [0x3C] = 54,  [0x3D] = 100, [0x3E] = 97,  [0x40] = 187, [0x41] = 83,
    [0x43] = 55,  [0x45] = 78,  [0x47] = 69,  [0x4B] = 98,  [0x4C] = 96,  [0x4E] = 74,  [0x4F] = 188, [0x50] = 189,
    [0x51] = 117, [0x52] = 82,  [0x53] = 79,  [0x54] = 80,  [0x55] = 81,  [0x56] = 75,  [0x57] = 76,  [0x58] = 77,
    [0x59] = 71,  [0x5A] = 190, [0x5B] = 72,  [0x5C] = 73,  [0x5D] = 124, [0x5E] = 89,  [0x5F] = 95,  [0x60] = 63,
    [0x61] = 64,  [0x62] = 65,  [0x63] = 61,  [0x64] = 66,  [0x65] = 67,  [0x66] = 123, [0x67] = 87,  [0x68] = 122,
    [0x69] = 183, [0x6A] = 186, [0x6B] = 184, [0x6D] = 68,  [0x6F] = 88,  [0x71] = 185, [0x72] = 110, [0x73] = 102,
    [0x74] = 104, [0x75] = 111, [0x76] = 62,  [0x77] = 107, [0x78] = 60,  [0x79] = 109, [0x7A] = 59,  [0x7B] = 105,
    [0x7C] = 106, [0x7D] = 108, [0x7E] = 103,
};

// Whether the Command keys go to the other side as Control (the default) or as the Super
// ("Windows") keys. What Command does on a Mac (copy, paste, save, close a tab) is done with
// Control in applications on Linux, so a person's hands keep working as they are used to.
// ISTESSO_COMMAND_KEY=super in the environment sends Super instead.
static BOOL command_is_control = YES;

// Linux's number for the key at this place of the Mac's keyboard, as it is sent.
static unsigned key_to_send(unsigned short key) {
    if (key >= 128) {
        return 0;
    }
    if (command_is_control && key == 0x37) {
        return 29; // left Command as left Control
    }
    if (command_is_control && key == 0x36) {
        return 97; // right Command as right Control
    }
    return linux_key[key];
}

// Which bit of an event's modifier flags says that a modifier key is down, by the key's number
// on macOS. 0 for keys that are not modifiers.
static unsigned long modifier_bit(unsigned short key) {
    switch (key) {
        case 0x38: return NX_DEVICELSHIFTKEYMASK;
        case 0x3C: return NX_DEVICERSHIFTKEYMASK;
        case 0x3B: return NX_DEVICELCTLKEYMASK;
        case 0x3E: return NX_DEVICERCTLKEYMASK;
        case 0x3A: return NX_DEVICELALTKEYMASK;
        case 0x3D: return NX_DEVICERALTKEYMASK;
        case 0x37: return NX_DEVICELCMDKEYMASK;
        case 0x36: return NX_DEVICERCMDKEYMASK;
        default: return 0;
    }
}

// The view the picture is shown in, when input is passed on: it takes the keyboard and the
// pointer and writes what happens to the socket.
@interface InputView : NSView
@end

@implementation InputView {
    BOOL _down[256];     // keys that are down, by Linux's number
    double _scroll[2];   // parts of a pixel not yet passed on
}

- (BOOL)acceptsFirstResponder {
    return YES;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    return YES;
}

- (void)viewDidMoveToWindow {
    [super viewDidMoveToWindow];
    if (self.window == nil) {
        return;
    }
    self.window.acceptsMouseMovedEvents = YES;
    [self addTrackingArea:[[NSTrackingArea alloc]
                              initWithRect:NSZeroRect
                                   options:NSTrackingMouseMoved | NSTrackingActiveAlways | NSTrackingInVisibleRect
                                     owner:self
                                  userInfo:nil]];
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(letGo)
                                               name:NSWindowDidResignKeyNotification
                                             object:self.window];
}

- (void)key:(unsigned)code pressed:(BOOL)pressed {
    if (code == 0 || code >= 256 || _down[code] == pressed) {
        return;
    }
    _down[code] = pressed;
    send_input("{\"kind\":\"key\",\"code\":%u,\"pressed\":%s}", code, pressed ? "true" : "false");
}

// The window is no longer the one that is typed into: no key stays down on the other side.
- (void)letGo {
    for (unsigned code = 1; code < 256; code++) {
        [self key:code pressed:NO];
    }
}

- (void)keyDown:(NSEvent *)event {
    if (event.keyCode >= 128) {
        return;
    }
    unsigned code = key_to_send(event.keyCode);
    if (event.modifierFlags & NSEventModifierFlagCommand) {
        // macOS does not say when a key comes up again while Command is held: press and let go.
        [self key:code pressed:YES];
        [self key:code pressed:NO];
    } else if (!event.isARepeat) {
        // The application on the other side repeats a held key itself.
        [self key:code pressed:YES];
    }
}

- (void)keyUp:(NSEvent *)event {
    [self key:key_to_send(event.keyCode) pressed:NO];
}

// Keys with Command come here first; they all go to the other side.
- (BOOL)performKeyEquivalent:(NSEvent *)event {
    if (self.window.firstResponder != self) {
        return NO;
    }
    [self keyDown:event];
    return YES;
}

- (void)flagsChanged:(NSEvent *)event {
    if (event.keyCode >= 128) {
        return;
    }
    unsigned code = key_to_send(event.keyCode);
    unsigned long bit = modifier_bit(event.keyCode);
    if (bit != 0) {
        [self key:code pressed:(event.modifierFlags & bit) != 0];
    } else if (event.keyCode == 0x39) {
        // Caps Lock reports each switch once.
        [self key:code pressed:YES];
        [self key:code pressed:NO];
    }
}

- (void)pointer:(NSEvent *)event {
    if (picture_width <= 0 || picture_height <= 0) {
        return;
    }
    NSPoint at = [self convertPoint:event.locationInWindow fromView:nil];
    NSSize size = self.bounds.size;
    if (size.width <= 0 || size.height <= 0) {
        return;
    }
    // The view counts from the bottom left in points, the picture from the top left in pixels.
    int x = (int)(at.x * picture_width / size.width);
    int y = (int)((size.height - at.y) * picture_height / size.height);
    x = x < 0 ? 0 : (x >= picture_width ? picture_width - 1 : x);
    y = y < 0 ? 0 : (y >= picture_height ? picture_height - 1 : y);
    send_input("{\"kind\":\"pointer_at\",\"x\":%d,\"y\":%d}", x, y);
}

- (void)button:(unsigned)code pressed:(BOOL)pressed event:(NSEvent *)event {
    [self pointer:event];
    send_input("{\"kind\":\"button\",\"code\":%u,\"pressed\":%s}", code, pressed ? "true" : "false");
}

- (void)mouseMoved:(NSEvent *)event { [self pointer:event]; }
- (void)mouseDragged:(NSEvent *)event { [self pointer:event]; }
- (void)rightMouseDragged:(NSEvent *)event { [self pointer:event]; }
- (void)otherMouseDragged:(NSEvent *)event { [self pointer:event]; }
- (void)mouseDown:(NSEvent *)event {
    [self.window makeFirstResponder:self];
    [self button:0x110 pressed:YES event:event];
}
- (void)mouseUp:(NSEvent *)event { [self button:0x110 pressed:NO event:event]; }
- (void)rightMouseDown:(NSEvent *)event { [self button:0x111 pressed:YES event:event]; }
- (void)rightMouseUp:(NSEvent *)event { [self button:0x111 pressed:NO event:event]; }
- (void)otherMouseDown:(NSEvent *)event { [self button:0x112 pressed:YES event:event]; }
- (void)otherMouseUp:(NSEvent *)event { [self button:0x112 pressed:NO event:event]; }

- (void)scrollWheel:(NSEvent *)event {
    // macOS says how far the content moves; the other side wants how far to scroll. A wheel
    // counts in lines, a trackpad in points.
    double scale = event.hasPreciseScrollingDeltas ? 1.0 : 15.0;
    _scroll[0] -= event.scrollingDeltaX * scale;
    _scroll[1] -= event.scrollingDeltaY * scale;
    int dx = (int)_scroll[0], dy = (int)_scroll[1];
    if (dx != 0 || dy != 0) {
        _scroll[0] -= dx;
        _scroll[1] -= dy;
        send_input("{\"kind\":\"scroll\",\"dx\":%d,\"dy\":%d}", dx, dy);
    }
}

@end

// A window without a title bar does not take the keyboard unless it says that it can.
@interface WholeDisplayWindow : NSWindow
@end

@implementation WholeDisplayWindow
- (BOOL)canBecomeKeyWindow {
    return YES;
}
- (BOOL)canBecomeMainWindow {
    return YES;
}
@end

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
        if (argc < 7 || argc > 10) {
            fprintf(stderr, "usage: gst-viewer X Y WIDTH HEIGHT TITLE PIPELINE [FRAME_LOG [KEY_REQUEST_PORT [INPUT_SOCKET]]]\n");
            return 2;
        }
        if (argc == 10) {
            if (strlen(argv[9]) >= sizeof input_to.sun_path) {
                fprintf(stderr, "error: the path of INPUT_SOCKET is too long\n");
                return 2;
            }
            input_to.sun_family = AF_UNIX;
            strcpy(input_to.sun_path, argv[9]);
            input_socket = socket(AF_UNIX, SOCK_DGRAM, 0);
            if (input_socket < 0 || fcntl(input_socket, F_SETFL, O_NONBLOCK) != 0) {
                fprintf(stderr, "error: cannot open a socket for the input\n");
                return 1;
            }
        }
        if (argc >= 9) {
            int port = atoi(argv[8]);
            if (port < 1 || port > 65535) {
                fprintf(stderr, "error: KEY_REQUEST_PORT has to be a port number\n");
                return 2;
            }
            key_request_port = (guint16)port;
        }
        double x = atof(argv[1]), y = atof(argv[2]);
        double width = atof(argv[3]), height = atof(argv[4]);
        BOOL whole_display = width == 0 && height == 0;
        if (!whole_display && (width < 1 || height < 1)) {
            fprintf(stderr, "error: WIDTH and HEIGHT have to be positive, or both 0 for the whole display\n");
            return 2;
        }

        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSRect display = NSZeroRect;
        if (whole_display) {
            // The display the point lies on. Screens count from the bottom left of the main one.
            double main_height = NSScreen.screens.firstObject.frame.size.height;
            NSPoint point = NSMakePoint(x + 1, main_height - y - 1);
            for (NSScreen *screen in NSScreen.screens) {
                if (NSPointInRect(point, screen.frame)) {
                    display = screen.frame;
                }
            }
            if (NSIsEmptyRect(display)) {
                fprintf(stderr, "error: X Y is on no display\n");
                return 2;
            }
            width = display.size.width;
            height = display.size.height;
            // While this is the active application the menu bar and the Dock stay away, so that
            // the picture has the whole display. They come back when another application is.
            NSApp.presentationOptions = NSApplicationPresentationHideDock | NSApplicationPresentationHideMenuBar;
        }
        NSWindow *window = whole_display
            ? [[WholeDisplayWindow alloc] initWithContentRect:NSMakeRect(0, 0, width, height)
                                                    styleMask:NSWindowStyleMaskBorderless
                                                      backing:NSBackingStoreBuffered
                                                        defer:NO]
            : [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, width, height)
                                          styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable)
                                            backing:NSBackingStoreBuffered
                                              defer:NO];
        window.title = [NSString stringWithUTF8String:argv[5]];
        if (getenv("ISTESSO_ON_TOP") != NULL) {
            window.level = NSFloatingWindowLevel;
        }
        const char *command_key = getenv("ISTESSO_COMMAND_KEY");
        command_is_control = command_key == NULL || strcmp(command_key, "super") != 0;
        NSView *view = input_socket >= 0 ? [[InputView alloc] initWithFrame:NSMakeRect(0, 0, width, height)]
                                         : [[NSView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
        window.contentView = view;
        if (input_socket >= 0) {
            [window makeFirstResponder:view];
        }
        [window orderFrontRegardless];
        // Moved after it exists: at creation macOS pulls a window that is placed on another
        // display than the one in use back onto that one. Coordinates count from the top left
        // of the main display, which is the first screen.
        double mainHeight = NSScreen.screens.firstObject.frame.size.height;
        if (whole_display) {
            [window setFrame:display display:YES];
        } else {
            [window setFrameTopLeftPoint:NSMakePoint(x, mainHeight - y)];
        }

        GError *error = NULL;
        GstElement *pipeline = gst_parse_launch(argv[6], &error);
        if (pipeline == NULL) {
            fprintf(stderr, "error: %s\n", error ? error->message : "cannot build the pipeline");
            return 1;
        }
        gboolean videotoolbox = use_videotoolbox(pipeline, view);
        if ((key_request_port != 0 || input_socket >= 0) && !videotoolbox) {
            fprintf(stderr, "error: key frames are asked for and input is passed on only when VideoToolbox decodes (appsink name=vt)\n");
            return 2;
        }
        if (argc >= 8 && strcmp(argv[7], "-") != 0 && !start_frame_log(pipeline, argv[7], videotoolbox)) {
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
