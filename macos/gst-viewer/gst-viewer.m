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
// For the everyday trial only (branch trial-daily, docs/trial-daily.md; never in a build that is
// measured):
//   ISTESSO_DOCK_TILE=none     the program gets no tile in the Dock and no menu bar of its own
//                              (an accessory application): the trial's launcher has the one tile
//   ISTESSO_NO_WINDOW          for checks that must put nothing on the screen: the window is made
//                              but never shown, the program never becomes the active one and has
//                              no Dock tile; the picture is received, decoded and handed to the
//                              window's layer as always, so that the frame log says what came
//   ISTESSO_END_WITH_WINDOW    the program ends when its window is closed (otherwise it stays,
//                              with nothing on the screen)
//   ISTESSO_PID_FILE=PATH      write this program's process number to PATH once it runs: for a
//                              program started through `open`, which does not say it
//   ISTESSO_PRETEND_RESIZE=WxH for checks with ISTESSO_DAILY_RECONNECT: four seconds after the start,
//                              as if the person had resized the window by hand to WxH points
//   ISTESSO_TITLE_FILE=PATH    the signal USR1 sets the window's title to the first line of PATH
//   ISTESSO_CLIPBOARD=1        the everyday trial's copy and paste of text (ADR 0022): Command-C,
//                              Command-X and Command-V in the window copy from, cut from and paste
//                              into the host's application, when the relay says the host takes them
//                              (a window of the application wants attention, and back); when the
//                              second line is "attention", the Dock tile asks for attention once
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
// frames do not get lost on that way.
// When the connection is cut (it fails, or the host says nothing for a second), the program
// connects again by itself: at once, then four times a second. It does not when the host said
// why the stream ends or refused this device (the session is over, the picture is for another
// device or put aside, the pairing or the right to look is gone): then the window says so and
// stays as it is. While the connection is being made again the window keeps the last picture.
// After half a second, or at once when keys or buttons were pressed in the second before, the
// picture is dimmed and a line says that what is typed does not arrive: from then until the
// picture is back no key or button press is passed on (what was held is let go on the other
// side when the line comes up). Nothing typed meanwhile is sent later. When the picture is
// back the modifiers that are held are pressed again, and a line stays for three seconds if
// anything was not passed on.
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
//   ISTESSO_PRETEND_TYPING     for checks that must not touch the keyboard: behave as if a key
//                              were pressed ten times a second, as far as the line about a cut
//                              connection goes (it comes at once, and counts what was not passed
//                              on). Nothing is sent anywhere for it
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
//   {"kind":"stream","t_ns":T,"event":E,...}   the connection's life. E is "connected" (with
//                              "again":0|1 and, after a cut, "silent_ms": for how long the host had
//                              not been heard), "ended" (cut; it is being made again), "back" (the
//                              first picture after a cut is decoded; "gap_ms" since the cut was
//                              noticed), "unreachable" (no host there yet), "over" (not tried
//                              again; "why" is one of ISTESSO_MEDIA_OVER_*)
//   {"kind":"reconnections","t_ns":T,"at":"start"|"cut"|"end","picture":N}   how often the
//                              picture's connection was cut so far: written when the log begins
//                              (0), at every cut, and when the program is asked to stop. The
//                              last such line has the count. A log without any was written
//                              without the channel, where nothing is counted: that is not 0.
//   {"kind":"held_back","t_ns":T,"marked":0|1,"dropped":N,"in_doubt":M}   when the picture is back
//                              or over: whether the line was shown, how many presses were not
//                              passed on, and how many were passed on in the second before the cut
// Both ways write {"kind":"to_decoder","pts":P,"t_ns":T} when an access unit is handed to
// VideoToolbox.
//
// Only for measurements and first trials: the own client of step 4 replaces it. GStreamer is
// linked dynamically, istesso-media statically.

#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#import <VideoToolbox/VideoToolbox.h>
#import <IOSurface/IOSurface.h>
#import <Carbon/Carbon.h>  // EnableSecureEventInput (the trial's password fields)
#include <gio/gio.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/stat.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#include <gst/app/gstappsink.h>
#include <gst/gst.h>
#include <gst/net/gstnetaddressmeta.h>
#include <gst/video/video.h>
#include <gst/video/videooverlay.h>
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <limits.h>
#include <stdatomic.h>
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
static BOOL daily_reconnect = NO;
// The synchronous decoder callback received a surface on the current daily connection.
static BOOL daily_picture_drawn = NO;
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

static void on_daily_picture(void *context, void *frame, OSStatus status, VTDecodeInfoFlags flags,
                             CVImageBufferRef picture, CMTime time, CMTime duration) {
    on_picture(context, frame, status, flags, picture, time, duration);
    if (!daily_picture_drawn && status == noErr && picture != NULL && vt_layer.contents != nil) {
        daily_picture_drawn = YES;
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
    VTDecompressionOutputCallbackRecord callback = {daily_reconnect ? on_daily_picture : on_picture, NULL};
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

// ---- When the connection is cut ----
//
// What the window shows while the picture is not the host's picture of this moment, and what is
// passed on of the keyboard and the pointer meanwhile. Only touched on the main thread.

static CALayer *dim_layer = nil;
static CALayer *note_box = nil;
static CATextLayer *note_layer = nil;
// The line is up: no key or button press is passed on.
static BOOL picture_stale = NO;
// The connection was cut and the picture is not back yet.
static BOOL cut_pending = NO;
static BOOL waiting_for_first_picture = NO;
static guint64 cut_noticed_ns = 0;
static unsigned long cut_generation = 0, note_generation = 0;
// Presses that were not passed on since the cut, and presses that were passed on in the second
// before it was noticed: whether those arrived, nobody knows.
static unsigned long held_back_count = 0, in_doubt_count = 0;
// When keys and buttons were last pressed and passed on.
static guint64 press_times[32];
static unsigned long press_count = 0;

@interface NSView (IstessoHeldBack)
- (void)holdBack;
- (void)fallInLine;
- (void)windowIsKeyOrNot;
@end

static void note_press(void) {
    press_times[press_count++ % 32] = now_ns();
}

static unsigned long presses_since(guint64 t) {
    unsigned long count = 0, kept = press_count < 32 ? press_count : 32;
    for (unsigned long i = 0; i < kept; i++) {
        count += press_times[i] >= t ? 1 : 0;
    }
    return count;
}

// Shows one line in the middle of the picture, or takes it away (nil).
static void show_note(NSString *text) {
    note_generation++;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (text == nil || cursor_view == nil || note_box == nil) {
        note_box.hidden = YES;
    } else {
        NSFont *font = [NSFont systemFontOfSize:14 weight:NSFontWeightMedium];
        CGSize room = cursor_view.bounds.size;
        CGFloat most = MIN(room.width - 40, 560);
        NSRect fit = [text boundingRectWithSize:NSMakeSize(most - 28, 400)
                                        options:NSStringDrawingUsesLineFragmentOrigin
                                     attributes:@{NSFontAttributeName : font}];
        CGFloat width = ceil(fit.size.width) + 28, height = ceil(fit.size.height) + 20;
        note_box.frame = CGRectMake(floor((room.width - width) / 2), floor((room.height - height) / 2), width, height);
        note_layer.frame = CGRectMake(14, 10, width - 28, height - 20);
        note_layer.font = (__bridge CFTypeRef)font;
        note_layer.fontSize = 14;
        note_layer.contentsScale = cursor_view.window != nil ? cursor_view.window.backingScaleFactor : 2;
        note_layer.string = text;
        note_box.hidden = NO;
    }
    [CATransaction commit];
}

static void set_dim(BOOL on) {
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (cursor_view != nil) {
        dim_layer.frame = cursor_view.bounds;
    }
    dim_layer.hidden = !on;
    [CATransaction commit];
}

static void log_held_back(BOOL marked) {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"held_back\",\"t_ns\":%llu,\"marked\":%d,\"dropped\":%lu,\"in_doubt\":%lu}\n",
                (unsigned long long)now_ns(), marked ? 1 : 0, held_back_count, in_doubt_count);
    }
}

// The line comes up: from here on nothing is pressed on the other side, and what is held there
// is let go, so that a key that is held does not go on typing into a picture nobody sees.
static void mark_the_picture(NSString *text) {
    BOOL was = picture_stale;
    picture_stale = YES;
    set_dim(YES);
    show_note(text);
    if (!was && [cursor_view respondsToSelector:@selector(holdBack)]) {
        [cursor_view holdBack];
    }
    if ([cursor_view respondsToSelector:@selector(cursorChanged)]) {
        [cursor_view cursorChanged];  // The Mac's own cursor again.
    }
}

// The connection was cut and is being made again.
static void picture_was_cut(guint64 t) {
    if (cut_pending || waiting_for_first_picture) {
        return;
    }
    cut_pending = YES;
    cut_noticed_ns = t;
    unsigned long generation = ++cut_generation;
    held_back_count = 0;
    in_doubt_count = presses_since(t > 1000000000ULL ? t - 1000000000ULL : 0);
    NSString *line = daily_reconnect ? @"つなぎ直しています…" : @"Reconnecting. What you type now does not arrive.";
    if (daily_reconnect) {
        // Stop input immediately, including during the short delay before the note appears.
        picture_stale = YES;
        [cursor_view holdBack];
        [cursor_view cursorChanged];
    }
    if (in_doubt_count > 0) {
        // Somebody is typing: said at once.
        mark_the_picture(line);
        return;
    }
    // Most cuts are over before anybody could read a line; one that is not is said.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        if (cut_pending && generation == cut_generation) {
            mark_the_picture(line);
        }
    });
}

