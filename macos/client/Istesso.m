// Istesso for the Mac: one application that shows an application of a paired host in a window
// and passes on what is done in that window with keyboard and pointer (docs/adr/0018-own-client.md).
//
//   Istesso --address ADDRESS:PORT [--host NAME] [options]
//
//   --address ADDRESS:PORT    where the host shows its picture (the encrypted channel of
//                             docs/adr/0015-media-transport.md)
//   --host NAME               the paired host, by its name or the beginning of its id. With it,
//                             keyboard and pointer go to that host over the control channel,
//                             from inside this program. Without it the window only shows
//   --input-address A[:PORT]  reach the host's agent at this address only (default: whichever of
//                             its remembered addresses answers)
//   --size WIDTHxHEIGHT       the window's size in points (default: the picture's size)
//   --position X,Y            its top left corner, in points from the top left of the main
//                             display (default: 100,100)
//   --whole-display           cover the display the position lies on, without a title bar
//   --title TEXT              the window's title (default: Istesso)
//   --frame-log FILE          write when frames arrived, were decoded and drawn, and what happened
//                             to the two connections; an existing file is not overwritten
//   --input-timings FILE      write when input was passed on (times and counts, never what it was)
//   --wake NAME[,NAME...]     switches for measuring, each by itself (none is on unless named):
//                               picture-qos       the thread that receives, decodes and shows asks
//                                                 for the highest interactive class
//                               picture-realtime  the same thread is scheduled as the threads of
//                                                 the sound system are
//                               input-qos         the input's thread asks for the highest
//                                                 interactive class
//                               latency-critical  the system is told that this program's work is
//                                                 latency critical for as long as it runs
//   --command-key super       send the Command keys as Super instead of Control
//   --config DIR              this device's identity and list of paired devices (default: where
//                             `istesso` keeps them; ISTESSO_CONFIG_DIR works as well)
//   --on-top                  keep the window above the others
//   For checks that must not touch pointer or keyboard:
//   --check-cursor-at X,Y     draw the host's cursor at this place of the view whatever the
//                             pointer does
//   --check-pretend-typing    behave as if a key were pressed ten times a second, as far as the
//                             line about a cut connection goes; nothing is sent for it
//
// What moved here unchanged from the prototype (macos/gst-viewer): decoding with VideoToolbox on
// the thread that receives, showing the decoder's own surface at once, the cursor's layer, the
// rules for keys (keys.h), what the window does while the picture's connection is cut, and the
// lines of the frame log, so that the measuring tools read both alike. What is new: there is no
// GStreamer and no second program. The input's connection is a thread of this process
// (istesso_client.h); the window hands it events in memory.
//
// The window. Close and minimise act on the window; they reach the host once the session
// messages exist (docs/adr/0018-own-client.md). The size is fixed while the window is open: the
// zoom button is off.
//
// The cursor. A host that names the cursor's shape (arrow, text, pointer, the resize arrows...)
// is shown the Mac's own cursor of that shape; an image is drawn in a layer where the pointer is;
// "hidden" shows nothing.
//
// How often the connections were cut is in the window's title for as long as the window is
// there, and in the frame log: {"kind":"reconnections","t_ns":T,"at":"start"|"cut"|"end",
// "picture":N,"input":M} (M is -1 when no input is sent). The last such line has the counts.
// The other lines are those of the prototype, and: {"kind":"wake",...} with the switches that are
// on, {"kind":"input","t_ns":T,"event":"open"|"cut"|"back"|"over","reconnections":N}.

#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#import <VideoToolbox/VideoToolbox.h>
#import <IOSurface/IOSurface.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#include <errno.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <time.h>
#include "istesso_client.h"
#include "istesso_media.h"

// ---- The switches for measuring (--wake) ----

static bool wake_picture_qos = false, wake_picture_realtime = false, wake_input_qos = false, wake_latency_critical = false;
static id<NSObject> wake_activity = nil;

static FILE *frame_log = NULL;

