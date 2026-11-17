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
// The encrypted channel. With ISTESSO_CHANNEL=ADDRESS:PORT in the environment the frames do not
// come from GStreamer at all: this program connects to the paired device that listens there
// (scripts/m1-step3-host.py --to-channel), with this device's identity and list of paired devices,
// through the library istesso-media, and is handed whole encoded frames over the control
// channel's kind of channel (docs/adr/0015-media-transport.md). They go to VideoToolbox as
// above. PIPELINE is then not used ("-" will do), and neither is KEY_REQUEST_PORT: key frames
// are asked for over the same connection, and only when a frame could not be decoded, because
// frames do not get lost on that way. When the connection ends, the program connects again.
//   ISTESSO_CHANNEL_PEER       optional: the name or id of the paired device that has to answer
//   ISTESSO_RECEIVE_REALTIME   set: the thread that receives and decodes is scheduled the way the
//                              threads of the sound system are (off unless set, so that what it
//                              changes can be measured)
// The cursor. A host that was started with --cursor leaves the pointer's cursor out of the
// picture and says over the same connection what it looks like. This program then draws that
// image itself, in a layer above the picture, where the Mac's own pointer is inside the view: the
// cursor follows the hand at once instead of trailing it by the picture's way. The image is
// scaled as the picture is. The Mac's own cursor is not shown over the view while the window is
// the one being typed into (public AppKit: a cursor rectangle with an empty cursor); outside the
// view, and while another window is, it is as always, and the layer is not shown. When the host
// says that the application has hidden the pointer, nothing is drawn; when it has no image of the
// cursor, the Mac's own is shown. Where the pointer is goes to the host as before (INPUT_SOCKET).
//   ISTESSO_CURSOR_AT=X,Y      for checks that must not touch the pointer: draw the cursor at this
//                              place of the view (points from its top left) whatever the pointer
//                              does and whichever window is key
// The frame log gets {"kind":"cursor","t_ns":T,"shape":0|1|2,"width":W,"height":H,
// "hotspot_x":X,"hotspot_y":Y,"scale":S} for every cursor the host tells (0 hidden, 1 no image,
// 2 an image).
// The frame log keeps its lines, and two of them mean something a little different:
//   {"kind":"arrived","rtp":S,"first_ns":T,"last_ns":T,"packets":N,"bytes":N,"key":K,"host_encoded_ns":E}
//                              S is the frame's sequence number, not an RTP timestamp; first_ns
//                              is when the frame's first bytes were there to be read, last_ns
//                              when the whole frame had been read and decrypted; N packets are
//                              the messages of the encrypted channel it came in; E is when the
//                              host's encoder had finished it, on the HOST's clock
//   {"kind":"key_request","t_ns":T,"sent":1}   asked for over the connection
//   {"kind":"stream","t_ns":T,"event":"connected"|"ended"|"unreachable"}   the connection's life
// Both ways write {"kind":"to_decoder","pts":P,"t_ns":T} when an access unit is handed to
// VideoToolbox.
//
// Only for measurements and first trials: the own client of step 4 replaces it. GStreamer is
// linked dynamically, istesso-media statically.

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
#include <pthread.h>
#include <time.h>
#include "istesso_media.h"

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

static gboolean vt_start_session(gboolean all_colour, const char *stream_colours);

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
    const char *colours = gst_structure_get_string(stream, "colorimetry");
    GstVideoColorimetry described;
    vt_srgb = colours != NULL && gst_video_colorimetry_from_string(&described, colours) && described.transfer == GST_VIDEO_TRANSFER_SRGB;
    const char *chroma = gst_structure_get_string(stream, "chroma-format");
    return vt_start_session(chroma != NULL && strcmp(chroma, "4:4:4") == 0, colours);
}