// The first picture after a cut is on the screen.
static void picture_is_back(void) {
    BOOL first = waiting_for_first_picture;
    if (!cut_pending && !first) {
        return;
    }
    waiting_for_first_picture = NO;
    cut_pending = NO;
    cut_generation++;
    BOOL was_marked = picture_stale;
    picture_stale = NO;
    set_dim(NO);
    log_held_back(was_marked);
    if (was_marked && [cursor_view respondsToSelector:@selector(fallInLine)]) {
        [cursor_view fallInLine];
    }
    if ([cursor_view respondsToSelector:@selector(cursorChanged)]) {
        [cursor_view cursorChanged];
    }
    if (!was_marked || held_back_count + in_doubt_count == 0) {
        show_note(nil);
        return;
    }
    // What was typed is gone, and the person is told so for long enough to read it.
    NSString *line;
    if (first) {
        line = [NSString stringWithFormat:@"最初の絵が映る前の入力 %lu 回は届いていません。", held_back_count];
    } else if (held_back_count > 0 && in_doubt_count > 0) {
        line = [NSString stringWithFormat:daily_reconnect
                ? @"切れているあいだの入力 %lu 回は届いていません。切れる直前の %lu 回は届いたか分かりません。"
                : @"The connection was cut. %lu input(s) during the cut did not arrive, and %lu just before it may not have.",
                held_back_count, in_doubt_count];
    } else if (held_back_count > 0) {
        line = [NSString stringWithFormat:daily_reconnect ? @"切れているあいだの入力 %lu 回は届いていません。"
                : @"The connection was cut. %lu input(s) during the cut did not arrive.", held_back_count];
    } else {
        line = [NSString stringWithFormat:daily_reconnect ? @"切れる直前の入力 %lu 回は届いたか分かりません。"
                : @"The connection was cut. %lu input(s) just before it may not have arrived.", in_doubt_count];
    }
    show_note(line);
    unsigned long generation = note_generation;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (generation == note_generation) {
            show_note(nil);
        }
    });
}

// It is over, and the host said why or refused: nothing is tried again. The window stays, with
// the last picture dimmed and the reason, until the person closes it.
static void picture_is_over(int32_t why) {
    waiting_for_first_picture = NO;
    cut_pending = NO;
    cut_generation++;
    NSString *line;
    switch (why) {
        case ISTESSO_MEDIA_OVER_ENDED: line = daily_reconnect ? @"配信が終わりました。" : @"The session has ended."; break;
        case ISTESSO_MEDIA_OVER_NOT_SHOWN: line = daily_reconnect ? @"ほかの端末に映しています。" : @"The host shows this to another device now."; break;
        case ISTESSO_MEDIA_OVER_SET_ASIDE: line = daily_reconnect ? @"このセッションはいったん切り離されました。" : @"The host has put this session aside."; break;
        case ISTESSO_MEDIA_OVER_NOT_PAIRED: line = daily_reconnect ? @"ホストとのペアリングがありません。" : @"This device is not paired with the host any more."; break;
        case ISTESSO_MEDIA_OVER_FORBIDDEN: line = daily_reconnect ? @"この端末には閲覧の許可がありません。" : @"This device may not look at the host."; break;
        case ISTESSO_MEDIA_OVER_UNSUPPORTED: line = daily_reconnect ? @"この映像の形式には対応していません。" : @"The host sends its picture in a form this program cannot show."; break;
        default: line = daily_reconnect ? @"ホストはこの端末に映像を送っていません。" : @"The host does not show its picture to this device."; break;
    }
    mark_the_picture(line);
    log_held_back(YES);
}

// ---- How often the connections were cut, where a person sees it ----
//
// The window's title says it for as long as the program runs: "... picture 0, input 0". The
// picture's count is this program's own. The input's is the relay's, which writes it into a
// small file next to its socket (SOCKET.state: its process id and the count); "input not
// counted" when there is no relay that counts. Without the channel the title is left alone.

static atomic_ulong picture_cuts;
// The input's count as the relay says it; -1 when no relay that counts is running.
static long input_cuts = -1;
static BOOL input_reconnecting = NO;
static NSWindow *counted_window = nil;
static NSString *counted_title = nil;
// A short line about copy and paste, shown in the title for two seconds (ADR 0022).
static NSString *clipboard_shown = nil;

static void log_reconnections(const char *at) {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"reconnections\",\"t_ns\":%llu,\"at\":\"%s\",\"picture\":%lu}\n",
                (unsigned long long)now_ns(), at, atomic_load(&picture_cuts));
    }
}

// On the main thread.
static void show_counts(void) {
    if (counted_window == nil) {
        return;
    }
    if (daily_reconnect) {
        NSString *input = input_cuts < 0 ? @"入力は未集計" : [NSString stringWithFormat:@"入力 %ld 回", input_cuts];
        if (input_reconnecting) {
            input = [input stringByAppendingString:input_cuts == 0 ? @"（入力をつないでいます…）" : @"（入力をつなぎ直しています…）"];
        }
        counted_window.title = [NSString stringWithFormat:@"%@ — 再接続：映像 %lu 回、%@%@",
                                counted_title, atomic_load(&picture_cuts), input,
                                clipboard_shown ? [@" — " stringByAppendingString:clipboard_shown] : @""];
    } else {
        NSString *input = input_cuts < 0 ? @"input not counted" : [NSString stringWithFormat:@"input %ld", input_cuts];
        counted_window.title = [NSString stringWithFormat:@"%@ \u2014 reconnections: picture %lu, %@",
                                counted_title, atomic_load(&picture_cuts), input];
    }
}

// Whether the host said it takes committed text (ADR 0007, "確定した文字"): only then is any sent.
static BOOL host_takes_text = NO;
// Whether the host said it takes copy and paste (ADR 0022): only then are Command-C, X and V taken.
static BOOL host_takes_clipboard = NO;
static BOOL host_takes_preedit = NO;
static BOOL force_preedit_strip = NO;
// ISTESSO_OUTSIDE_FIELD=ime: outside a field, keys go through the input method as before.
static BOOL outside_field_keys_through_ime = NO;

// Trial only (ADR 0007, password fields): Secure Event Input is on while a password field of the
// host has the keyboard and this window takes keys, so that other applications on this Mac
// cannot see the keystrokes. Always let go: when that ends, and when the program ends (atexit;
// the window's end and the stop signals end the program through exit()).
// Atomic: the handler at exit may run on another thread than the main one.
static atomic_bool secure_input_on = false;

static void secure_input(BOOL on) {
    bool was = !on;
    if (atomic_compare_exchange_strong(&secure_input_on, &was, on ? true : false)) {
        if (on) {
            EnableSecureEventInput();
        } else {
            DisableSecureEventInput();
        }
    }
}

static void secure_input_off_at_exit(void) {
    secure_input(NO);
}

@interface NSView (IstessoTextInput)
- (void)setTextInputField:(IstessoMediaTextInput)field;
- (void)textInputContextChanged;
@end

// Field descriptions belong to one picture connection. Only the text input view consumes them.
static void update_text_input_field(IstessoMediaTextInput field) {
    if ([cursor_view respondsToSelector:@selector(setTextInputField:)]) {
        [cursor_view setTextInputField:field];
    }
}

// The number after "NAME": in the relay's state line, or `otherwise` when the line does not say it.
static long relay_state_field(const char *line, const char *name, long otherwise) {
    char key[32];
    snprintf(key, sizeof key, "\"%s\":", name);
    const char *at = strstr(line, key);
    if (at == NULL) {
        return otherwise;
    }
    const char *digits = at + strlen(key);
    char *end = NULL;
    long value = strtol(digits, &end, 10);
    return end == digits ? otherwise : value;
}

// Looks once a second at what the relay says about its own connection, off the main thread:
// reading a file must never hold up a key on its way out.
static void watch_relay_state(const char *socket_path) {
    NSString *path = [NSString stringWithFormat:@"%s.state", socket_path];
    static dispatch_source_t timer;
    dispatch_queue_t queue = dispatch_get_global_queue(QOS_CLASS_UTILITY, 0);
    timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, queue);
    dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, 0), NSEC_PER_SEC, NSEC_PER_SEC / 10);
    dispatch_source_set_event_handler(timer, ^{
        long now = -1, text = 0, clipboard = 0, preedit = 0;
        BOOL reconnecting = NO;
        FILE *file = fopen(path.fileSystemRepresentation, "r");
        char content[256] = {0};
        if (file != NULL) {
            size_t got = fread(content, 1, sizeof content - 1, file);
            content[got] = 0;
            fclose(file);
        }
        if (strncmp(content, "{\"pid\":", 7) == 0) {
            // Only a relay that still runs counts: a file left by an earlier one says nothing.
            // "text" (whether the host takes committed text) is left out by an older relay, so are
            // "clipboard" (copy and paste) and "preedit" (composing text), and "reconnecting" is
            // said by a relay of the everyday trial's reconnecting kind only.
            long pid = relay_state_field(content, "pid", 0);
            long count = relay_state_field(content, "input", -1);
            // A process number is a positive int: anything else (a negative one, or one past
            // INT_MAX that the cast would turn into a group or every process) is not looked at.
            if (pid > 0 && pid <= INT_MAX && count >= 0 && (kill((pid_t)pid, 0) == 0 || errno == EPERM)) {
                now = count;
                text = relay_state_field(content, "text", 0) == 1;
                clipboard = relay_state_field(content, "clipboard", 0) == 1;
                preedit = relay_state_field(content, "preedit", 0) == 1;
                reconnecting = daily_reconnect && relay_state_field(content, "reconnecting", 0) == 1;
            }
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            host_takes_clipboard = clipboard != 0;
            if (host_takes_text != (text != 0) || host_takes_preedit != (preedit != 0)) {
                host_takes_text = text != 0;
                host_takes_preedit = preedit != 0;
                if ([cursor_view respondsToSelector:@selector(textInputContextChanged)]) {
                    [cursor_view textInputContextChanged];
                }
            }
            if (now != input_cuts || reconnecting != input_reconnecting) {
                if (daily_reconnect && now >= 0 && !reconnecting && !picture_stale &&
                    [cursor_view respondsToSelector:@selector(fallInLine)]) {
                    [cursor_view fallInLine];
                }
                input_reconnecting = reconnecting;
                input_cuts = now;
                show_counts();
            }
        });
    });
    dispatch_resume(timer);
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