// Nanoseconds of mach absolute time: the clock ScreenCaptureKit stamps frames with.
static uint64_t now_ns(void) {
    return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

static void log_clock(void) {
    uint64_t before = now_ns();
    struct timespec day;
    clock_gettime(CLOCK_REALTIME, &day);
    uint64_t after = now_ns();
    fprintf(frame_log, "{\"kind\":\"clock\",\"mono_ns\":%llu,\"unix_ns\":%llu}\n",
            (unsigned long long)(before + (after - before) / 2),
            (unsigned long long)day.tv_sec * 1000000000ULL + (unsigned long long)day.tv_nsec);
}


// Whether the next access unit has to be a key frame for the decoder to go on.
static bool vt_needs_key_frame = true;

// ---- Decoding with VideoToolbox ----

static VTDecompressionSessionRef vt_session = NULL;
static CMVideoFormatDescriptionRef vt_format = NULL;
static CALayer *vt_layer = nil;
// Whether the access unit VideoToolbox is decoding is a key frame.
static bool vt_decoding_key_frame = false;
// Whether the stream says its picture is sRGB, as the content of any window is.
static bool vt_srgb = false;

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
    uint64_t decoded = now_ns();
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
    static bool described = false;
    if (!described && frame_log != NULL) {
        // What the picture says about its colours: this is what the window system goes by.
        described = true;
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

static bool vt_start_session(bool all_colour, const char *stream_colours);

static bool vt_start_session(bool all_colour, const char *stream_colours) {
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
        return false;
    }
    VTSessionSetProperty(vt_session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue);
    CFBooleanRef hardware = NULL;
    VTSessionCopyProperty(vt_session, kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder, kCFAllocatorDefault, &hardware);
    bool in_hardware = hardware == kCFBooleanTrue;
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
    return true;
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
// The Mac's own cursor for the shape the host named, when it named one this program knows.
static NSCursor *cursor_named = nil;
// The view that shows the picture, when it is one that knows about the cursor.
static NSView *cursor_view = nil;

@interface NSView (IstessoCursor)
- (void)cursorChanged;
@end

// The Mac's own cursor for a shape by its name, as hosts say it (the names of CSS, with "-" or
// "_"). nil for a name this program does not know: the arrow is shown then.
static NSCursor *cursor_for_name(NSString *name) {
    static NSDictionary<NSString *, NSCursor *> *known = nil;
    if (known == nil) {
        NSMutableDictionary<NSString *, NSCursor *> *table = [@{
            @"default" : NSCursor.arrowCursor,
            @"arrow" : NSCursor.arrowCursor,
            @"left-ptr" : NSCursor.arrowCursor,
            @"text" : NSCursor.IBeamCursor,
            @"xterm" : NSCursor.IBeamCursor,
            @"vertical-text" : NSCursor.IBeamCursorForVerticalLayout,
            @"pointer" : NSCursor.pointingHandCursor,
            @"hand2" : NSCursor.pointingHandCursor,
            @"crosshair" : NSCursor.crosshairCursor,
            @"grab" : NSCursor.openHandCursor,
            @"grabbing" : NSCursor.closedHandCursor,
            @"not-allowed" : NSCursor.operationNotAllowedCursor,
            @"no-drop" : NSCursor.operationNotAllowedCursor,
            @"context-menu" : NSCursor.contextualMenuCursor,
            @"copy" : NSCursor.dragCopyCursor,
            @"alias" : NSCursor.dragLinkCursor,
            @"ew-resize" : NSCursor.resizeLeftRightCursor,
            @"e-resize" : NSCursor.resizeLeftRightCursor,
            @"w-resize" : NSCursor.resizeLeftRightCursor,
            @"col-resize" : NSCursor.resizeLeftRightCursor,
            @"ns-resize" : NSCursor.resizeUpDownCursor,
            @"n-resize" : NSCursor.resizeUpDownCursor,
            @"s-resize" : NSCursor.resizeUpDownCursor,
            @"row-resize" : NSCursor.resizeUpDownCursor,
        } mutableCopy];
        if (@available(macOS 15.0, *)) {
            // The diagonal arrows of a window's corners.
            NSCursor *rising = [NSCursor frameResizeCursorFromPosition:NSCursorFrameResizePositionTopRight
                                                           inDirections:NSCursorFrameResizeDirectionsAll];
            NSCursor *falling = [NSCursor frameResizeCursorFromPosition:NSCursorFrameResizePositionTopLeft
                                                            inDirections:NSCursorFrameResizeDirectionsAll];
            for (NSString *each in @[@"nesw-resize", @"ne-resize", @"sw-resize"]) {
                table[each] = rising;
            }
            for (NSString *each in @[@"nwse-resize", @"nw-resize", @"se-resize"]) {
                table[each] = falling;
            }
        }
        known = table;
    }
    return name == nil ? nil : known[[name.lowercaseString stringByReplacingOccurrencesOfString:@"_" withString:@"-"]];
}

// Takes over what the host said the cursor is. `image` may be NULL; it is released here. `name`
// is what the host called the shape when it sent no image, or nil.
static void show_cursor(uint32_t shape, CGImageRef image, CGSize pixels, CGPoint hot, CGFloat scale, NSString *name) {
    cursor_shape = shape;
    cursor_named = shape == 1 ? cursor_for_name(name) : nil;
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
static uint64_t cut_noticed_ns = 0;
static unsigned long cut_generation = 0, note_generation = 0;
// Presses that were not passed on since the cut, and presses that were passed on in the second
// before it was noticed: whether those arrived, nobody knows.
static unsigned long held_back_count = 0, in_doubt_count = 0;
// When keys and buttons were last pressed and passed on.
static uint64_t press_times[32];
static unsigned long press_count = 0;

@interface NSView (IstessoHeldBack)
- (void)holdBack;
- (void)fallInLine;
@end

static void note_press(void) {
    press_times[press_count++ % 32] = now_ns();
}

static unsigned long presses_since(uint64_t t) {
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
static void picture_was_cut(uint64_t t) {
    if (cut_pending) {
        return;
    }
    cut_pending = YES;
    cut_noticed_ns = t;
    unsigned long generation = ++cut_generation;
    held_back_count = 0;
    in_doubt_count = presses_since(t > 1000000000ULL ? t - 1000000000ULL : 0);
    NSString *line = @"Reconnecting. What you type now does not arrive.";
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
    if (!cut_pending) {
        return;
    }
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
    if (held_back_count > 0 && in_doubt_count > 0) {
        line = [NSString stringWithFormat:@"The connection was cut. %lu input(s) during the cut did not arrive, and %lu just before it "
                                          @"may not have.",
                                          held_back_count, in_doubt_count];
    } else if (held_back_count > 0) {
        line = [NSString stringWithFormat:@"The connection was cut. %lu input(s) during the cut did not arrive.", held_back_count];
    } else {
        line = [NSString stringWithFormat:@"The connection was cut. %lu input(s) just before it may not have arrived.", in_doubt_count];
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
    cut_pending = NO;
    cut_generation++;
    NSString *line;
    switch (why) {
        case ISTESSO_MEDIA_OVER_ENDED: line = @"The session has ended."; break;
        case ISTESSO_MEDIA_OVER_NOT_SHOWN: line = @"The host shows this to another device now."; break;
        case ISTESSO_MEDIA_OVER_SET_ASIDE: line = @"The host has put this session aside."; break;
        case ISTESSO_MEDIA_OVER_NOT_PAIRED: line = @"This device is not paired with the host any more."; break;
        case ISTESSO_MEDIA_OVER_FORBIDDEN: line = @"This device may not look at the host."; break;
        case ISTESSO_MEDIA_OVER_UNSUPPORTED: line = @"The host sends its picture in a form this program cannot show."; break;
        default: line = @"The host does not show its picture to this device."; break;
    }
    mark_the_picture(line);
    log_held_back(YES);
}

// ---- How often the connections were cut, where a person sees it ----
//
// The window's title says it for as long as the window is there: "... picture 0, input 0". Both
// counts are this program's own. Without --host no input is sent, and the title says so.

static atomic_ulong picture_cuts;
// The input's count; -1 when no input is sent.
static atomic_long input_cuts = -1;
// 0 not open yet, 1 open or being opened again, 2 refused. Only touched on the main thread.
static int input_stands = 0;
static NSWindow *counted_window = nil;
static NSString *counted_title = nil;

// A measuring build says that it is one, wherever it says who it is: it has a local way in for
// the measurement's keys, which a build that is handed out does not contain at all (ADR 0018).
#ifdef ISTESSO_MEASURING
#define MEASURING_TITLE "[measuring build] "
#define MEASURING_HEADER ",\"measuring_build\":true"
static IstessoMeasureKeys *measure_keys = NULL;
#else
#define MEASURING_TITLE ""
#define MEASURING_HEADER ""
#endif

static void log_reconnections(const char *at) {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"reconnections\",\"t_ns\":%llu,\"at\":\"%s\",\"picture\":%lu,\"input\":%ld}\n",
                (unsigned long long)now_ns(), at, atomic_load(&picture_cuts), atomic_load(&input_cuts));
    }
}

// On the main thread.
static void show_counts(void) {
    if (counted_window == nil) {
        return;
    }
    long cuts = atomic_load(&input_cuts);
    NSString *input = cuts < 0 ? @"input off"
        : input_stands == 2 ? @"input refused"
        : input_stands == 0 ? @"input not open yet"
        : [NSString stringWithFormat:@"input %ld", cuts];
    counted_window.title =
        [NSString stringWithFormat:@MEASURING_TITLE "%@ — reconnections: picture %lu, %@", counted_title, atomic_load(&picture_cuts), input];
}

// ---- Frames from the encrypted channel ----
//
// One thread connects to the host, is handed whole frames by the library, and decodes each one
// there and then, as the streaming thread of the GStreamer source does on the other way.

// Where the host listens: ISTESSO_CHANNEL. NULL when the frames come from GStreamer.
static const char *channel_address = NULL;
// The paired host, when one was named, and where this device keeps its identity (NULL: the usual place).
static const char *host_name = NULL, *config_dir = NULL;
static bool check_pretend_typing = false;
static void show_the_window(void);
// The parameter sets VideoToolbox was last set up with: VPS, SPS, PPS.
static uint8_t *channel_sets[3];
static size_t channel_set_sizes[3];
static IstessoMediaStreamInfo channel_info;
static uint64_t channel_asked_ns = 0;

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
static bool channel_configure(const NalUnit *sets[3]) {
    bool same = vt_session != NULL;
    for (int i = 0; i < 3 && same; i++) {
        same = channel_set_sizes[i] == sets[i]->size && memcmp(channel_sets[i], sets[i]->bytes, sets[i]->size) == 0;
    }
    if (same) {
        return true;
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
        return false;
    }
    CMVideoDimensions size = CMVideoFormatDescriptionGetDimensions(vt_format);
    picture_width = size.width;
    picture_height = size.height;
    vt_srgb = channel_info.colour == 1;
    return vt_start_session(channel_info.chroma == 444, vt_srgb ? "srgb" : "-");
}

static void channel_ask_for_key_frame(IstessoMediaWatch *watch, uint64_t t) {
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
        exit(1);
    }
    if (frame->discontinuity && !frame->key_frame) {
        vt_needs_key_frame = true;  // The host left frames out and went on without a key frame.
    }
    if (vt_session == NULL || total == 0 || (vt_needs_key_frame && !frame->key_frame)) {
        vt_needs_key_frame = true;
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
        bool fits = cursor->width >= 1 && cursor->width <= 256 && cursor->height >= 1 && cursor->height <= 256
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
    // The name lives in the library only until the cursor changes again.
    NSString *name = cursor->shape == 1 && cursor->name != NULL ? [NSString stringWithUTF8String:cursor->name] : nil;
    dispatch_async(dispatch_get_main_queue(), ^{
        show_cursor(shape, image, pixels, hot, scale, name);
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
    pthread_setname_np("istesso picture");
    if (wake_picture_qos) {
        // Work a person is waiting for: the highest class there is short of real time.
        int refused = pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
        if (frame_log != NULL) {
            fprintf(frame_log, "{\"kind\":\"scheduling\",\"interactive\":%d}\n", refused == 0 ? 1 : 0);
        }
    }
    if (wake_picture_realtime) {
        // Work once per frame (85 a second), a few milliseconds of it, to be done well within the frame.
        int agreed = istesso_media_thread_real_time(11764706, 3000000, 6000000);
        if (frame_log != NULL) {
            fprintf(frame_log, "{\"kind\":\"scheduling\",\"real_time\":%d}\n", agreed);
        }
        if (!agreed) {
            fprintf(stderr, "note: the receiving thread was not given real-time scheduling\n");
        }
    }
    IstessoMediaWatch *watch = istesso_media_watch_open(config_dir, channel_address, host_name);
    if (watch == NULL) {
        fprintf(stderr, "error: %s\n", istesso_media_last_error());
        exit(2);
    }
    uint64_t began = now_ns(), cut_at = 0;
    bool ever = false, said_unreachable = false, back_pending = false, shown = false;
    for (;;) {
        IstessoMediaFrame frame;
        int32_t got = istesso_media_watch_next(watch, 250, &frame);
        if (got == 1) {
            on_channel_frame(watch, &frame);
            if (!shown && !vt_needs_key_frame) {
                // The first picture is on the layer: the window is shown from here on.
                shown = true;
                dispatch_async(dispatch_get_main_queue(), ^{
                    show_the_window();
                });
            }
            if (back_pending && !vt_needs_key_frame) {
                // The first picture after the cut has been decoded and handed to the layer.
                back_pending = false;
                uint64_t t = now_ns();
                if (frame_log != NULL) {
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
        } else if (got == 3) {
            // Cut, and nobody said why: the library is connecting again.
            cut_at = now_ns();
            back_pending = true;
            atomic_fetch_add(&picture_cuts, 1);
            fprintf(stderr, "note: the picture's connection was cut; connecting again\n");
            log_stream("ended");
            log_reconnections("cut");
            uint64_t t = cut_at;
            dispatch_async(dispatch_get_main_queue(), ^{
                show_counts();
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
            ever = true;
            if (frame_log != NULL) {
                fprintf(frame_log, "{\"kind\":\"stream\",\"t_ns\":%llu,\"event\":\"connected\",\"again\":%u,\"silent_ms\":%llu}\n",
                        (unsigned long long)now_ns(), state.again, (unsigned long long)state.silent_ms);
            }
            // A new connection begins with a key frame, and the decoder begins anew with it.
            vt_needs_key_frame = true;
            channel_asked_ns = 0;
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
                picture_is_over(why);
            });
            return NULL;
        } else if (!ever && !said_unreachable && now_ns() - began > 1000000000ULL) {
            // Said once; the sentence holds no address.
            said_unreachable = true;
            fprintf(stderr, "note: no picture from the host yet; trying again\n");
            log_stream("unreachable");
        }
    }
    return NULL;
}

// Shows the frames of the encrypted channel in the layer of `view`.
static bool use_channel(NSView *view) {
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
    if (check_pretend_typing) {
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
    pthread_t thread;
    return pthread_create(&thread, NULL, channel_thread, NULL) == 0;
}

// The input's connection, when a host was named: a thread of the library owns it.
static IstessoInput *input = NULL;

// ---- The frame log ----

static bool start_frame_log(const char *path) {
    frame_log = fopen(path, "wx");  // Never overwrite an earlier measurement.
    if (frame_log == NULL) {
        fprintf(stderr, "error: cannot create %s: %s\n", path, strerror(errno));
        return false;
    }
    struct timespec day;
    clock_gettime(CLOCK_REALTIME, &day);
    fprintf(frame_log, "{\"kind\":\"header\",\"side\":\"client\",\"tool\":\"istesso-client\",\"version\":\"0.0.1\"" MEASURING_HEADER ",\"started_unix_ms\":%llu}\n",
            (unsigned long long)day.tv_sec * 1000ULL + (unsigned long long)day.tv_nsec / 1000000ULL);
    fprintf(frame_log, "{\"kind\":\"wake\",\"picture_qos\":%d,\"picture_realtime\":%d,\"input_qos\":%d,\"latency_critical\":%d}\n",
            wake_picture_qos, wake_picture_realtime, wake_input_qos, wake_latency_critical);
    log_clock();
    // The log reaches the disk twice a second, and once more when the program ends.
    static dispatch_source_t timer;
    timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 2), NSEC_PER_SEC / 2, NSEC_PER_SEC / 20);
    dispatch_source_set_event_handler(timer, ^{
        log_clock();
        fflush(frame_log);
    });
    dispatch_resume(timer);
    return true;
}

// The program ends: the input's conversation is closed (the host lets go of what was down), and
// the log says how often the connections were cut.
static void finish(void) {
    static bool finished = false;
    if (finished) {
        return;
    }
    finished = true;
#ifdef ISTESSO_MEASURING
    if (measure_keys != NULL) {
        // Before the input: nothing may be handed to an input that is closed.
        istesso_measure_keys_close(measure_keys);
        measure_keys = NULL;
    }
#endif
    if (input != NULL) {
        IstessoInput *closing = input;
        input = NULL;
        istesso_input_close(closing);
    }
    if (frame_log != NULL) {
        log_clock();
        log_reconnections("end");
        fflush(frame_log);
    }
}

// ---- Passing on what the person does in the window ----
//
// The window's thread hands the events to the input's thread of the library, in memory, and goes
// on. It never touches the connection.

static void send_input(const IstessoInputEvent *events, size_t count) {
    if (input != NULL && count > 0) {
        istesso_input_send(input, events, count);
    }
}

static void send_one(uint32_t kind, int32_t a, int32_t b) {
    IstessoInputEvent event = {kind, a, b};
    send_input(&event, 1);
}

// Called on the input's thread for what happens to its conversation with the host.
static void on_input_told(void *context, int32_t event, uint64_t cuts, double after_ms, const char *why) {
    static const char *const names[] = {"", "open", "cut", "back", "over"};
    const char *name = event >= 1 && event <= 4 ? names[event] : "other";
    atomic_store(&input_cuts, (long)cuts);
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"input\",\"t_ns\":%llu,\"event\":\"%s\",\"reconnections\":%llu,\"after_ms\":%.1f}\n",
                (unsigned long long)now_ns(), name, (unsigned long long)cuts, after_ms);
    }
    if (event == ISTESSO_INPUT_CUT) {
        fprintf(stderr, "note: the input's conversation with the host was cut; opening it again\n");
    } else if (event == ISTESSO_INPUT_OVER) {
        // The sentence holds no address and no key.
        fprintf(stderr, "note: the host does not take input from here: %s\n", why != NULL ? why : "");
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        input_stands = event == ISTESSO_INPUT_OVER ? 2 : 1;
        show_counts();
    });
}

// Which keys go to the other side for what happens on the keyboard: keys.h, tested by keys-test.c.
#include "../gst-viewer/keys.h"

// Whether the Command keys go to the other side as Control (the default) or as the Super
// ("Windows") keys. ISTESSO_COMMAND_KEY=super in the environment sends Super.
static BOOL command_is_control = YES;

// The view the picture is shown in, when input is passed on: it takes the keyboard and the
// pointer and writes what happens to the socket.
@interface InputView : NSView
@end

@implementation InputView {
    Keys _keys;          // keys that are down on the other side
    unsigned _buttons;   // buttons that are down on the other side (bit 0: left, 1: right, 2: other)
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
    if (cursor_layer != nil && cursor_shape != 1 && !picture_stale) {
        [self addCursorRect:self.bounds cursor:empty_cursor()];
    } else if (cursor_named != nil && !picture_stale) {
        // The host named the shape: the Mac's own cursor of that shape.
        [self addCursorRect:self.bounds cursor:cursor_named];
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
    // What one event of the keyboard caused leaves together, in its order.
    IstessoInputEvent events[256];
    for (size_t i = 0; i < count && i < 256; i++) {
        events[i] = (IstessoInputEvent){ISTESSO_INPUT_KEY, (int32_t)changes[i].code, changes[i].pressed ? 1 : 0};
    }
    send_input(events, count < 256 ? count : 256);
}

// The window is no longer the one that is typed into: no key stays down on the other side.
- (void)letGo {
    KeyChange changes[256];
    [self send:changes count:keys_let_go(&_keys, changes)];
}

// The line that says "what you type does not arrive" has come up: whatever is held on the other
// side is let go there, keys and buttons. From here until the picture is back nothing is pressed.
- (void)holdBack {
    [self letGo];
    for (unsigned i = 0; i < 3; i++) {
        if (_buttons & (1u << i)) {
            send_one(ISTESSO_INPUT_BUTTON, (int32_t)(0x110 + i), 0);
        }
    }
    _buttons = 0;
}

// The picture is back: the modifiers held on this keyboard are held on the other side again.
// Buttons stay up there; one that is still held here counts as not held until it is let go
// (pressing it over there now would begin a click or a drag where the pointer happens to be).
- (void)fallInLine {
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    [self send:changes count:keys_fall_in_line(&_keys, NSEvent.modifierFlags, changes)];
}

- (void)keyDown:(NSEvent *)event {
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    size_t count = keys_key_down(&_keys, event.keyCode, event.modifierFlags, event.isARepeat, changes);
    if (picture_stale) {
        // Not pressed on the other side while the picture is not the host's of this moment.
        bool dropped = false;
        count = keys_hold_back(&_keys, changes, count, &dropped);
        held_back_count += dropped ? 1 : 0;
    } else if (count > 0) {
        note_press();
    }
    [self send:changes count:count];
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
    send_one(ISTESSO_INPUT_POINTER_AT, x, y);
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
    _buttons = pressed ? (_buttons | bit) : (_buttons & ~bit);
    if (pressed) {
        note_press();
    }
    send_one(ISTESSO_INPUT_BUTTON, (int32_t)code, pressed ? 1 : 0);
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
    if (picture_stale) {
        // Scrolling a picture that is not there moves something nobody sees.
        _scroll[0] = _scroll[1] = 0;
        return;
    }
    if (dx != 0 || dy != 0) {
        _scroll[0] -= dx;
        _scroll[1] -= dy;
        send_one(ISTESSO_INPUT_SCROLL, dx, dy);
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


// ---- The application ----

static NSWindow *the_window = nil;
static bool window_whole_display = false;
static bool window_sized = false;  // --size was given
static NSPoint window_at;          // top left, in points from the top left of the main display
static NSRect window_display;      // the display to cover, with --whole-display

// The first picture has been decoded: the window is given the picture's size, unless one was
// asked for, and shown.
static void show_the_window(void) {
    if (the_window == nil || the_window.visible) {
        return;
    }
    double main_height = NSScreen.screens.firstObject.frame.size.height;
    if (window_whole_display) {
        [the_window setFrame:window_display display:YES];
    } else {
        if (!window_sized && picture_width > 0 && picture_height > 0) {
            // One pixel of the picture to one pixel of the display the window goes to.
            CGFloat scale = 1;
            NSPoint corner = NSMakePoint(window_at.x + 1, main_height - window_at.y - 1);
            for (NSScreen *screen in NSScreen.screens) {
                if (NSPointInRect(corner, screen.frame)) {
                    scale = screen.backingScaleFactor;
                }
            }
            [the_window setContentSize:NSMakeSize(picture_width / scale, picture_height / scale)];
        }
        // Moved after it has its size: coordinates count from the top left of the main display.
        [the_window setFrameTopLeftPoint:NSMakePoint(window_at.x, main_height - window_at.y)];
    }
    [the_window makeKeyAndOrderFront:nil];
    [the_window orderFrontRegardless];
}

@interface IstessoApplication : NSObject <NSApplicationDelegate, NSWindowDelegate>
@end

@implementation IstessoApplication

// The red button. Until the host can be asked to close the application it shows (the session
// messages of docs/adr/0018-own-client.md), closing the window leaves the session: the picture's
// and the input's connections end with it, and the host lets go of every key.
- (BOOL)windowShouldClose:(NSWindow *)sender {
    return YES;
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

- (void)applicationWillTerminate:(NSNotification *)notification {
    finish();
}

// The yellow button puts the window away as it does any window. The view lets go of the keys when
// the window is no longer the one typed into; the picture goes on arriving, so that the window
// is up to date the moment it comes back. (Asking the host to stop the picture meanwhile needs
// the session messages.)
- (void)windowDidMiniaturize:(NSNotification *)notification {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"window\",\"t_ns\":%llu,\"event\":\"minimised\"}\n", (unsigned long long)now_ns());
    }
}

- (void)windowDidDeminiaturize:(NSNotification *)notification {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"window\",\"t_ns\":%llu,\"event\":\"restored\"}\n", (unsigned long long)now_ns());
    }
}

@end

static void usage(void) {
    fprintf(stderr,
            "usage: Istesso --address ADDRESS:PORT [--host NAME] [--input-address ADDRESS[:PORT]] [--size WxH] [--position X,Y]\n"
            "               [--whole-display] [--title TEXT] [--frame-log FILE] [--input-timings FILE] [--wake NAME,...]\n"
            "               [--command-key super] [--config DIR] [--on-top]\n"
#ifdef ISTESSO_MEASURING
            "               [--measure-keys FOLDER]   (a measuring build: keys written to FOLDER/measure-keys.sock go to the host)\n"
#endif
            );
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        const char *title = "Istesso", *frame_log_path = NULL, *timings_path = NULL, *input_address = NULL;
#ifdef ISTESSO_MEASURING
        const char *measure_keys_dir = NULL;
#endif
        double width = 0, height = 0;
        bool on_top = false;
        window_at = NSMakePoint(100, 100);
        for (int i = 1; i < argc; i++) {
            const char *name = argv[i];
            // What the system adds when it starts an application.
            if (strncmp(name, "-psn_", 5) == 0) {
                continue;
            }
            if (strcmp(name, "--whole-display") == 0) {
                window_whole_display = true;
                continue;
            }
            if (strcmp(name, "--on-top") == 0) {
                on_top = true;
                continue;
            }
            if (strcmp(name, "--check-pretend-typing") == 0) {
                check_pretend_typing = true;
                continue;
            }
            if (i + 1 >= argc) {
                usage();
                return 2;
            }
            const char *value = argv[++i];
            if (strcmp(name, "--address") == 0) {
                channel_address = value;
            } else if (strcmp(name, "--host") == 0) {
                host_name = value;
            } else if (strcmp(name, "--input-address") == 0) {
                input_address = value;
            } else if (strcmp(name, "--size") == 0) {
                if (sscanf(value, "%lfx%lf", &width, &height) != 2 || width < 1 || height < 1) {
                    fprintf(stderr, "error: --size is WIDTHxHEIGHT, in points\n");
                    return 2;
                }
                window_sized = true;
            } else if (strcmp(name, "--position") == 0) {
                if (sscanf(value, "%lf,%lf", &window_at.x, &window_at.y) != 2) {
                    fprintf(stderr, "error: --position is X,Y, in points from the top left of the main display\n");
                    return 2;
                }
            } else if (strcmp(name, "--title") == 0) {
                title = value;
            } else if (strcmp(name, "--frame-log") == 0) {
                frame_log_path = value;
            } else if (strcmp(name, "--input-timings") == 0) {
                timings_path = value;
            } else if (strcmp(name, "--config") == 0) {
                config_dir = value;
#ifdef ISTESSO_MEASURING
            } else if (strcmp(name, "--measure-keys") == 0) {
                measure_keys_dir = value;
#endif
            } else if (strcmp(name, "--command-key") == 0) {
                command_is_control = strcmp(value, "super") != 0;
            } else if (strcmp(name, "--check-cursor-at") == 0) {
                double pin_x = 0, pin_y = 0;
                if (sscanf(value, "%lf,%lf", &pin_x, &pin_y) != 2) {
                    usage();
                    return 2;
                }
                cursor_pinned = YES;
                cursor_pin = NSMakePoint(pin_x, pin_y);
            } else if (strcmp(name, "--wake") == 0) {
                char *names = strdup(value), *rest = names, *each;
                while ((each = strsep(&rest, ",")) != NULL) {
                    if (strcmp(each, "picture-qos") == 0) {
                        wake_picture_qos = true;
                    } else if (strcmp(each, "picture-realtime") == 0) {
                        wake_picture_realtime = true;
                    } else if (strcmp(each, "input-qos") == 0) {
                        wake_input_qos = true;
                    } else if (strcmp(each, "latency-critical") == 0) {
                        wake_latency_critical = true;
                    } else if (*each != '\0') {
                        fprintf(stderr, "error: --wake knows picture-qos, picture-realtime, input-qos and latency-critical, not %s\n", each);
                        return 2;
                    }
                }
                free(names);
            } else {
                usage();
                return 2;
            }
        }
        if (channel_address == NULL) {
            usage();
            return 2;
        }

        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        static IstessoApplication *application;
        application = [IstessoApplication new];
        NSApp.delegate = application;
        if (wake_latency_critical) {
            // Kept until the program ends: timers and wake-ups of this process are not put off.
            wake_activity = [NSProcessInfo.processInfo beginActivityWithOptions:(NSActivityUserInitiated | NSActivityLatencyCritical)
                                                                       reason:@"Showing an application of another computer"];
        }

        if (window_whole_display) {
            // The display the point lies on. Screens count from the bottom left of the main one.
            double main_height = NSScreen.screens.firstObject.frame.size.height;
            NSPoint point = NSMakePoint(window_at.x + 1, main_height - window_at.y - 1);
            window_display = NSZeroRect;
            for (NSScreen *screen in NSScreen.screens) {
                if (NSPointInRect(point, screen.frame)) {
                    window_display = screen.frame;
                }
            }
            if (NSIsEmptyRect(window_display)) {
                fprintf(stderr, "error: --position is on no display\n");
                return 2;
            }
            width = window_display.size.width;
            height = window_display.size.height;
            // While this is the active application the menu bar and the Dock stay away, so that
            // the picture has the whole display. They come back when another application is.
            NSApp.presentationOptions = NSApplicationPresentationHideDock | NSApplicationPresentationHideMenuBar;
        } else if (!window_sized) {
            // Until the host has said how large its picture is.
            width = 860;
            height = 360;
        }
        NSWindow *window = window_whole_display
            ? [[WholeDisplayWindow alloc] initWithContentRect:NSMakeRect(0, 0, width, height)
                                                    styleMask:NSWindowStyleMaskBorderless
                                                      backing:NSBackingStoreBuffered
                                                        defer:NO]
            : [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, width, height)
                                          styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable)
                                            backing:NSBackingStoreBuffered
                                              defer:NO];
        window.title = [NSString stringWithUTF8String:title];
        window.delegate = application;
        window.releasedWhenClosed = NO;
        // The size is fixed while the window is open.
        [window standardWindowButton:NSWindowZoomButton].enabled = NO;
        if (on_top) {
            window.level = NSFloatingWindowLevel;
        }
        InputView *view = [[InputView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
        window.contentView = view;
        [window makeFirstResponder:view];
        the_window = window;

        if (frame_log_path != NULL && !start_frame_log(frame_log_path)) {
            return 1;
        }
        // Asked to stop from outside: as when the person quits.
        static dispatch_source_t stop[2];
        int signals[2] = {SIGTERM, SIGINT};
        for (int i = 0; i < 2; i++) {
            signal(signals[i], SIG_IGN);
            stop[i] = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, signals[i], 0, dispatch_get_main_queue());
            dispatch_source_set_event_handler(stop[i], ^{
                finish();
                exit(0);
            });
            dispatch_resume(stop[i]);
        }

        if (host_name != NULL) {
            // Keyboard and pointer go to the host from a thread of this process.
            atomic_store(&input_cuts, 0);
            input = istesso_input_open(config_dir, host_name, input_address, timings_path,
                                       wake_input_qos ? ISTESSO_INPUT_INTERACTIVE : 0, on_input_told, NULL);
            if (input == NULL) {
                fprintf(stderr, "error: %s\n", istesso_client_last_error());
                return 1;
            }
        }
#ifdef ISTESSO_MEASURING
        if (measure_keys_dir != NULL) {
            if (input == NULL) {
                fprintf(stderr, "error: --measure-keys needs --host: there is no input to hand the keys to\n");
                return 1;
            }
            // Not started without it: a measurement would begin with nowhere to write its keys.
            measure_keys = istesso_measure_keys_open(input, measure_keys_dir);
            if (measure_keys == NULL) {
                fprintf(stderr, "error: the measurement's keys: %s\n", istesso_client_last_error());
                istesso_input_close(input);
                return 1;
            }
            if (frame_log != NULL) {
                fprintf(frame_log, "{\"kind\":\"measure_keys\",\"t_ns\":%llu}\n", (unsigned long long)now_ns());
            }
        }
#endif
        // How often the connections were cut is counted from here, and always shown.
        log_reconnections("start");
        counted_window = window;
        counted_title = [window.title copy];
        show_counts();
        if (!use_channel(view)) {
            fprintf(stderr, "error: cannot start the thread that receives\n");
            return 1;
        }
        [NSApp run];
        finish();
    }
    return 0;
}