// Starts VideoToolbox for the stream that `vt_format` describes. `stream_colours` is what the
// stream says about its colours, for the log.
static gboolean vt_start_session(gboolean all_colour, const char *stream_colours) {
    NSDictionary *decoder = @{(__bridge NSString *)kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder : @YES};
    // Pictures the window system can show as they are, on a surface it shares with the decoder,
    // with as much colour as the stream has: asked for 4:2:0, a 4:4:4 stream would lose it here.
    NSDictionary *pictures = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey :
            @(all_colour ? kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange : kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{}
    };
    VTDecompressionOutputCallbackRecord callback = {on_picture, NULL};
    OSStatus status = VTDecompressionSessionCreate(kCFAllocatorDefault, vt_format, (__bridge CFDictionaryRef)decoder,
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
        fprintf(frame_log, "{\"kind\":\"decoder\",\"hardware\":%d,\"pictures\":\"%s\",\"stream_colours\":\"%s\"}\n",
                in_hardware ? 1 : 0, all_colour ? "444v" : "420v", stream_colours != NULL ? stream_colours : "-");
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
        if (frame_log != NULL) {
            fprintf(frame_log, "{\"kind\":\"to_decoder\",\"pts\":%llu,\"t_ns\":%llu}\n",
                    (unsigned long long)GST_BUFFER_PTS(buffer), (unsigned long long)now_ns());
        }
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

// ---- The cursor, drawn here ----
//
// What the host said last, and where the pointer is. Only touched on the main thread.

static CALayer *cursor_layer = nil;
// 0 hidden, 1 no image (the Mac's own cursor), 2 an image. Until the host has said anything the
// Mac's own cursor is shown, as it is with a host that draws the cursor into the picture.
static uint32_t cursor_shape = 1;
static CGSize cursor_pixels;   // size of the image in its own pixels
static CGPoint cursor_hot;     // the pixel of it that lies on the pointer's position
static CGFloat cursor_scale = 1;  // pixels of the image to one pixel of the picture
// ISTESSO_CURSOR_AT: a place of the view to draw the cursor at, for checks.
static BOOL cursor_pinned = NO;
static NSPoint cursor_pin;
// The view that shows the picture, when it is one that knows about the cursor.
static NSView *cursor_view = nil;

@interface NSView (IstessoCursor)
- (void)cursorChanged;
@end

// Takes over what the host said the cursor is. `image` may be NULL; it is released here.
static void show_cursor(uint32_t shape, CGImageRef image, CGSize pixels, CGPoint hot, CGFloat scale) {
    cursor_shape = shape;
    cursor_pixels = pixels;
    cursor_hot = hot;
    cursor_scale = scale > 0 ? scale : 1;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    cursor_layer.contents = shape == 2 ? (__bridge id)image : nil;
    [CATransaction commit];
    if (image != NULL) {
        CGImageRelease(image);
    }
    if ([cursor_view respondsToSelector:@selector(cursorChanged)]) {
        [cursor_view cursorChanged];
    }
}

// ---- Frames from the encrypted channel ----
//
// One thread connects to the host, is handed whole frames by the library, and decodes each one
// there and then, as the streaming thread of the GStreamer source does on the other way.

// Where the host listens: ISTESSO_CHANNEL. NULL when the frames come from GStreamer.
static const char *channel_address = NULL;
// The parameter sets VideoToolbox was last set up with: VPS, SPS, PPS.
static uint8_t *channel_sets[3];
static size_t channel_set_sizes[3];
static IstessoMediaStreamInfo channel_info;
static guint64 channel_asked_ns = 0;

// Where the next start code (00 00 01) begins in `data`, from `from` on; `size` when there is none.
static size_t next_start_code(const uint8_t *data, size_t from, size_t size) {
    size_t i = from;
    while (i + 3 <= size) {
        if (data[i + 2] > 1) {
            i += 3;
        } else if (data[i + 2] == 1) {
            if (data[i] == 0 && data[i + 1] == 0) {
                return i;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    return size;
}

typedef struct {
    const uint8_t *bytes;
    size_t size;
    int type;
} NalUnit;

// Splits an access unit whose NAL units stand behind start codes. Returns how many there are, at
// most `room`.
static size_t split_access_unit(const uint8_t *data, size_t size, NalUnit *units, size_t room) {
    size_t count = 0;
    size_t at = next_start_code(data, 0, size);
    while (at < size && count < room) {
        size_t begin = at + 3;
        size_t next = next_start_code(data, begin, size);
        size_t end = next;
        // Zero bytes in front of the next start code belong to it, not to this unit.
        while (end > begin && data[end - 1] == 0) {
            end -= 1;
        }
        if (end > begin + 1) {
            units[count].bytes = data + begin;
            units[count].size = end - begin;
            units[count].type = (data[begin] >> 1) & 0x3f;
            count += 1;
        }
        at = next;
    }
    return count;
}

// Sets VideoToolbox up for a stream with these parameter sets, unless it is set up for them already.
static gboolean channel_configure(const NalUnit *sets[3]) {
    gboolean same = vt_session != NULL;
    for (int i = 0; i < 3 && same; i++) {
        same = channel_set_sizes[i] == sets[i]->size && memcmp(channel_sets[i], sets[i]->bytes, sets[i]->size) == 0;
    }
    if (same) {
        return TRUE;
    }
    if (vt_session != NULL) {
        VTDecompressionSessionInvalidate(vt_session);
        CFRelease(vt_session);
        vt_session = NULL;
    }
    if (vt_format != NULL) {
        CFRelease(vt_format);
        vt_format = NULL;
    }
    const uint8_t *pointers[3];
    for (int i = 0; i < 3; i++) {
        free(channel_sets[i]);
        channel_sets[i] = malloc(sets[i]->size);
        memcpy(channel_sets[i], sets[i]->bytes, sets[i]->size);
        channel_set_sizes[i] = sets[i]->size;
        pointers[i] = channel_sets[i];
    }
    // NAL units in a sample stand behind four bytes of length.
    OSStatus status = CMVideoFormatDescriptionCreateFromHEVCParameterSets(kCFAllocatorDefault, 3, pointers, channel_set_sizes, 4, NULL,
                                                                          &vt_format);
    if (status != noErr) {
        fprintf(stderr, "error: VideoToolbox does not take the stream's parameter sets (%d)\n", (int)status);
        return FALSE;
    }
    CMVideoDimensions size = CMVideoFormatDescriptionGetDimensions(vt_format);
    picture_width = size.width;
    picture_height = size.height;
    vt_srgb = channel_info.colour == 1;
    return vt_start_session(channel_info.chroma == 444, vt_srgb ? "srgb" : "-");
}

static void channel_ask_for_key_frame(IstessoMediaClient *client, guint64 t) {
    // Once, and again every half second for as long as none has come.
    if (channel_asked_ns != 0 && t - channel_asked_ns < 500 * 1000 * 1000ULL) {
        return;
    }
    channel_asked_ns = t;
    istesso_media_client_ask_key_frame(client);
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"key_request\",\"t_ns\":%llu,\"sent\":1}\n", (unsigned long long)t);
    }
}

// One frame has arrived, whole: decode it now, on the thread that received it.
static void on_channel_frame(IstessoMediaClient *client, const IstessoMediaFrame *frame) {
    if (frame_log != NULL) {
        fprintf(frame_log,
                "{\"kind\":\"arrived\",\"rtp\":%llu,\"first_ns\":%llu,\"last_ns\":%llu,\"packets\":%u,\"bytes\":%zu,\"key\":%u,"
                "\"host_encoded_ns\":%llu}\n",
                (unsigned long long)frame->seq, (unsigned long long)frame->first_ns, (unsigned long long)frame->whole_ns,
                frame->messages, frame->len, frame->key_frame, (unsigned long long)frame->encoded_ns);
    }
    NalUnit units[128];
    size_t count = split_access_unit(frame->data, frame->len, units, sizeof units / sizeof units[0]);
    // The parameter sets come with every key frame; everything else but the delimiter is the picture.
    const NalUnit *sets[3] = {NULL, NULL, NULL};
    size_t total = 0;
    for (size_t i = 0; i < count; i++) {
        if (units[i].type >= 32 && units[i].type <= 34) {
            sets[units[i].type - 32] = &units[i];
        } else if (units[i].type != 35) {
            total += 4 + units[i].size;
        }
    }
    if (frame->key_frame && sets[0] != NULL && sets[1] != NULL && sets[2] != NULL && !channel_configure(sets)) {
        exit(1);
    }
    if (frame->discontinuity && !frame->key_frame) {
        vt_needs_key_frame = TRUE;  // The host left frames out and went on without a key frame.
    }
    if (vt_session == NULL || total == 0 || (vt_needs_key_frame && !frame->key_frame)) {
        vt_needs_key_frame = TRUE;
        channel_ask_for_key_frame(client, now_ns());
        return;
    }
    CMBlockBufferRef block = NULL;
    CMSampleBufferRef sample = NULL;
    char *bytes = NULL;
    OSStatus status = CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, NULL, total, kCFAllocatorDefault, NULL, 0, total,
                                                         kCMBlockBufferAssureMemoryNowFlag, &block);
    if (status == noErr) {
        status = CMBlockBufferGetDataPointer(block, 0, NULL, NULL, &bytes);
    }
    if (status == noErr) {
        for (size_t i = 0; i < count; i++) {
            if (units[i].type >= 32 && units[i].type <= 35) {
                continue;
            }
            uint32_t length = (uint32_t)units[i].size;
            const uint8_t prefix[4] = {(uint8_t)(length >> 24), (uint8_t)(length >> 16), (uint8_t)(length >> 8), (uint8_t)length};
            memcpy(bytes, prefix, 4);
            memcpy(bytes + 4, units[i].bytes, units[i].size);
            bytes += 4 + units[i].size;
        }
        status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, vt_format, 1, 0, NULL, 1, &total, &sample);
    }
    if (status == noErr) {
        vt_decoding_key_frame = frame->key_frame != 0;
        if (frame_log != NULL) {
            fprintf(frame_log, "{\"kind\":\"to_decoder\",\"pts\":%llu,\"t_ns\":%llu}\n", (unsigned long long)frame->pts_ns,
                    (unsigned long long)now_ns());
        }
        // No flags: decoded here and now, and `on_picture` has run by the time this returns.
        status = VTDecompressionSessionDecodeFrame(vt_session, sample, 0, (void *)(uintptr_t)frame->pts_ns, NULL);
    }
    vt_needs_key_frame = status != noErr;
    if (vt_needs_key_frame) {
        channel_ask_for_key_frame(client, now_ns());
    } else if (frame->key_frame) {
        channel_asked_ns = 0;
    }
    if (sample != NULL) {
        CFRelease(sample);
    }
    if (block != NULL) {
        CFRelease(block);
    }
}

// The host has said what the cursor is. The library has checked it; it is checked once more
// before a picture is made of it, and handed to the main thread.
static void on_channel_cursor(const IstessoMediaCursor *cursor) {
    if (frame_log != NULL) {
        fprintf(frame_log,
                "{\"kind\":\"cursor\",\"t_ns\":%llu,\"shape\":%u,\"width\":%u,\"height\":%u,\"hotspot_x\":%u,\"hotspot_y\":%u,"
                "\"scale\":%u}\n",
                (unsigned long long)now_ns(), cursor->shape, cursor->width, cursor->height, cursor->hotspot_x, cursor->hotspot_y,
                cursor->scale);
    }
    CGImageRef image = NULL;
    uint32_t shape = cursor->shape;
    if (shape == 2) {
        gboolean fits = cursor->width >= 1 && cursor->width <= 256 && cursor->height >= 1 && cursor->height <= 256
            && cursor->hotspot_x < cursor->width && cursor->hotspot_y < cursor->height && cursor->scale >= 1 && cursor->pixels != NULL
            && cursor->pixels_len == (size_t)cursor->width * cursor->height * 4;
        if (fits) {
            // Copied: the library keeps the bytes only until the cursor changes again.
            CFDataRef bytes = CFDataCreate(kCFAllocatorDefault, cursor->pixels, (CFIndex)cursor->pixels_len);
            CGDataProviderRef provider = CGDataProviderCreateWithCFData(bytes);
            CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            image = CGImageCreate(cursor->width, cursor->height, 8, 32, (size_t)cursor->width * 4, space,
                                  (CGBitmapInfo)kCGImageAlphaPremultipliedLast, provider, NULL, false, kCGRenderingIntentDefault);
            CGColorSpaceRelease(space);
            CGDataProviderRelease(provider);
            CFRelease(bytes);
        }
        if (image == NULL) {
            shape = 1;  // No picture of it after all: the Mac's own cursor.
        }
    } else if (shape > 2) {
        shape = 1;
    }
    CGSize pixels = CGSizeMake(cursor->width, cursor->height);
    CGPoint hot = CGPointMake(cursor->hotspot_x, cursor->hotspot_y);
    CGFloat scale = cursor->scale;
    dispatch_async(dispatch_get_main_queue(), ^{
        show_cursor(shape, image, pixels, hot, scale);
    });
}

static void log_stream(const char *event) {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"stream\",\"t_ns\":%llu,\"event\":\"%s\"}\n", (unsigned long long)now_ns(), event);
    }
}