// The everyday trial (ISTESSO_DAILY_RECONNECT): when the host starts its stream again at another
// size (the Mac host does when the application's window is resized), the window follows: the same
// points for each of the host's pixels as before (square pixels, as the host sends its window 1:1),
// its top left where it was, and all of it on its screen (made smaller as a whole if needed).
static NSWindow *picture_window = nil;
static BOOL picture_whole_display = NO;
// Points of the window to one pixel of the host's picture, as the window was first made (0: not
// known yet). Kept when the window follows another size, so that the pixels stay as they were.
static double points_per_pixel = 0;
// A resize was asked of the host, and its answer (the next stream's size) is awaited.
static BOOL resize_asked = NO;

#include "window-size.h"

static void window_follows_picture(int width, int height) {
    if (picture_window == nil || picture_whole_display || width <= 0 || height <= 0) {
        return;
    }
    NSRect frame = picture_window.frame;
    NSRect content = [picture_window contentRectForFrameRect:frame];
    CGFloat bar = frame.size.height - content.size.height;
    NSRect room = (picture_window.screen ?: NSScreen.mainScreen).visibleFrame;
    double w, h;
    window_size_for_picture((unsigned)width, (unsigned)height, points_per_pixel, room.size.width, room.size.height - bar, &w, &h);
    NSPoint top_left = NSMakePoint(NSMinX(frame), NSMaxY(frame));
    [picture_window setContentSize:NSMakeSize(w, h)];
    NSRect now = picture_window.frame;
    top_left.x = MAX(NSMinX(room), MIN(top_left.x, NSMaxX(room) - now.size.width));
    top_left.y = MIN(NSMaxY(room), MAX(top_left.y, NSMinY(room) + now.size.height));
    [picture_window setFrameTopLeftPoint:top_left];
    fprintf(stderr, "window: %.0fx%.0f points for a picture of %dx%d (top left %.0f,%.0f)\n", w, h, width, height, top_left.x, top_left.y);
}

// The person has stopped resizing the window by hand (the end of a live resize): once, the host is
// asked for a picture of the size the window's picture area now has. While the window is being
// resized nothing is sent (the picture is only made larger or smaller). The answer is the next
// stream's size, to which the window then goes; that is never asked about again.
static void send_input(const char *format, ...) __attribute__((format(printf, 1, 2)));
static void ask_for_window_size(void) {
    if (picture_window == nil || picture_whole_display) {
        return;
    }
    NSRect content = [picture_window contentRectForFrameRect:picture_window.frame];
    unsigned width, height;
    picture_size_for_window(content.size.width, content.size.height, points_per_pixel, &width, &height);
    resize_asked = YES;
    send_input("{\"kind\":\"resize\",\"width\":%u,\"height\":%u}", width, height);
    fprintf(stderr, "note: asked the host for a picture of %ux%u (the window was resized by hand)\n", width, height);
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
    int was_width = picture_width, was_height = picture_height;
    picture_width = size.width;
    picture_height = size.height;
    // A new decoder for the new parameter sets is made below; the window follows a new size, and
    // goes to the answer of a resize that was asked for even when the size stayed the same.
    if (daily_reconnect) {
        int width = size.width, height = size.height;
        BOOL changed = was_width > 0 && (was_width != size.width || was_height != size.height);
        if (changed) {
            fprintf(stderr, "note: the host's picture is %dx%d now (it was %dx%d)\n", width, height, was_width, was_height);
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            if (points_per_pixel <= 0 && picture_window != nil && !picture_whole_display) {
                // As the window was made for the first picture.
                NSRect content = [picture_window contentRectForFrameRect:picture_window.frame];
                points_per_pixel = content.size.width / width;
            }
            if (changed || resize_asked) {
                resize_asked = NO;
                window_follows_picture(width, height);
            }
        });
    }
    vt_srgb = channel_info.colour == 1;
    return vt_start_session(channel_info.chroma == 444, vt_srgb ? "srgb" : "-");
}

static void channel_ask_for_key_frame(IstessoMediaWatch *watch, guint64 t) {
    // Once, and again every half second for as long as none has come.
    if (channel_asked_ns != 0 && t - channel_asked_ns < 500 * 1000 * 1000ULL) {
        return;
    }
    channel_asked_ns = t;
    istesso_media_watch_ask_key_frame(watch);
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"key_request\",\"t_ns\":%llu,\"sent\":1}\n", (unsigned long long)t);
    }
}

// One frame has arrived, whole: decode it now, on the thread that received it.
static void on_channel_frame(IstessoMediaWatch *watch, const IstessoMediaFrame *frame) {
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
        if (daily_reconnect && !daily_picture_drawn) {
            // The decoder may still be unavailable after wake. Try the next key frame.
            vt_needs_key_frame = TRUE;
            channel_ask_for_key_frame(watch, now_ns());
            return;
        }
        exit(1);
    }
    if (frame->discontinuity && !frame->key_frame) {
        vt_needs_key_frame = TRUE;  // The host left frames out and went on without a key frame.
    }
    if (vt_session == NULL || total == 0 || (vt_needs_key_frame && !frame->key_frame)) {
        vt_needs_key_frame = TRUE;
        channel_ask_for_key_frame(watch, now_ns());
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
        channel_ask_for_key_frame(watch, now_ns());
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

// Receives and decodes. The library connects, and connects again when the connection is cut;
// here the window is told what to show meanwhile.
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
    IstessoMediaWatch *watch = istesso_media_watch_open(NULL, channel_address, getenv("ISTESSO_CHANNEL_PEER"));
    if (watch == NULL) {
        fprintf(stderr, "error: %s\n", istesso_media_last_error());
        exit(2);
    }
    if (daily_reconnect) {
        istesso_media_watch_follow_restarts(watch);
    }
    guint64 began = now_ns(), cut_at = 0;
    gboolean ever = FALSE, said_unreachable = FALSE, back_pending = daily_reconnect;
    for (;;) {
        IstessoMediaFrame frame;
        int32_t got = istesso_media_watch_next(watch, 250, &frame);
        if (got == 1) {
            on_channel_frame(watch, &frame);
            if (back_pending && daily_reconnect && !daily_picture_drawn) {
                vt_needs_key_frame = TRUE;
                channel_ask_for_key_frame(watch, now_ns());
            }
            if (back_pending && !vt_needs_key_frame) {
                // The first picture after the cut has been decoded and handed to the layer.
                back_pending = FALSE;
                guint64 t = now_ns();
                if (frame_log != NULL && cut_at != 0) {
                    fprintf(frame_log, "{\"kind\":\"stream\",\"t_ns\":%llu,\"event\":\"back\",\"gap_ms\":%.1f}\n",
                            (unsigned long long)t, (double)(t - cut_at) / 1e6);
                }
                dispatch_async(dispatch_get_main_queue(), ^{
                    picture_is_back();
                });
            }
        } else if (got == 2) {
            IstessoMediaCursor cursor;
            if (istesso_media_watch_cursor(watch, &cursor) == 1) {
                on_channel_cursor(&cursor);
            }
        } else if (got == 5) {
            IstessoMediaTextInput field;
            if (istesso_media_watch_text_input(watch, &field) == 1) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    update_text_input_field(field);
                });
            }
        } else if (got == 3) {
            // Cut, and nobody said why: the library is connecting again.
            cut_at = now_ns();
            back_pending = TRUE;
            atomic_fetch_add(&picture_cuts, 1);
            fprintf(stderr, "note: the picture's connection was cut; connecting again\n");
            log_stream("ended");
            log_reconnections("cut");
            guint64 t = cut_at;
            dispatch_async(dispatch_get_main_queue(), ^{
                show_counts();
                update_text_input_field((IstessoMediaTextInput){0});
                picture_was_cut(t);
            });
        } else if (got == 4) {
            IstessoMediaWatchState state;
            memset(&state, 0, sizeof state);
            istesso_media_watch_state(watch, &state);
            if (istesso_media_watch_info(watch, &channel_info) != 0 || channel_info.codec != 1) {
                fprintf(stderr, "error: the host sends its picture in a form this program cannot decode\n");
                exit(1);
            }
            ever = TRUE;
            if (frame_log != NULL) {
                fprintf(frame_log, "{\"kind\":\"stream\",\"t_ns\":%llu,\"event\":\"connected\",\"again\":%u,\"silent_ms\":%llu}\n",
                        (unsigned long long)now_ns(), state.again, (unsigned long long)state.silent_ms);
            }
            // A new connection begins with a key frame, and the decoder begins anew with it.
            vt_needs_key_frame = TRUE;
            channel_asked_ns = 0;
            if (daily_reconnect) {
                daily_picture_drawn = NO;
                if (state.again) {
                    if (vt_session != NULL) {
                        VTDecompressionSessionInvalidate(vt_session);
                        CFRelease(vt_session);
                        vt_session = NULL;
                    }
                    channel_ask_for_key_frame(watch, now_ns());
                }
            }
        } else if (got < 0) {
            // Over: the host said why, or refused. Not tried again.
            IstessoMediaWatchState state;
            memset(&state, 0, sizeof state);
            istesso_media_watch_state(watch, &state);
            fprintf(stderr, "note: no picture any more: %s\n", istesso_media_last_error());
            if (frame_log != NULL) {
                fprintf(frame_log, "{\"kind\":\"stream\",\"t_ns\":%llu,\"event\":\"over\",\"why\":%d}\n",
                        (unsigned long long)now_ns(), state.why);
                log_reconnections("end");
                fflush(frame_log);
            }
            int32_t why = state.why;
            dispatch_async(dispatch_get_main_queue(), ^{
                update_text_input_field((IstessoMediaTextInput){0});
                picture_is_over(why);
            });
            return NULL;
        } else if (!ever && !said_unreachable && now_ns() - began > 1000000000ULL) {
            // Said once; the sentence holds no address.
            said_unreachable = TRUE;
            fprintf(stderr, "note: no picture from the host yet; trying again\n");
            log_stream("unreachable");
        }
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
    // Above both: what is shown while the connection is being made again, or when it is over.
    dim_layer = [CALayer layer];
    dim_layer.zPosition = 20;
    dim_layer.hidden = YES;
    dim_layer.backgroundColor = CGColorCreateGenericGray(0, 0.5);
    [vt_layer addSublayer:dim_layer];
    note_box = [CALayer layer];
    note_box.zPosition = 30;
    note_box.hidden = YES;
    note_box.cornerRadius = 8;
    note_box.backgroundColor = CGColorCreateGenericGray(0.08, 0.92);
    note_layer = [CATextLayer layer];
    note_layer.wrapped = YES;
    note_layer.alignmentMode = kCAAlignmentCenter;
    note_layer.foregroundColor = CGColorGetConstantColor(kCGColorWhite);
    [note_box addSublayer:note_layer];
    [vt_layer addSublayer:note_box];
    const char *pin = getenv("ISTESSO_CURSOR_AT");
    double pin_x = 0, pin_y = 0;
    if (pin != NULL && sscanf(pin, "%lf,%lf", &pin_x, &pin_y) == 2) {
        cursor_pinned = YES;
        cursor_pin = NSMakePoint(pin_x, pin_y);
    }
    if (getenv("ISTESSO_PRETEND_TYPING") != NULL) {
        static dispatch_source_t typing;
        typing = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
        dispatch_source_set_timer(typing, dispatch_time(DISPATCH_TIME_NOW, 0), NSEC_PER_SEC / 10, NSEC_PER_SEC / 100);
        dispatch_source_set_event_handler(typing, ^{
            // As a key press would be: noted when it would be passed on, counted when not.
            if (picture_stale) {
                held_back_count++;
            } else {
                note_press();
            }
        });
        dispatch_resume(typing);
    }
    if (daily_reconnect) {
        waiting_for_first_picture = YES;
        mark_the_picture(@"つないでいます…");
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
            if (channel_address != NULL) {
                log_reconnections("end");
            }
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
    char text[256]; // a scroll of the trackpad is the longest (scroll.h)
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

// ---- Copy and paste of text with the host's application (ADR 0022, the everyday trial) ----
//
// Only on Command-C, Command-X and Command-V in this window, and only when the relay says the host
// takes them. The window sends its requests from a socket of its own, in a folder only its user
// can open, with a nonce of 128 random bits; the relay answers to that socket only, with that
// nonce, and only the answer to the latest request, within two seconds, is used. What was copied
// or pasted is never written anywhere by this program.

#include "clipboard.h"

static BOOL clipboard_on = NO;
static uint32_t clipboard_next_id = 1;
static uint32_t clipboard_waiting = 0;
static char clipboard_nonce[33];
static BOOL clipboard_cut = NO;
static BOOL clipboard_pasting = NO;
static char clipboard_folder[PATH_MAX];
static char clipboard_socket_path[sizeof ((struct sockaddr_un *)0)->sun_path];

// Shows `note` in the title for two seconds; nil shows nothing.
static void clipboard_show(NSString *note) {
    if (note == nil) {
        return;
    }
    clipboard_shown = note;
    show_counts();
    if (counted_window == nil) {
        // A window without the counted title (not the everyday trial's): only that something
        // was not done, never what.
        fprintf(stderr, "note: a copy or paste was not done\n");
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (clipboard_shown == note) {
            clipboard_shown = nil;
            show_counts();
        }
    });
}

// One datagram of a request. The relay reads at once; a full socket is tried again for 50 ms.
static BOOL clipboard_send(NSData *datagram) {
    for (int attempt = 0; attempt < 50; attempt++) {
        if (sendto(input_socket, datagram.bytes, datagram.length, 0, (const struct sockaddr *)&input_to, sizeof input_to)
            == (ssize_t)datagram.length) {
            return YES;
        }
        if (errno != ENOBUFS && errno != EAGAIN) {
            return NO;
        }
        usleep(1000);
    }
    return NO;
}

// Asks for a copy ('c'), a cut ('x') or a paste ('v'). On the main thread.
static void clipboard_ask(char which) {
    uint8_t random[16];
    arc4random_buf(random, sizeof random);
    for (int i = 0; i < 16; i++) {
        snprintf(clipboard_nonce + 2 * i, 3, "%02x", random[i]);
    }
    uint32_t id = clipboard_next_id++;
    NSMutableDictionary *request = [@{@"id": @(id), @"nonce": @(clipboard_nonce)} mutableCopy];
    if (which == 'v') {
        NSString *text = nil;
        PasteKind kind = clipboard_read_paste(NSPasteboard.generalPasteboard, &text);
        switch (kind) {
        case PASTE_SECRET: clipboard_show(clipboard_note("paste:secret", NULL, NO)); return;
        case PASTE_TOO_LARGE: clipboard_show(clipboard_note("paste:too_large", NULL, NO)); return;
        case PASTE_NOT_TEXT: clipboard_show(clipboard_note("paste:not_text", NULL, NO)); return;
        case PASTE_WITH_TEXT: request[@"text"] = text; break;
        case PASTE_KEY_ONLY: break;
        }
        request[@"kind"] = @"paste";
    } else {
        request[@"kind"] = @"copy";
        if (which == 'x') {
            request[@"cut"] = @YES;
        }
    }
    NSData *datagram = [NSJSONSerialization dataWithJSONObject:request options:0 error:NULL];
    if (datagram == nil || !clipboard_send(datagram)) {
        clipboard_show(clipboard_note("timeout", NULL, NO));
        return;
    }
    clipboard_waiting = id;
    clipboard_cut = which == 'x';
    clipboard_pasting = which == 'v';
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(CLIPBOARD_WITHIN * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        if (clipboard_waiting == id) {
            clipboard_waiting = 0;
            clipboard_show(clipboard_note("timeout", NULL, NO));
        }
    });
}

// An answer from the relay. On the main thread.
static void clipboard_answered(NSDictionary *answer) {
    if (![answer isKindOfClass:NSDictionary.class] || ![answer[@"kind"] isEqual:@"clipboard"]) {
        return;
    }
    NSNumber *id = answer[@"id"];
    NSString *nonce = answer[@"nonce"], *result = answer[@"result"], *why = answer[@"why"], *text = answer[@"text"];
    if (clipboard_waiting == 0 || ![id isKindOfClass:NSNumber.class] || id.unsignedIntValue != clipboard_waiting ||
        ![nonce isKindOfClass:NSString.class] || strcmp(nonce.UTF8String, clipboard_nonce) != 0 ||
        ![result isKindOfClass:NSString.class]) {
        return;
    }
    clipboard_waiting = 0;
    if (![why isKindOfClass:NSString.class]) {
        why = nil;
    }
    if (clipboard_pasting) {
        clipboard_show(clipboard_note([result isEqual:@"pasted"] ? "pasted" : "paste:none", NULL, NO));
        return;
    }
    if ([result isEqual:@"text"]) {
        if (![text isKindOfClass:NSString.class] || clipboard_text_problem(text) != NULL) {
            clipboard_show(clipboard_note("not_text", NULL, clipboard_cut));
            return;
        }
        NSPasteboard *board = NSPasteboard.generalPasteboard;
        // This Mac only: not handed to other devices by Universal Clipboard (ADR 0022, decision 2).
        [board prepareForNewContentsWithOptions:NSPasteboardContentsCurrentHostOnly];
        [board setString:text forType:NSPasteboardTypeString];
        return;
    }
    clipboard_show(clipboard_note(result.UTF8String, why.UTF8String, clipboard_cut));
}