// Connects, receives and decodes, and connects again when the connection has ended.
static void *channel_thread(void *unused) {
    if (getenv("ISTESSO_RECEIVE_REALTIME") != NULL) {
        // Work once per frame (85 a second), a few milliseconds of it, to be done well within the frame.
        int agreed = istesso_media_thread_real_time(11764706, 3000000, 6000000);
        if (frame_log != NULL) {
            fprintf(frame_log, "{\"kind\":\"scheduling\",\"real_time\":%d}\n", agreed);
        }
        if (!agreed) {
            fprintf(stderr, "note: the receiving thread was not given real-time scheduling\n");
        }
    }
    gboolean reachable = TRUE;
    for (;;) {
        IstessoMediaClient *client = istesso_media_client_connect(NULL, channel_address, getenv("ISTESSO_CHANNEL_PEER"), 3000);
        if (client == NULL) {
            if (reachable) {
                // Said once for each time the host goes away; the sentence holds no address.
                fprintf(stderr, "note: no picture from the host: %s; trying again\n", istesso_media_last_error());
                log_stream("unreachable");
                reachable = FALSE;
            }
            usleep(500 * 1000);
            continue;
        }
        reachable = TRUE;
        if (istesso_media_client_info(client, &channel_info) != 0 || channel_info.codec != 1) {
            fprintf(stderr, "error: the host sends its picture in a form this program cannot decode\n");
            exit(1);
        }
        log_stream("connected");
        vt_needs_key_frame = TRUE;
        channel_asked_ns = 0;
        for (;;) {
            IstessoMediaFrame frame;
            int32_t got = istesso_media_client_next(client, 250, &frame);
            if (got < 0) {
                break;
            }
            if (got == 1) {
                on_channel_frame(client, &frame);
            } else if (got == 2) {
                IstessoMediaCursor cursor;
                if (istesso_media_client_cursor(client, &cursor) == 1) {
                    on_channel_cursor(&cursor);
                }
            }
        }
        fprintf(stderr, "note: the picture's connection is over: %s; connecting again\n", istesso_media_last_error());
        log_stream("ended");
        istesso_media_client_close(client);
        usleep(200 * 1000);
    }
    return NULL;
}