// The window's own socket, in a folder of its own, for requests and their answers.
static BOOL clipboard_start(void) {
    snprintf(clipboard_folder, sizeof clipboard_folder, "%sistesso-viewer-XXXXXX", NSTemporaryDirectory().fileSystemRepresentation);
    if (mkdtemp(clipboard_folder) == NULL) {
        return NO;
    }
    struct sockaddr_un own = {.sun_family = AF_UNIX};
    if ((size_t)snprintf(own.sun_path, sizeof own.sun_path, "%s/w.sock", clipboard_folder) >= sizeof own.sun_path) {
        rmdir(clipboard_folder);
        return NO;
    }
    if (bind(input_socket, (const struct sockaddr *)&own, sizeof own) != 0) {
        rmdir(clipboard_folder);
        return NO;
    }
    strcpy(clipboard_socket_path, own.sun_path);
    chmod(clipboard_socket_path, 0600);
    atexit_b(^{
        unlink(clipboard_socket_path);
        rmdir(clipboard_folder);
    });
    int size = 256 * 1024;
    setsockopt(input_socket, SOL_SOCKET, SO_SNDBUF, &size, sizeof size);
    setsockopt(input_socket, SOL_SOCKET, SO_RCVBUF, &size, sizeof size);
    static dispatch_source_t reading;
    reading = dispatch_source_create(DISPATCH_SOURCE_TYPE_READ, (uintptr_t)input_socket, 0,
                                     dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0));
    dispatch_source_set_event_handler(reading, ^{
        static uint8_t buffer[192 * 1024];
        for (;;) {
            ssize_t length = recv(input_socket, buffer, sizeof buffer, 0);
            if (length <= 0) {
                break;
            }
            NSData *data = [NSData dataWithBytes:buffer length:(NSUInteger)length];
            NSDictionary *answer = [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL];
            if (answer != nil) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    clipboard_answered(answer);
                });
            }
        }
    });
    dispatch_resume(reading);
    return YES;
}

// Whether the Command keys go to the other side as Control (the default) or as the Super
// ("Windows") keys. ISTESSO_COMMAND_KEY=super in the environment sends Super.
static BOOL command_is_control = YES;

#include "scroll.h"

// The view the picture is shown in, when input is passed on: it takes the keyboard and the
// pointer and writes what happens to the socket.
@interface InputView : NSView
@end

@implementation InputView {
    Keys _keys;          // keys that are down on the other side
    unsigned _buttons;   // buttons that are down on the other side (bit 0: left, 1: right, 2: other)
    ScrollState _scroll; // parts of a pixel and of a notch not yet passed on
    BOOL _inside;        // the pointer is over the view
    NSPoint _at;         // where, in the view's own coordinates (from its bottom left)
    NSMutableIndexSet *_taken;  // keys taken for copy and paste: their release is not passed on
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
    if (cursor_layer != nil && cursor_shape != 1 && !picture_stale) {
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
    // Not while the picture is being reconnected: the cursor is the host's, and the host's
    // picture is not there.
    BOOL show = here && !picture_stale && cursor_shape == 2 && picture_width > 0 && self.bounds.size.width > 0;
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
    [self modifiersUp:NSEvent.modifierFlags];
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
    // What this view watched was the window it was in: it watches the one it is in now, once.
    [NSNotificationCenter.defaultCenter removeObserver:self];
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
    // Keys go to the other side only while this window is the one typed into: when it stops
    // being that (another window, another application, Command-Tab, the screenshot keys), what
    // is down over there comes up; when it is that again, the modifiers held now go down there.
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(letGo)
                                               name:NSWindowDidResignKeyNotification
                                             object:self.window];
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(letGo)
                                               name:NSApplicationDidResignActiveNotification
                                             object:NSApp];
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(keyboardIsBack)
                                               name:NSWindowDidBecomeKeyNotification
                                             object:self.window];
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(keyboardIsBack)
                                               name:NSApplicationDidBecomeActiveNotification
                                             object:NSApp];
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
    [_taken removeAllIndexes];
}

// This window is the one typed into: the key window of the active application. Only then do the
// modifiers' own releases come here.
- (BOOL)typedInto {
    return self.window.isKeyWindow && NSApp.isActive;
}

// The window is the one typed into again: the modifiers held on this keyboard now go down on the
// other side (nothing else: a letter pressed again would be typed). Not while the picture is
// being reconnected; fallInLine does it when the picture is back.
- (void)keyboardIsBack {
    if (picture_stale) {
        return;
    }
    [self fallInLine];
}

// Before a button or a scroll is sent the modifiers over there are brought in line with the
// event's flags (keys_around_pointer() in keys.h): a modifier whose release went elsewhere comes
// up; one that is held and whose press went elsewhere goes down, and stays down only while this
// window is the one typed into. What is to go after the button or scroll is left in `changes`
// from the returned place to `*end`. Usually there is nothing to send, and nothing is sent.
- (size_t)modifiersBefore:(NSEvent *)event changes:(KeyChange *)changes end:(size_t *)end {
    _keys.command_is_control = command_is_control;
    size_t before = 0;
    *end = keys_around_pointer(&_keys, event.modifierFlags, [self typedInto], picture_stale, changes, &before);
    if (before > 0) {
        [self send:changes count:before];
    }
    return before;
}

// A modifier that is down over there and not held any more comes up (something took the keyboard
// without the window losing it: Spotlight, a palette, a menu). Nothing goes down, and when all is
// in line nothing is sent.
- (void)modifiersUp:(NSEventModifierFlags)flags {
    KeyChange changes[8];
    _keys.command_is_control = command_is_control;
    size_t count = keys_releases(&_keys, flags, changes);
    if (count > 0) {
        [self send:changes count:count];
    }
}

// The line that says "what you type does not arrive" has come up: whatever is held on the other
// side is let go there, keys and buttons. From here until the picture is back nothing is pressed.
- (void)holdBack {
    [self letGo];
    for (unsigned i = 0; i < 3; i++) {
        if (_buttons & (1u << i)) {
            send_input("{\"kind\":\"button\",\"code\":%u,\"pressed\":false}", 0x110 + i);
        }
    }
    _buttons = 0;
}

// The picture is back: the modifiers held on this keyboard are held on the other side again.
// Buttons stay up there; one that is still held here counts as not held until it is let go
// (pressing it over there now would begin a click or a drag where the pointer happens to be).
// Only while this window is the one typed into: otherwise nothing is held for it.
- (void)fallInLine {
    if (![self typedInto]) {
        return;
    }
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    [self send:changes count:keys_fall_in_line(&_keys, NSEvent.modifierFlags, daily_reconnect, changes)];
}

// The state of the host's text input field that has the keyboard, as the host last said (0: none
// said). Only the text input view hears it.
- (unsigned int)fieldState {
    return 0;
}

- (void)keyDown:(NSEvent *)event {
    // Copy, cut and paste with the host (ADR 0022): the key itself does not go; the host presses
    // its own. Once for each press: a repeat, and the release, do not go either.
    if (clipboard_on && host_takes_clipboard) {
        char which = clipboard_key(event.keyCode, event.charactersIgnoringModifiers, event.modifierFlags);
        if (which != 0) {
            if (_taken == nil) {
                _taken = [NSMutableIndexSet indexSet];
            }
            [_taken addIndex:event.keyCode];
            if (!event.isARepeat && !picture_stale) {
                if (clipboard_refused_here(which, [self fieldState])) {
                    // A password field over there: not asked at all, with the host's own notice.
                    clipboard_show(clipboard_note("refused", "password_field", which == 'x'));
                } else {
                    clipboard_ask(which);
                }
            }
            return;
        }
    }
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    size_t count = keys_key_down(&_keys, event.keyCode, event.modifierFlags, event.isARepeat, changes);
    if (picture_stale) {
        // Not pressed on the other side while the picture is not the host's of this moment.
        bool dropped = false;
        count = keys_hold_back(&_keys, changes, count, &dropped);
        held_back_count += dropped ? 1 : 0;
    } else if (keys_presses_key(&_keys, event.keyCode, changes, count)) {
        note_press();
    }
    [self send:changes count:count];
}

- (void)keyUp:(NSEvent *)event {
    if ([_taken containsIndex:event.keyCode]) {
        [_taken removeIndex:event.keyCode];
        return;
    }
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    [self send:changes count:keys_key_up(&_keys, event.keyCode, event.modifierFlags, changes)];
}

// Keys with Command come here first; they all go to the other side, but for Command-V while the
// host takes copy and paste.
- (BOOL)performKeyEquivalent:(NSEvent *)event {
    // Only for the window that is typed into: a key equivalent can reach a window that is not
    // (review of ADR 0022's code), and Command-C there must not copy from the host.
    if (self.window.firstResponder != self || !self.window.isKeyWindow) {
        return NO;
    }
    if (clipboard_on && host_takes_clipboard &&
        clipboard_key(event.keyCode, event.charactersIgnoringModifiers, event.modifierFlags) == 'v') {
        // Not taken here: it goes on to the Edit menu's Paste (paste:, below), so that this
        // Mac's pasteboard is read in answer to a paste-related control, as macOS's pasteboard
        // privacy asks ("input on a UI element that the system considers paste-related"). Without
        // the menu the key comes to keyDown:, which asks as before.
        return NO;
    }
    [self keyDown:event];
    return YES;
}

// The Edit menu's Paste, by Command-V (or a click on it): the paste goes to the host with what
// this Mac's pasteboard has. The key itself does not go, nor its repeats and its release.
- (void)paste:(id)sender {
    (void)sender;
    if (!(clipboard_on && host_takes_clipboard) || self.window.firstResponder != self) {
        return;
    }
    NSEvent *event = NSApp.currentEvent;
    if (event.type == NSEventTypeKeyDown) {
        if (_taken == nil) {
            _taken = [NSMutableIndexSet indexSet];
        }
        [_taken addIndex:event.keyCode];
        if (event.isARepeat) {
            return;
        }
    }
    if (!picture_stale) {
        clipboard_ask('v');
    }
}

- (BOOL)validateMenuItem:(NSMenuItem *)item {
    if (item.action == @selector(paste:)) {
        return clipboard_on && host_takes_clipboard && self.window.firstResponder == self && self.window.isKeyWindow;
    }
    return NO;
}

- (void)flagsChanged:(NSEvent *)event {
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    size_t count = keys_flags_changed(&_keys, event.keyCode, event.modifierFlags, changes);
    if (picture_stale) {
        // A modifier by itself is not something typed: not counted, and not pressed.
        bool dropped = false;
        count = keys_hold_back(&_keys, changes, count, &dropped);
    }
    [self send:changes count:count];
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
    unsigned bit = 1u << (code - 0x110);
    if (pressed && picture_stale) {
        // Not pressed on the other side while the picture is not the host's of this moment.
        held_back_count++;
        return;
    }
    if (!pressed && !(_buttons & bit)) {
        return;  // It never went down over there, or was let go there already.
    }
    KeyChange around[KEYS_MOST];
    size_t end = 0, after = [self modifiersBefore:event changes:around end:&end];
    _buttons = pressed ? (_buttons | bit) : (_buttons & ~bit);
    if (pressed) {
        note_press();
    }
    send_input("{\"kind\":\"button\",\"code\":%u,\"pressed\":%s}", code, pressed ? "true" : "false");
    if (end > after) {
        [self send:around + after count:end - after];
    }
}

- (void)mouseMoved:(NSEvent *)event {
    [self pointer:event];
    [self modifiersUp:event.modifierFlags];
}
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

static void send_scroll(void *context, const char *message) {
    (void)context;
    send_input("%s", message);
}

// The modifiers around one scroll event: brought in line before its first datagram (and lent
// when the window is not the one typed into; keys_around_pointer() in keys.h), and the rest of
// the changes after its last. Nothing for an event that sends nothing.
typedef struct {
    __unsafe_unretained InputView *view;
    __unsafe_unretained NSEvent *event;
    KeyChange around[KEYS_MOST];
    size_t after, end;
} ScrollModifiers;

static void scroll_modifiers_first(void *context) {
    ScrollModifiers *m = context;
    m->after = [m->view modifiersBefore:m->event changes:m->around end:&m->end];
}

- (void)scrollWheel:(NSEvent *)event {
    if (picture_stale) {
        // Scrolling a picture that is not there moves something nobody sees. An end held back
        // is dropped too: the compositor stops a scroll whose end never comes by itself.
        _scroll = (ScrollState){0};
        return;
    }
    // A wheel goes as notches (its whole lines), a trackpad as pixels of the picture with the
    // stage of the gesture and of the inertia after it (scroll.h; ADR 0007, "スクロールの種類と
    // 段階"), so that the content moves under the fingers as far as it would in a window here.
    NSSize size = self.bounds.size;
    CGEventRef cg = event.CGEvent;
    ScrollIn in = {
        .precise = event.hasPreciseScrollingDeltas,
        .dx = event.scrollingDeltaX,
        .dy = event.scrollingDeltaY,
        .has_lines = cg != NULL,
        .lines_x = cg ? (long)CGEventGetIntegerValueField(cg, kCGScrollWheelEventDeltaAxis2) : 0,
        .lines_y = cg ? (long)CGEventGetIntegerValueField(cg, kCGScrollWheelEventDeltaAxis1) : 0,
        .phase = (unsigned)event.phase,
        .momentum = (unsigned)event.momentumPhase,
        .scale_x = size.width > 0 && picture_width > 0 ? picture_width / size.width : 1.0,
        .scale_y = size.height > 0 && picture_height > 0 ? picture_height / size.height : 1.0,
    };
    // The modifiers over there are brought in line with this event's around what it sends, and
    // only when it sends something (a Command-scroll carries Command).
    ScrollModifiers modifiers = {.view = self, .event = event};
    ScrollAround around = {send_scroll, &modifiers, scroll_modifiers_first, false};
    if (scroll_event_around(&_scroll, &in, event.timestamp * 1000.0, &around) && modifiers.end > modifiers.after) {
        [self send:modifiers.around + modifiers.after count:modifiers.end - modifiers.after];
    }
    if (_scroll.held) {
        // The end of the gesture waits here for macOS's inertia; it goes without it once the
        // wait is over (a later event that comes first sends it instead).
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)((SCROLL_MOMENTUM_WAIT_MS + 1) * NSEC_PER_MSEC)),
                       dispatch_get_main_queue(), ^{
                           if (!picture_stale) {
                               scroll_due(&self->_scroll, NSProcessInfo.processInfo.systemUptime * 1000.0, send_scroll, NULL);
                           }
                       });
    }
}

@end

// ADR 0007 ("確定した文字"), ADR 0020 route d: local composition in the trial viewer
// is switched on by ISTESSO_TEXT_INPUT=1.
#include "text-input.h"

@interface TextInputView : InputView <NSTextInputClient>
@end

// Commits that could not be passed on whole (the rest of each was not sent).
static unsigned long text_lost_count;

// One datagram of a commit. The socket is non-blocking and holds only a few kilobytes on macOS: a
// full one is tried again for up to 50 ms (the relay reads it at once), then given up.
static BOOL send_text_datagram(const char *packet, size_t length) {
    for (int attempt = 0; attempt < 50; attempt++) {
        if (sendto(input_socket, packet, length, 0, (const struct sockaddr *)&input_to, sizeof input_to) == (ssize_t)length) {
            return YES;
        }
        if (errno != ENOBUFS && errno != EAGAIN) {
            return NO;
        }
        usleep(1000);
    }
    return NO;
}

@implementation TextInputView {
    NSString *_marked;
    NSRange _selection;
    CATextLayer *_markedStrip;
    NSEvent *_interpretedEvent;
    BOOL _startedWithMark, _passKey, _discardingMark;
    IstessoMediaTextInput _field;
    BOOL _preeditSent, _stripComposition;
    // Clicks and keys that may move the focus over there (not plain typing), sent since the
    // host last said what the field is.
    unsigned long _movesSinceNotice;
}

// Cancel the local composition and clear any preedit still visible on the host.
- (void)discardComposition {
    _interpretedEvent = nil;
    _discardingMark = YES;
    [self.inputContext discardMarkedText];
    [self unmarkText];
    _discardingMark = NO;
}

// Command-V (the Edit menu's Paste) while Japanese is being composed: performKeyEquivalent: left
// it to the menu, so the input method never saw it. The composition is committed first.
- (void)paste:(id)sender {
    if (clipboard_on && host_takes_clipboard && self.window.firstResponder == self) {
        [self commitComposition];
    }
    [super paste:sender];
}

- (void)letGo {
    [self discardComposition];
    [super letGo];
    [self updatePasswordField];
}

- (void)windowIsKeyOrNot {
    [super windowIsKeyOrNot];
    [self updatePasswordField];
}

- (void)viewWillMoveToWindow:(NSWindow *)window {
    if (window == nil) {
        secure_input(NO);
    }
    [super viewWillMoveToWindow:window];
}

- (void)viewDidMoveToWindow {
    [super viewDidMoveToWindow];
    if (self.window == nil) {
        return;
    }
    // The candidates' place is in screen points: asked again when the window moves or changes
    // size.
    for (NSNotificationName name in @[ NSWindowDidMoveNotification, NSWindowDidResizeNotification ]) {
        [NSNotificationCenter.defaultCenter addObserver:self
                                               selector:@selector(windowPlaceChanged)
                                                   name:name
                                                 object:self.window];
    }
}

- (void)windowPlaceChanged {
    [self.inputContext invalidateCharacterCoordinates];
}

// A password field of the host (field state 2): plain keys only, as on this Mac, without the
// input method (Latin input sources only), no composing text and no strip; Secure Event Input
// while this window takes keys.
- (unsigned int)fieldState {
    return _field.state;
}

- (void)updatePasswordField {
    BOOL password = _field.state == 2;
    self.inputContext.allowedInputSourceLocales = password ? @[ NSAllRomanInputSourcesLocaleIdentifier ] : nil;
    secure_input(password && self.window.isKeyWindow && !picture_stale);
}

// A click, and a paste (Command-V by the Edit menu, which the input method never sees), commit
// what is being composed, once, into the field it was composed for (as a Mac does), before the
// click or the paste itself goes: a click may put the cursor into another field over there, and a
// paste lands after the composition, never inside it. Committed text goes on the input socket
// like the paste's request, so it arrives first.
- (void)commitComposition {
    if (!self.hasMarkedText) {
        return;
    }
    NSString *text = [_marked copy];
    [self discardComposition];
    if (_field.state != 2 && host_takes_text && !picture_stale && input_socket >= 0) {
        if (!text_input_packets(text, ^BOOL(const char *packet, size_t length) {
                return send_text_datagram(packet, length);
            })) {
            text_lost_count++;
            fprintf(stderr, "viewer: a commit could not be passed on whole, and the rest of it was not sent (%lu so far; the text is never shown)\n", text_lost_count);
        }
    }
}
- (void)mouseDown:(NSEvent *)event {
    [self commitComposition];
    _movesSinceNotice++;
    [super mouseDown:event];
}
- (void)rightMouseDown:(NSEvent *)event {
    [self commitComposition];
    _movesSinceNotice++;
    [super rightMouseDown:event];
}
- (void)otherMouseDown:(NSEvent *)event {
    [self commitComposition];
    _movesSinceNotice++;
    [super otherMouseDown:event];
}