// Shows the frames of the encrypted channel in the layer of `view`.
static gboolean use_channel(NSView *view) {
    view.wantsLayer = YES;
    vt_layer = view.layer;
    vt_layer.contentsGravity = kCAGravityResize;
    vt_layer.backgroundColor = CGColorGetConstantColor(kCGColorBlack);
    // The cursor lies above the picture, and is only shown once the host has said what it is.
    cursor_layer = [CALayer layer];
    cursor_layer.zPosition = 10;
    cursor_layer.hidden = YES;
    cursor_layer.contentsGravity = kCAGravityResize;
    [vt_layer addSublayer:cursor_layer];
    cursor_view = view;
    const char *pin = getenv("ISTESSO_CURSOR_AT");
    double pin_x = 0, pin_y = 0;
    if (pin != NULL && sscanf(pin, "%lf,%lf", &pin_x, &pin_y) == 2) {
        cursor_pinned = YES;
        cursor_pin = NSMakePoint(pin_x, pin_y);
    }
    pthread_t thread;
    return pthread_create(&thread, NULL, channel_thread, NULL) == 0;
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

// Which keys go to the other side for what happens on the keyboard: keys.h, tested by keys-test.c.
#include "keys.h"

// Whether the Command keys go to the other side as Control (the default) or as the Super
// ("Windows") keys. ISTESSO_COMMAND_KEY=super in the environment sends Super.
static BOOL command_is_control = YES;

// The view the picture is shown in, when input is passed on: it takes the keyboard and the
// pointer and writes what happens to the socket.
@interface InputView : NSView
@end

@implementation InputView {
    Keys _keys;          // keys that are down on the other side
    double _scroll[2];   // parts of a pixel not yet passed on
    BOOL _inside;        // the pointer is over the view
    NSPoint _at;         // where, in the view's own coordinates (from its bottom left)
}

// A cursor that shows nothing: over the view the cursor is the one drawn into the layer.
static NSCursor *empty_cursor(void) {
    static NSCursor *cursor = nil;
    if (cursor == nil) {
        cursor = [[NSCursor alloc] initWithImage:[[NSImage alloc] initWithSize:NSMakeSize(16, 16)] hotSpot:NSZeroPoint];
    }
    return cursor;
}

// The Mac's own cursor is left as it is unless the host has an image of the cursor, or says
// that the application has hidden it. Cursor rectangles only work in the key window, which is
// what is wanted: over a window that is not being used the Mac's cursor stays.
- (void)resetCursorRects {
    if (cursor_layer != nil && cursor_shape != 1) {
        [self addCursorRect:self.bounds cursor:empty_cursor()];
    } else {
        [super resetCursorRects];
    }
}

// Puts the cursor's layer where the pointer is, at the picture's scale, or takes it away.
- (void)placeCursor {
    if (cursor_layer == nil) {
        return;
    }
    BOOL here = cursor_pinned || (_inside && self.window.isKeyWindow);
    BOOL show = here && cursor_shape == 2 && picture_width > 0 && self.bounds.size.width > 0;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (show) {
        NSPoint at = cursor_pinned ? NSMakePoint(cursor_pin.x, self.bounds.size.height - cursor_pin.y) : _at;
        // Points of the view to one pixel of the cursor's image.
        CGFloat each = self.bounds.size.width / picture_width / cursor_scale;
        CGFloat width = cursor_pixels.width * each, height = cursor_pixels.height * each;
        // The hot spot lies on the pointer; the view counts upwards, the image downwards.
        cursor_layer.frame = CGRectMake(at.x - cursor_hot.x * each, at.y + cursor_hot.y * each - height, width, height);
    }
    cursor_layer.hidden = !show;
    [CATransaction commit];
}

// The host has said that the cursor is something else.
- (void)cursorChanged {
    [self.window invalidateCursorRectsForView:self];
    [self placeCursor];
}

- (void)pointerIsAt:(NSEvent *)event inside:(BOOL)inside {
    _at = [self convertPoint:event.locationInWindow fromView:nil];
    _inside = inside;
    [self placeCursor];
}

- (void)mouseEntered:(NSEvent *)event {
    [self pointerIsAt:event inside:YES];
}

- (void)mouseExited:(NSEvent *)event {
    [self pointerIsAt:event inside:NO];
}

- (void)windowIsKeyOrNot {
    [self placeCursor];
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
                                   options:NSTrackingMouseMoved | NSTrackingMouseEnteredAndExited | NSTrackingActiveAlways
                                           | NSTrackingInVisibleRect
                                     owner:self
                                  userInfo:nil]];
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(letGo)
                                               name:NSWindowDidResignKeyNotification
                                             object:self.window];
    for (NSNotificationName name in @[NSWindowDidResignKeyNotification, NSWindowDidBecomeKeyNotification]) {
        [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(windowIsKeyOrNot) name:name object:self.window];
    }
}