- (void)keyDown:(NSEvent *)event {
    if ([self outsideField]) {
        // Not in a field over there: keys as they are, no input method, no strip. Eisu and
        // Kana still switch this Mac's input method, and nothing is sent for them.
        if (event.keyCode == 0x66 || event.keyCode == 0x68) {
            [self.inputContext handleEvent:event];
        } else {
            [super keyDown:event];
        }
        if (moves_focus(event)) {
            _movesSinceNotice++;
        }
        return;
    }
    if (moves_focus(event)) {
        _movesSinceNotice++;
    }
    if (_field.state == 2) {
        // A password field: keys as they are, never through the input method. Eisu and Kana
        // do nothing here.
        if (event.keyCode != 0x66 && event.keyCode != 0x68) {
            [super keyDown:event];
        }
        return;
    }
    // Eisu and Kana select the local input method, even with modifiers held.
    if (event.keyCode == 0x66 || event.keyCode == 0x68) {
        [self.inputContext handleEvent:event];
        return;
    }
    _startedWithMark = self.hasMarkedText;
    if (!_startedWithMark && (event.modifierFlags & (NSEventModifierFlagCommand | NSEventModifierFlagControl))) {
        [super keyDown:event];
        return;
    }
    _interpretedEvent = event;
    _passKey = NO;
    BOOL handled = [self.inputContext handleEvent:event];
    if (_interpretedEvent && (_passKey || !handled)) {
        [super keyDown:event];
    }
    _interpretedEvent = nil;
}

- (NSRect)markedStripRect {
    return NSMakeRect(8, 8, MAX(1, NSWidth(self.bounds) - 16), 24);
}

- (BOOL)hasMarkedText {
    return _marked.length > 0;
}

- (NSRange)markedRange {
    return self.hasMarkedText ? NSMakeRange(0, _marked.length) : NSMakeRange(NSNotFound, 0);
}

- (NSRange)selectedRange {
    return self.hasMarkedText ? _selection : NSMakeRange(0, 0);
}

static NSString *plain_text(id value) {
    return [value isKindOfClass:NSAttributedString.class] ? [value string] : value;
}

- (void)setMarkedText:(id)value selectedRange:(NSRange)selection replacementRange:(NSRange)replacement {
    if (_discardingMark) {
        return;
    }
    _marked = [plain_text(value) copy];
    if (!self.hasMarkedText) {
        [self unmarkText];
        return;
    }
    NSUInteger start = MIN(selection.location, _marked.length);
    _selection = NSMakeRange(start, MIN(selection.length, _marked.length - start));
    if ([self canUseInlinePreedit]) {
        BOOL hasText = NO;
        NSData *packet = text_input_preedit(_marked, _selection, &hasText);
        if (packet && send_text_datagram(packet.bytes, packet.length)) {
            _preeditSent = hasText;
            [self hideStrip];
            return;
        }
        // Never send just part of a mark, even if it gets shorter on the next edit.
        _stripComposition = YES;
    }
    [self clearInlinePreedit];
    [self showStrip:_marked selection:_selection];
}

- (BOOL)canUseInlinePreedit {
    return !force_preedit_strip && !_stripComposition && host_takes_preedit && host_takes_text
        && _field.state == 1 && !picture_stale && !cut_pending;
}

- (void)clearInlinePreedit {
    if (_preeditSent) {
        _preeditSent = NO;
        // Also when the host has just said it takes no more: the relay drops it then, and it
        // costs nothing; otherwise the application would go on showing a stale composition.
        const char clear[] = "{\"kind\":\"preedit\",\"text\":\"\",\"cursor\":[0,0]}";
        send_text_datagram(clear, sizeof clear - 1);
    }
}

// Outside a field (state 3, newer than any click or focus-moving key since): keys go as they
// are, without the input method and without the strip. Otherwise, as before.
- (BOOL)outsideField {
    return !outside_field_keys_through_ime && host_takes_preedit && _field.state == 3 && _movesSinceNotice == 0
        && !picture_stale && !self.hasMarkedText;
}

// Whether a key may move the focus over there: anything but plain typing.
static BOOL moves_focus(NSEvent *event) {
    if (event.modifierFlags & (NSEventModifierFlagCommand | NSEventModifierFlagControl | NSEventModifierFlagOption)) {
        return YES;
    }
    NSString *typed = event.charactersIgnoringModifiers;
    if (typed.length != 1) {
        return YES;
    }
    unichar c = [typed characterAtIndex:0];
    // Tab, Return, Escape, Delete, arrows and other function keys are not plain typing.
    return c < 0x20 || c == 0x7f || (c >= 0xF700 && c <= 0xF8FF);
}

- (void)setTextInputField:(IstessoMediaTextInput)field {
    _movesSinceNotice = 0;
    BOOL toPassword = field.state == 2 && _field.state != 2;
    // Another field has the keyboard (the application moved it by itself, or it was enabled
    // again): what was being composed ends here, on this Mac too, without being committed.
    BOOL otherField = field.serial != _field.serial;
    _field = field;
    if ((toPassword || otherField) && self.hasMarkedText) {
        [self discardComposition];
    }
    [self updatePasswordField];
    [self textInputContextChanged];
}

- (void)textInputContextChanged {
    if (self.hasMarkedText && ![self canUseInlinePreedit]) {
        [self clearInlinePreedit];
        [self showStrip:_marked selection:_selection];
    }
    [self.inputContext invalidateCharacterCoordinates];
}

- (void)showStrip:(NSString *)text selection:(NSRange)selection {
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (!_markedStrip) {
        self.wantsLayer = YES;
        _markedStrip = [CATextLayer layer];
        _markedStrip.frame = self.markedStripRect;
        _markedStrip.autoresizingMask = kCALayerWidthSizable;
        _markedStrip.backgroundColor = [NSColor colorWithWhite:0 alpha:0.75].CGColor;
        _markedStrip.zPosition = 15;
        [self.layer addSublayer:_markedStrip];
    }
    _markedStrip.contentsScale = self.window.backingScaleFactor;
    NSMutableAttributedString *display = [[NSMutableAttributedString alloc] initWithString:text attributes:@{
        NSForegroundColorAttributeName : NSColor.whiteColor, NSFontAttributeName : [NSFont systemFontOfSize:16]
    }];
    [display addAttribute:NSUnderlineStyleAttributeName value:@(NSUnderlineStyleSingle) range:selection];
    _markedStrip.string = display;
    _markedStrip.hidden = NO;
    [CATransaction commit];
}

// A line in the strip for a few seconds, unless something is being composed by then.
- (void)showNotice:(NSString *)notice {
    [self showStrip:notice selection:NSMakeRange(notice.length, 0)];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (!self.hasMarkedText) {
            [self unmarkText];
        }
    });
}

- (void)hideStrip {
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    _markedStrip.string = nil;
    _markedStrip.hidden = YES;
    [CATransaction commit];
}

- (void)unmarkText {
    [self clearInlinePreedit];
    _marked = nil;
    _stripComposition = NO;
    [self hideStrip];
}

- (void)insertText:(id)value replacementRange:(NSRange)replacement {
    if (_discardingMark) {
        return;
    }
    NSString *text = plain_text(value);
    if (_interpretedEvent && !_startedWithMark && !self.hasMarkedText && [text isEqualToString:_interpretedEvent.characters]) {
        _passKey = YES;
        return;
    }
    if (picture_stale) {
        [self unmarkText];
        held_back_count += text.length > 0;
        return;
    }
    if (!host_takes_text) {
        // An agent that does not take committed text could not read the message at all
        // (keys and all): the text stays here, and the strip says why.
        [self unmarkText];
        [self showNotice:@"このホストは文字の入力にまだ対応していません"];
        return;
    }
    if (input_socket >= 0 && !text_input_packets(text, ^BOOL(const char *packet, size_t length) {
            note_press();
            return send_text_datagram(packet, length);
        })) {
        text_lost_count++;
        fprintf(stderr, "viewer: a commit could not be passed on whole, and the rest of it was not sent (%lu so far; the text is never shown)\n", text_lost_count);
    }
    [self unmarkText];
}

- (void)doCommandBySelector:(SEL)selector {
    if (_interpretedEvent && !_startedWithMark && !self.hasMarkedText) {
        _passKey = YES;
    }
}

- (NSRect)firstRectForCharacterRange:(NSRange)range actualRange:(NSRangePointer)actual {
    if (actual) {
        *actual = self.hasMarkedText ? self.markedRange : self.selectedRange;
    }
    NSRect anchor = self.markedStripRect;
    NSSize size = self.bounds.size;
    CGFloat width = picture_width, height = picture_height;
    // A field whose place the host does not know (all zero) keeps the strip's place.
    BOOL placed = _field.x != 0 || _field.y != 0 || _field.width != 0 || _field.height != 0;
    if ([self canUseInlinePreedit] && placed && (!self.hasMarkedText || _markedStrip.hidden || !_markedStrip)
        && width > 0 && height > 0 && size.width > 0 && size.height >= 1) {
        // Clip both edges in picture pixels before reversing -pointer:'s top-left mapping.
        CGFloat left = MAX(0, MIN(width, (CGFloat)_field.x));
        CGFloat right = MAX(0, MIN(width, (CGFloat)_field.x + _field.width));
        CGFloat top = MAX(0, MIN(height, (CGFloat)_field.y));
        CGFloat bottom = MAX(0, MIN(height, (CGFloat)_field.y + _field.height));
        CGFloat h = MAX(1, (bottom - top) * size.height / height);
        anchor = NSMakeRect(left * size.width / width, MIN(size.height - h, size.height - bottom * size.height / height),
                            (right - left) * size.width / width, h);
    }
    NSRect rect = [self convertRect:anchor toView:nil];
    return [self.window convertRectToScreen:rect];
}