- (void)send:(const KeyChange *)changes count:(size_t)count {
    for (size_t i = 0; i < count; i++) {
        send_input("{\"kind\":\"key\",\"code\":%u,\"pressed\":%s}", changes[i].code, changes[i].pressed ? "true" : "false");
    }
}

// The window is no longer the one that is typed into: no key stays down on the other side.
- (void)letGo {
    KeyChange changes[256];
    [self send:changes count:keys_let_go(&_keys, changes)];
}

- (void)keyDown:(NSEvent *)event {
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    [self send:changes count:keys_key_down(&_keys, event.keyCode, event.modifierFlags, event.isARepeat, changes)];
}

- (void)keyUp:(NSEvent *)event {
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    [self send:changes count:keys_key_up(&_keys, event.keyCode, changes)];
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
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    [self send:changes count:keys_flags_changed(&_keys, event.keyCode, event.modifierFlags, changes)];
}

- (void)pointer:(NSEvent *)event {
    // The cursor is drawn here first: it does not wait for anything.
    [self pointerIsAt:event inside:NSPointInRect([self convertPoint:event.locationInWindow fromView:nil], self.bounds)];
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
        channel_address = getenv("ISTESSO_CHANNEL");
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
        if (argc >= 9 && channel_address == NULL) {
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
        // The view that follows the pointer: to pass it on, and, with the channel, to draw the cursor.
        NSView *view = input_socket >= 0 || channel_address != NULL ? [[InputView alloc] initWithFrame:NSMakeRect(0, 0, width, height)]
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

        if (channel_address != NULL) {
            // The frames come from the encrypted channel: no pipeline, and the log is written as
            // with VideoToolbox behind a pipeline.
            if (argc >= 8 && strcmp(argv[7], "-") != 0 && !start_frame_log(NULL, argv[7], TRUE)) {
                return 1;
            }
            if (!use_channel(view)) {
                fprintf(stderr, "error: cannot start the thread that receives\n");
                return 1;
            }
            [NSApp run];
            return 0;
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