- (NSAttributedString *)attributedSubstringForProposedRange:(NSRange)range actualRange:(NSRangePointer)actual {
    if (actual) {
        *actual = NSMakeRange(NSNotFound, 0);
    }
    return nil;
}

- (NSArray<NSAttributedStringKey> *)validAttributesForMarkedText {
    return @[];
}

- (NSUInteger)characterIndexForPoint:(NSPoint)point {
    return NSNotFound;
}
@end

// The everyday trial's ISTESSO_END_WITH_WINDOW: the program ends with its window.
@interface EndWithWindow : NSObject <NSApplicationDelegate>
@end

@implementation EndWithWindow
- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
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
        if (argc == 2 && strcmp(argv[1], "--pasteboard-access") == 0) {
            // Says how macOS lets this application read the general pasteboard (its pasteboard
            // privacy, macOS 15.4 and later) and ends. Reads nothing of the pasteboard: neither
            // its contents nor its types nor its change count. No window, no menu.
            NSPasteboard *board = NSPasteboard.generalPasteboard;
            if (@available(macOS 15.4, *)) {
                long behavior = (long)board.accessBehavior;
                const char *names[] = {"default (asks at the first programmatic read)", "ask", "always allow", "always deny"};
                printf("pasteboard access: %ld %s\n", behavior, behavior >= 0 && behavior <= 3 ? names[behavior] : "(unknown)");
            } else {
                printf("pasteboard access: not reported before macOS 15.4\n");
            }
            return 0;
        }
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
            const char *clipboard = getenv("ISTESSO_CLIPBOARD");
            if (clipboard != NULL && strcmp(clipboard, "1") == 0) {
                clipboard_on = clipboard_start();
                if (!clipboard_on) {
                    fprintf(stderr, "note: copy and paste are off: no socket of this window's own\n");
                }
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

        // The everyday trial's switches (see the top of this file).
        daily_reconnect = getenv("ISTESSO_DAILY_RECONNECT") != NULL;
        BOOL no_window = getenv("ISTESSO_NO_WINDOW") != NULL;
        const char *dock_tile = getenv("ISTESSO_DOCK_TILE");
        const char *pid_file = getenv("ISTESSO_PID_FILE");
        if (pid_file != NULL) {
            FILE *file = fopen(pid_file, "w");
            if (file == NULL) {
                fprintf(stderr, "error: cannot write ISTESSO_PID_FILE\n");
                return 1;
            }
            fprintf(file, "%d\n", (int)getpid());
            fclose(file);
        }
        [NSApplication sharedApplication];
        if (clipboard_on) {
            // With copy and paste on (ISTESSO_CLIPBOARD=1 and its socket made, above): an Edit
            // menu with Paste on Command-V (sent to the window's view: paste:). See
            // performKeyEquivalent: of InputView. The first item is the application's own menu.
            NSMenu *bar = [NSMenu new];
            NSMenuItem *own = [NSMenuItem new];
            own.submenu = [NSMenu new];
            [bar addItem:own];
            NSMenuItem *edit = [NSMenuItem new];
            edit.submenu = [[NSMenu alloc] initWithTitle:@"編集"];
            [edit.submenu addItemWithTitle:@"ペースト" action:@selector(paste:) keyEquivalent:@"v"];
            [bar addItem:edit];
            NSApp.mainMenu = bar;
        }
        if (no_window) {
            [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        } else if (dock_tile != NULL && strcmp(dock_tile, "none") == 0) {
            [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        } else {
            [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        }
        static EndWithWindow *ender = nil;
        if (getenv("ISTESSO_END_WITH_WINDOW") != NULL) {
            ender = [EndWithWindow new];
            NSApp.delegate = ender;
        }
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
                                          styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                                                     (daily_reconnect ? NSWindowStyleMaskResizable : 0))
                                            backing:NSBackingStoreBuffered
                                              defer:NO];
        window.title = [NSString stringWithUTF8String:argv[5]];
        if (getenv("ISTESSO_ON_TOP") != NULL) {
            window.level = NSFloatingWindowLevel;
        }
        const char *command_key = getenv("ISTESSO_COMMAND_KEY");
        command_is_control = command_key == NULL || strcmp(command_key, "super") != 0;
        atexit(secure_input_off_at_exit);
        const char *outside = getenv("ISTESSO_OUTSIDE_FIELD");
        outside_field_keys_through_ime = outside != NULL && strcmp(outside, "ime") == 0;
        const char *preedit = getenv("ISTESSO_PREEDIT");
        force_preedit_strip = preedit != NULL && strcmp(preedit, "strip") == 0;
        const char *text_input = getenv("ISTESSO_TEXT_INPUT");
        Class viewClass = NSView.class;
        if (input_socket >= 0 || channel_address != NULL) {
            viewClass = input_socket >= 0 && text_input != NULL && strcmp(text_input, "1") == 0 ? TextInputView.class : InputView.class;
        }
        NSView *view = [[viewClass alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
        window.contentView = view;
        picture_window = window;
        picture_whole_display = whole_display;
        if (daily_reconnect && !whole_display) {
            // The everyday trial (the Mac host's docs/m4-daily-host.md): when this window becomes
            // the one typed into, the host brings the application's window to the front; when the
            // person has resized it by hand, the host is asked once for a picture of that size.
            [NSNotificationCenter.defaultCenter addObserverForName:NSWindowDidBecomeKeyNotification
                                                            object:window
                                                             queue:NSOperationQueue.mainQueue
                                                        usingBlock:^(NSNotification *note) {
                                                            (void)note;
                                                            send_input("{\"kind\":\"focus\"}");
                                                        }];
            [NSNotificationCenter.defaultCenter addObserverForName:NSWindowDidEndLiveResizeNotification
                                                            object:window
                                                             queue:NSOperationQueue.mainQueue
                                                        usingBlock:^(NSNotification *note) {
                                                            (void)note;
                                                            ask_for_window_size();
                                                        }];
            // For checks that must not touch the window: behave, once the first picture is there,
            // as if the person had resized the window by hand to WxH points.
            const char *pretend = getenv("ISTESSO_PRETEND_RESIZE");
            double pretend_w = 0, pretend_h = 0;
            if (pretend != NULL && sscanf(pretend, "%lfx%lf", &pretend_w, &pretend_h) == 2 && pretend_w > 0 && pretend_h > 0) {
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 4 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                    [window setContentSize:NSMakeSize(pretend_w, pretend_h)];
                    ask_for_window_size();
                });
            }
        }
        if (input_socket >= 0) {
            [window makeFirstResponder:view];
        }
        if (!no_window) {
            [window orderFrontRegardless];
        }
        if (pid_file != NULL) {
            // The everyday trial: the signal USR2 brings the window to the front, for a program
            // started through `open` whose window is asked for again. Without a window it does
            // nothing.
            static dispatch_source_t front;
            signal(SIGUSR2, SIG_IGN);
            front = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGUSR2, 0, dispatch_get_main_queue());
            dispatch_source_set_event_handler(front, ^{
                if (!no_window) {
                    [NSApp activate];
                    [window makeKeyAndOrderFront:nil];
                }
            });
            dispatch_resume(front);
        }
        const char *title_file = getenv("ISTESSO_TITLE_FILE");
        if (title_file != NULL) {
            // The everyday trial: the title says when a window of the application is calling.
            static dispatch_source_t retitle;
            NSString *path = [NSString stringWithUTF8String:title_file];
            signal(SIGUSR1, SIG_IGN);
            retitle = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGUSR1, 0, dispatch_get_main_queue());
            dispatch_source_set_event_handler(retitle, ^{
                NSString *text = [NSString stringWithContentsOfFile:path encoding:NSUTF8StringEncoding error:nil];
                NSArray<NSString *> *lines = [text componentsSeparatedByString:@"\n"];
                if (lines.count == 0 || lines[0].length == 0) {
                    return;
                }
                if (daily_reconnect && counted_window != nil) {
                    counted_title = lines[0];
                    show_counts();
                } else {
                    window.title = lines[0];
                }
                BOOL calling = lines.count > 1 && [lines[1] isEqualToString:@"attention"];
                if (calling && !no_window) {
                    [NSApp requestUserAttention:NSInformationalRequest];
                }
                fprintf(stderr, "title: %s%s\n", lines[0].UTF8String, calling ? " (asks for attention)" : "");
            });
            dispatch_resume(retitle);
            // Said, so that USR1 is sent only to a program that takes it (it would end one that
            // does not).
            fprintf(stderr, "title file: ready\n");
        }
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
            // How often the connections were cut is counted from here, and always shown.
            log_reconnections("start");
            counted_window = window;
            counted_title = [window.title copy];
            show_counts();
            if (input_socket >= 0) {
                watch_relay_state(argv[9]);
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
