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
//                               awake-spin        one thread keeps a fast core busy while the window
//                                                 is in front and input is going on (awake.h)
//                               awake-tick        the same, with a thread that wakes a thousand
//                                                 times a second instead of one that never rests
//                               awake-session     with one of the two: from start to end, whatever
//                                                 the window and the person do
//                               usage             nothing is switched: the frame log says at start
//                                                 and end what the process used (processor time,
//                                                 energy as the system bills it), at the end also
//                                                 for each thread, and when the window was in front
//                                                 (usage.h)
//                               front             the application asks to be the active one and its
//                                                 window the one that takes the keys, at the start
//                                                 and again whenever it is not, and the log says
//                                                 what came of it
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
// on, {"kind":"input","t_ns":T,"event":"open"|"cut"|"back"|"over","reconnections":N}, and with
// --wake awake-spin or awake-tick {"kind":"awake","t_ns":T,"on":1|0} for when a core was kept
// awake. With those, with usage and with front: {"kind":"awake","t_ns":T,"in_front":1|0,
// "active":1|0,"key":1|0,"visible":1|0,"role":N} whenever one of them changes (in front: the
// application is the active one and the window takes the keys; visible: the window is not
// wholly covered; role: usage.h). With front: {"kind":"front","t_ns":T,"event":"lost"|"asked"|
// "granted"|"refused"|"gave_up","how":"activate"|"activate_ignoring_others"}.

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
#include <unistd.h>
#include "awake.h"
#include "usage.h"
#include "istesso_client.h"
#include "istesso_media.h"

// ---- The switches for measuring (--wake) ----

static bool wake_picture_qos = false, wake_picture_realtime = false, wake_input_qos = false, wake_latency_critical = false;
#ifdef ISTESSO_MEASURING
// The thread that takes the measurement's keys stands where the window's thread does for a
// person's keys; this gives it that thread's scheduling.
static bool wake_keys_qos = false;
#endif
static id<NSObject> wake_activity = nil;

static FILE *frame_log = NULL;

// In what way an attempt to reach the host failed (ISTESSO_MEDIA_FAILURE_*), as the log says it.
static const char *failure_word(uint32_t failure) {
    static const char *const words[] = {"none", "refused", "unreachable", "timed_out", "no_answer", "not_ready", "other"};
    return failure < sizeof words / sizeof *words ? words[failure] : "other";
}

static uint64_t now_ns(void);

// While the picture has no connection: writes to the log in what way the host cannot be
// reached, once for each way. "refused" (the machine answered, nothing listens) is not
// "unreachable" (no way to it from this Mac). Called when no frame came, never for a frame.
static void say_the_way(IstessoMediaWatch *watch, uint32_t *said) {
    IstessoMediaWatchState stands;
    memset(&stands, 0, sizeof stands);
    istesso_media_watch_state(watch, &stands);
    if (stands.failure == *said) {
        return;
    }
    *said = stands.failure;
    if (stands.failure != 0 && frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"stream\",\"t_ns\":%llu,\"event\":\"not_yet\",\"why\":\"%s\"}\n",
                (unsigned long long)now_ns(), failure_word(stands.failure));
    }
}

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

// ---- What the process used (--wake usage; usage.h) ----
//
// Switches nothing. The frame log says at start and end what the system counts for the process,
// at the end also for each of its threads and on which cores the picture's thread was when a
// frame arrived and when it was decoded; and, as with the switches below, when the window was
// the one in front.
static bool wake_usage = false;

// ---- Staying in front (--wake front) ----
//
// A measuring switch: the application asks to be the active one, and its window the one that
// takes the keys, at the start and again whenever it stops being that, for the whole run. The
// system may refuse an application that asks for itself; the log says what was asked and what
// came of it. Not for a program that is handed out: it takes the front from whatever the
// person turned to.
static bool wake_front = false;
static bool front_over = false;  // the program is ending: nothing is asked any more

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

// ---- When a connection is cut ----
//
// What the window shows while the picture is not the host's picture of this moment, or while
// what is typed does not reach the host although the picture runs, and what is passed on of the
// keyboard and the pointer meanwhile: nobody is to type into a window that does not pass it on
// without knowing (ADR 0015, ADR 0018). Only touched on the main thread.

static CALayer *dim_layer = nil;
static CALayer *note_box = nil;
static CATextLayer *note_layer = nil;
// A small line that stays, at the bottom left: this device may look at the host and not type.
// The picture is not dimmed for it: looking is what the device is there for.
static CALayer *standing_box = nil;
static CATextLayer *standing_layer = nil;
static BOOL view_only = NO;
// The small line is made noticeable for a moment when a key or a button is pressed.
static BOOL standing_loud = NO;
static unsigned long standing_generation = 0;
// No picture has been shown yet: the window is up and says so.
static BOOL before_first_picture = YES;
// The line is up: no key or button press is passed on.
static BOOL picture_stale = NO;
// The connection was cut and the picture is not back yet.
static BOOL cut_pending = NO;
// The picture's connection is over for good: its line stays whatever the input does.
static BOOL picture_over = NO;
// The input's conversation is not there (not open yet, or cut) and is being asked for.
static BOOL input_away = NO, input_was_open = NO;
static uint64_t away_noticed_ns = 0;
static unsigned long away_generation = 0;
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

// Shows the standing line while the device may only look and there is a picture to look at.
static void show_standing_note(void) {
    BOOL on = view_only && !picture_over && cursor_view != nil && standing_box != nil;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (on) {
        NSString *text = @"View only — what you type is not sent";
        CGFloat points = standing_loud ? 15 : 11;
        NSFont *font = [NSFont systemFontOfSize:points weight:standing_loud ? NSFontWeightSemibold : NSFontWeightMedium];
        NSSize fit = [text sizeWithAttributes:@{NSFontAttributeName : font}];
        CGFloat width = ceil(fit.width) + 20, height = ceil(fit.height) + 10;
        // The layer counts from the bottom left, as the view does.
        standing_box.frame = CGRectMake(10, 10, width, height);
        standing_layer.frame = CGRectMake(10, 5, width - 20, height - 10);
        standing_layer.font = (__bridge CFTypeRef)font;
        standing_layer.fontSize = points;
        standing_box.backgroundColor = CGColorCreateGenericGray(0.08, standing_loud ? 0.96 : 0.82);
        standing_box.borderColor = CGColorGetConstantColor(kCGColorWhite);
        standing_box.borderWidth = standing_loud ? 1.5 : 0;
        standing_layer.contentsScale = cursor_view.window != nil ? cursor_view.window.backingScaleFactor : 2;
        standing_layer.string = text;
    }
    standing_box.hidden = !on;
    [CATransaction commit];
}

// A key or a button was pressed on a device that may only look: the small line is larger and
// framed for a moment, so that the person sees why nothing happens, and then small again.
static void notice_standing_note(void) {
    if (!view_only || picture_over) {
        return;
    }
    unsigned long generation = ++standing_generation;
    if (!standing_loud) {
        standing_loud = YES;
        show_standing_note();
    }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(1.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        if (generation == standing_generation) {
            standing_loud = NO;
            show_standing_note();
        }
    });
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

// What the line says while something is being made again.
static NSString *away_line(void) {
    if (before_first_picture) {
        return @"Connecting. What you type now does not arrive.";
    }
    return input_away && !input_was_open && !cut_pending
        ? @"Input is not available yet. What you type now does not arrive."
        : @"Reconnecting. What you type now does not arrive.";
}

// The connection was cut and is being made again.
static void picture_was_cut(uint64_t t) {
    if (cut_pending) {
        return;
    }
    cut_pending = YES;
    cut_noticed_ns = t;
    unsigned long generation = ++cut_generation;
    if (picture_stale) {
        // The line is up already, for the input: it stays up, and the counts go on.
        show_note(away_line());
        return;
    }
    if (!input_away) {
        held_back_count = 0;
        in_doubt_count = presses_since(t > 1000000000ULL ? t - 1000000000ULL : 0);
    }
    NSString *line = away_line();
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

static void take_the_line_down(void);

// The first picture after a cut is on the screen.
static void picture_is_back(void) {
    if (!cut_pending) {
        return;
    }
    cut_pending = NO;
    cut_generation++;
    if (input_away) {
        // The picture is the host's again, but what is typed still does not arrive: a line
        // that is up stays up (one that is not is the input's own to bring up).
        if (picture_stale) {
            show_note(away_line());
        }
        return;
    }
    take_the_line_down();
}

// Picture and input are both there again.
static void take_the_line_down(void) {
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
    picture_over = YES;
    show_standing_note();  // The reason the picture ended is what the window says now.
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

#ifdef ISTESSO_CHECKS
// ---- For looking, without a person at the keyboard ----
//
// Only in an executable built for a check (-DISTESSO_CHECKS=1): neither the application nor the
// measuring build is built with it. A key press cannot be put into another program's window
// without the system's permission to control the computer, and a check must not ask for it. So
// here the application presses for itself: it makes the event a key press is and hands it to
// its own view's key handler, the one the window server would call. Whatever the handler
// passes on goes the usual way. And it can show its own menu inside its window.
static double check_press_when_cut_ms = -1, check_menu_at_s = -1;
static double check_press_at_s[8];
static int check_presses = 0;

// The A key goes down in the window and comes up 40 ms later.
static void check_press(void) {
    NSView *view = cursor_view;
    if (view == nil || ![view respondsToSelector:@selector(holdBack)]) {
        return;
    }
    NSEvent * (^key)(NSEventType) = ^NSEvent *(NSEventType type) {
        return [NSEvent keyEventWithType:type
                                location:NSZeroPoint
                           modifierFlags:0
                               timestamp:NSProcessInfo.processInfo.systemUptime
                            windowNumber:view.window.windowNumber
                                 context:nil
                              characters:@"a"
             charactersIgnoringModifiers:@"a"
                               isARepeat:NO
                                 keyCode:0];
    };
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"check\",\"t_ns\":%llu,\"event\":\"press\",\"line_up\":%d,\"input_away\":%d,\"view_only\":%d}\n",
                (unsigned long long)now_ns(), picture_stale ? 1 : 0, input_away ? 1 : 0, view_only ? 1 : 0);
    }
    [view keyDown:key(NSEventTypeKeyDown)];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 40 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        [view keyUp:key(NSEventTypeKeyUp)];
    });
}

// The application's own menu, opened inside the window for a second and a half.
static void check_show_menu(void) {
    NSMenu *menu = NSApp.mainMenu.itemArray.firstObject.submenu;
    NSView *view = cursor_view;
    if (menu == nil || view == nil) {
        return;
    }
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"check\",\"t_ns\":%llu,\"event\":\"menu\",\"items\":%ld,\"first\":\"%s\",\"shortcut\":\"%s\"}\n",
                (unsigned long long)now_ns(), (long)menu.numberOfItems, menu.itemArray.firstObject.title.UTF8String,
                menu.itemArray.firstObject.keyEquivalent.UTF8String);
    }
    // Closed again by a timer of the run loop's common modes: while a menu is open the main
    // thread is inside the menu's own loop, and what is queued for the main queue waits until
    // the menu is closed (the first form of this check waited there for good).
    NSTimer *close = [NSTimer timerWithTimeInterval:1.5
                                            repeats:NO
                                              block:^(NSTimer *timer) {
                                                  [menu cancelTracking];
                                              }];
    [NSRunLoop.mainRunLoop addTimer:close forMode:NSRunLoopCommonModes];
    [menu popUpMenuPositioningItem:nil atLocation:NSMakePoint(20, view.bounds.size.height - 20) inView:view];
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"check\",\"t_ns\":%llu,\"event\":\"menu_closed\"}\n", (unsigned long long)now_ns());
    }
}
#endif

// The input's conversation is not there: it was cut, or it has not been open yet and the host
// is asked again. The picture may well be running; what is typed does not arrive.
static void input_went_away(uint64_t t) {
    if (input_away || picture_over) {
        return;
    }
    input_away = YES;
    away_noticed_ns = t;
    unsigned long generation = ++away_generation;
#ifdef ISTESSO_CHECKS
    if (check_press_when_cut_ms >= 0 && input_was_open) {
        // A key is pressed this long after the input's conversation was cut: before the half
        // second is over, the line must come up for it at once.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(check_press_when_cut_ms * NSEC_PER_MSEC)), dispatch_get_main_queue(), ^{
            if (input_away && generation == away_generation) {
                check_press();
            }
        });
    }
#endif
    if (picture_stale) {
        return;  // The line is up for the picture: it stays until both are back.
    }
    if (!cut_pending) {
        held_back_count = 0;
        in_doubt_count = presses_since(t > 1000000000ULL ? t - 1000000000ULL : 0);
    }
    if (in_doubt_count > 0) {
        // Somebody is typing: said at once.
        mark_the_picture(away_line());
        return;
    }
    // As with the picture: one that is over before anybody could read a line is not said.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        if (input_away && generation == away_generation && !picture_stale && !picture_over) {
            // What was pressed since the input went away was handed on to nobody.
            held_back_count += presses_since(away_noticed_ns);
            mark_the_picture(away_line());
        }
    });
}

// Somebody presses a key or a button while the input is away and the line is not up yet: it
// comes up at once, and this press is not passed on either.
static void mark_at_once_for_the_input(void) {
    if (picture_over) {
        return;
    }
    away_generation++;  // The half second is not waited for.
    held_back_count += presses_since(away_noticed_ns);
    mark_the_picture(away_line());
}

// The input's conversation is open (again).
static void input_is_there(void) {
    input_was_open = YES;
    if (!input_away) {
        return;
    }
    input_away = NO;
    away_generation++;
    if (cut_pending || picture_over) {
        return;  // The picture's line stays.
    }
    take_the_line_down();
}

// The host refused for good: nothing is asked for any more. The device may look and not type:
// the title says so, and a small line that stays.
static void input_is_over(void) {
    view_only = YES;
    show_standing_note();
    if (!input_away) {
        return;
    }
    input_away = NO;
    away_generation++;
    if (!cut_pending && !picture_over) {
        take_the_line_down();
    }
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
        : input_stands == 2 ? @"view only"
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
static void first_picture_is_there(void);
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
    uint32_t said_failure = 0;
    for (;;) {
        IstessoMediaFrame frame;
        int32_t got = istesso_media_watch_next(watch, 250, &frame);
        if (got == 1) {
            if (wake_usage) {
                usage_sample(0);
            }
            on_channel_frame(watch, &frame);
            if (wake_usage) {
                usage_sample(1);
            }
            if (!shown && !vt_needs_key_frame) {
                // The first picture is on the layer.
                shown = true;
                dispatch_async(dispatch_get_main_queue(), ^{
                    first_picture_is_there();
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
            say_the_way(watch, &said_failure);
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
            said_failure = 0;
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
        } else {
            // Nothing came in a quarter of a second.
            say_the_way(watch, &said_failure);
            if (!ever && !said_unreachable && now_ns() - began > 1000000000ULL) {
                // Said once; the sentence holds no address.
                said_unreachable = true;
                fprintf(stderr, "note: no picture from the host yet; trying again\n");
                log_stream("unreachable");
            }
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
    standing_box = [CALayer layer];
    standing_box.zPosition = 25;  // Above the dimming, below the line in the middle.
    standing_box.hidden = YES;
    standing_box.cornerRadius = 6;
    standing_box.backgroundColor = CGColorCreateGenericGray(0.08, 0.82);
    standing_layer = [CATextLayer layer];
    standing_layer.alignmentMode = kCAAlignmentLeft;
    standing_layer.foregroundColor = CGColorCreateGenericGray(1, 0.92);
    [standing_box addSublayer:standing_layer];
    [vt_layer addSublayer:standing_box];
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
    fprintf(frame_log, "{\"kind\":\"wake\",\"picture_qos\":%d,\"picture_realtime\":%d,\"input_qos\":%d,\"latency_critical\":%d",
            wake_picture_qos, wake_picture_realtime, wake_input_qos, wake_latency_critical);
#ifdef ISTESSO_MEASURING
    fprintf(frame_log, ",\"keys_qos\":%d", wake_keys_qos);
#endif
    fprintf(frame_log, ",\"awake_spin\":%d,\"awake_tick\":%d,\"awake_session\":%d,\"usage\":%d,\"front\":%d", awake_form == AWAKE_SPIN,
            awake_form == AWAKE_TICK, awake_whole_session, wake_usage, wake_front);
    fprintf(frame_log, "}\n");
    if (wake_usage) {
        usage_log_process(frame_log, "start");
    }
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
    front_over = true;
    awake_finish();
#ifdef ISTESSO_MEASURING
    if (measure_keys != NULL) {
        if (frame_log != NULL) {
            // Keys a measuring program left down and that were let go for it: a run with
            // any is not a clean run.
            IstessoMeasureKeysState stands = {0};
            istesso_measure_keys_state(measure_keys, &stands);
            fprintf(frame_log, "{\"kind\":\"measure_keys\",\"t_ns\":%llu,\"at\":\"close\",\"let_go\":%llu}\n",
                    (unsigned long long)now_ns(), (unsigned long long)stands.let_go);
        }
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
        if (wake_usage) {
            usage_log_process(frame_log, "end");
            usage_log_threads(frame_log);
            usage_log_cores(frame_log, "picture", "frame_arrived", "frame_decoded");
        }
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
        if (awake_form != AWAKE_OFF) {
            awake_input();
        }
    }
}

static void send_one(uint32_t kind, int32_t a, int32_t b) {
    IstessoInputEvent event = {kind, a, b};
    send_input(&event, 1);
}

// Called on the input's thread for what happens to its conversation with the host.
static void on_input_told(void *context, int32_t event, uint64_t cuts, double after_ms, int32_t failure, const char *why) {
    static const char *const names[] = {"", "open", "cut", "back", "over", "not_yet"};
    const char *name = event >= 1 && event <= 5 ? names[event] : "other";
    uint64_t t = now_ns();
    atomic_store(&input_cuts, (long)cuts);
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"input\",\"t_ns\":%llu,\"event\":\"%s\",\"reconnections\":%llu,\"after_ms\":%.1f",
                (unsigned long long)now_ns(), name, (unsigned long long)cuts, after_ms);
        if (failure != 0) {
            // In what way the attempt failed, in a word that names no address.
            fprintf(frame_log, ",\"why\":\"%s\"", failure_word((uint32_t)failure));
        }
        fprintf(frame_log, "}\n");
    }
    if (event == ISTESSO_INPUT_CUT) {
        fprintf(stderr, "note: the input's conversation with the host was cut; opening it again\n");
    } else if (event == ISTESSO_INPUT_OVER) {
        // The sentence holds no address and no key.
        fprintf(stderr, "note: the host does not take input from here: %s\n", why != NULL ? why : "");
    } else if (event == ISTESSO_INPUT_NOT_YET) {
        // Without the reason's sentence: when the host could not be reached, it names an
        // address. The way it failed, in a word, does not.
        fprintf(stderr, "note: the host does not take input yet (%s); asking again\n", failure_word((uint32_t)failure));
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        static BOOL effect_said = NO;
        if (!effect_said && input != NULL && frame_log != NULL) {
            // What became of --wake input-qos: 0 not asked for, 1 in effect, 2 asked for
            // and not in effect.
            effect_said = YES;
            IstessoInputState stands = {0};
            istesso_input_state(input, &stands);
            fprintf(frame_log, "{\"kind\":\"wake_effect\",\"t_ns\":%llu,\"input_qos\":%u}\n", (unsigned long long)now_ns(),
                    stands.interactive);
        }
        input_stands = event == ISTESSO_INPUT_OVER ? 2 : event == ISTESSO_INPUT_NOT_YET ? 0 : 1;
        switch (event) {
            case ISTESSO_INPUT_OPEN:
            case ISTESSO_INPUT_BACK: input_is_there(); break;
            case ISTESSO_INPUT_CUT:
            case ISTESSO_INPUT_NOT_YET: input_went_away(t); break;
            case ISTESSO_INPUT_OVER: input_is_over(); break;
            default: break;
        }
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

// The window is the one typed into again: the modifiers held on this keyboard now go down on the
// other side (nothing else: a letter pressed again would be typed). Not while the picture is
// being reconnected; fallInLine does it when the picture is back.
- (void)keyboardIsBack {
    if (!self.window.isKeyWindow || !NSApp.isActive || picture_stale) {
        return;
    }
    [self fallInLine];
}

// Before a button or a scroll is sent the modifiers over there are brought in line with the
// event's flags (keys.h): a modifier whose release went elsewhere comes up, one that is held and
// whose press went elsewhere goes down. Nothing goes down while the picture is being reconnected.
// Usually nothing is to be sent, and nothing is.
- (void)modifiersInLine:(NSEvent *)event {
    KeyChange changes[KEYS_MOST];
    _keys.command_is_control = command_is_control;
    size_t count = keys_fall_in_line(&_keys, event.modifierFlags, changes);
    if (picture_stale) {
        bool dropped = false;
        count = keys_hold_back(&_keys, changes, count, &dropped);
    }
    [self send:changes count:count];
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
    if (input_away && !picture_stale) {
        mark_at_once_for_the_input();
    } else if (view_only) {
        notice_standing_note();
    }
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
    [self send:changes count:keys_key_up(&_keys, event.keyCode, event.modifierFlags, changes)];
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
    if (pressed && input_away && !picture_stale) {
        mark_at_once_for_the_input();
    } else if (pressed && view_only) {
        notice_standing_note();
    }
    if (pressed && picture_stale) {
        // Not pressed on the other side while the picture is not the host's of this moment.
        held_back_count++;
        return;
    }
    if (!pressed && !(_buttons & bit)) {
        return;  // It never went down over there, or was let go there already.
    }
    [self modifiersInLine:event];
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
        [self modifiersInLine:event];
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

// Puts the window where it belongs, in the size that was asked for; without one, in the
// picture's size once there is a picture, and in a size of its own before that.
static void place_the_window(void) {
    double main_height = NSScreen.screens.firstObject.frame.size.height;
    if (window_whole_display) {
        [the_window setFrame:window_display display:YES];
    } else {
        if (!window_sized && (picture_width <= 0 || picture_height <= 0)) {
            [the_window setContentSize:NSMakeSize(960, 540)];
        }
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
}

// At the start, before anything has come from the host: the window is there and says what it
// is doing. An application that waits for a host that is not there, or that was refused, is
// not an icon without a window and without a word.
static void show_the_window(void) {
    if (the_window == nil || the_window.visible) {
        return;
    }
    place_the_window();
    show_note(@"Connecting.");
    // Key, so that what the person types goes to this window as soon as the application is
    // the active one: it is the application's only window. This does not make the
    // application active and takes nothing from the one in front. (The prototype was a tool
    // started from a shell and only came to the front; it took the keyboard at the first
    // click.) To the front also when the application is not the active one, as the
    // prototype's window came.
    [the_window makeKeyAndOrderFront:nil];
    [the_window orderFrontRegardless];
}

// The first picture has been decoded: the window takes the picture's size, unless one was
// asked for, and the word about connecting goes.
static void first_picture_is_there(void) {
    if (!before_first_picture) {
        return;
    }
    before_first_picture = NO;
    if (the_window != nil && !window_whole_display && !window_sized && picture_width > 0 && picture_height > 0) {
        // The picture's size, one pixel of the picture to one of the display the window is
        // on now. Its top left corner stays where it is: the person may have moved it.
        NSRect frame = the_window.frame;
        NSPoint top_left = NSMakePoint(NSMinX(frame), NSMaxY(frame));
        CGFloat scale = the_window.screen != nil ? the_window.screen.backingScaleFactor : 1;
        [the_window setContentSize:NSMakeSize(picture_width / scale, picture_height / scale)];
        [the_window setFrameTopLeftPoint:top_left];
    }
    // A line that is up for the input stays, with its own words.
    show_note(picture_stale && !picture_over ? away_line() : nil);
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

// ---- Whether the window is the one in front (--wake awake-spin, awake-tick, usage, front) ----

static void front_ask(void);
static void front_look(bool first);

// What was asked for the window to be in front again, and what came of it (--wake front).
static int front_asked = 0;  // 0 nothing is asked; 1 asked with activate; 2 with activateIgnoringOtherApps
static uint64_t front_asked_ns = 0;
static unsigned front_refused = 0;  // askings in a row that the system did not grant
static unsigned long front_asking = 0;

static void front_say(const char *event, const char *how, double after_ms) {
    if (frame_log == NULL) {
        return;
    }
    fprintf(frame_log, "{\"kind\":\"front\",\"t_ns\":%llu,\"event\":\"%s\"", (unsigned long long)now_ns(), event);
    if (how != NULL) {
        fprintf(frame_log, ",\"how\":\"%s\"", how);
    }
    if (after_ms >= 0) {
        fprintf(frame_log, ",\"after_ms\":%.1f", after_ms);
    }
    fprintf(frame_log, "}\n");
}

static const char *front_how(void) {
    return front_asked == 1 ? "activate" : "activate_ignoring_others";
}

// The application is the active one and the window takes the keys. Looked at on the window's
// thread whenever one of them, or whether the window can be seen, changes; the log says each
// change, and how the system treats the process at that moment (it may follow a little later,
// so there is a second look half a second after every change).
static void front_look(bool first) {
    static int said[4] = {-1, -1, -1, -2};
    BOOL active = NSApp.active, key = the_window != nil && the_window.keyWindow;
    BOOL visible = the_window != nil && (the_window.occlusionState & NSWindowOcclusionStateVisible) != 0;
    int stands[4] = {active, key, visible, usage_role()};
    bool front = active && key, was_front = atomic_load(&awake_in_front);
    awake_set_in_front(front);
    if (!first && memcmp(said, stands, sizeof said) == 0) {
        return;
    }
    bool role_only = !first && memcmp(said, stands, 3 * sizeof *said) == 0;
    memcpy(said, stands, sizeof said);
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"awake\",\"t_ns\":%llu,\"in_front\":%d,\"active\":%d,\"key\":%d,\"visible\":%d,\"role\":%d}\n",
                (unsigned long long)now_ns(), front, active, key, visible, stands[3]);
    }
    if (!role_only) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 2), dispatch_get_main_queue(), ^{
            front_look(false);
        });
    }
    if (!wake_front || front_over) {
        return;
    }
    if (front) {
        if (front_asked != 0) {
            // What was asked for has come.
            front_say("granted", front_how(), (now_ns() - front_asked_ns) / 1e6);
        }
        front_asked = 0;
        front_refused = 0;
        front_asking++;
    } else if (was_front) {
        front_say("lost", NULL, -1);
        // Not from inside the notification that says so.
        dispatch_async(dispatch_get_main_queue(), ^{
            front_ask();
        });
    }
}

// Asks the system for the front: first as an application asks since macOS 14 (the system
// decides), and when that is not granted within a quarter of a second, with the older call.
// Both are AppKit's own, and nothing else is tried. After both were refused it asks again every
// two seconds, ten times, and then leaves it; whenever the front is lost anew it begins anew.
static void front_ask(void) {
    if (front_over || (NSApp.active && the_window.keyWindow)) {
        return;
    }
    front_asked = front_asked == 1 ? 2 : 1;
    if (@available(macOS 14.0, *)) {
    } else {
        front_asked = 2;  // Before macOS 14 there is only the older call.
    }
    // Said before it is asked: what comes of it may come during the call.
    front_asked_ns = now_ns();
    front_say("asked", front_how(), -1);
    unsigned long asking = ++front_asking;
    if (front_asked == 1) {
        if (@available(macOS 14.0, *)) {
            [NSApp activate];
        }
    } else {
        [NSApp activateIgnoringOtherApps:YES];
    }
    [the_window makeKeyAndOrderFront:nil];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 4), dispatch_get_main_queue(), ^{
        if (asking != front_asking || front_over) {
            return;  // Granted meanwhile, or the program is ending.
        }
        front_look(false);
        if (asking != front_asking) {
            return;  // In front after all: the look has said so.
        }
        front_say("refused", front_how(), (now_ns() - front_asked_ns) / 1e6);
        if (front_asked == 1) {
            front_ask();
            return;
        }
        front_asked = 0;
        if (++front_refused >= 10) {
            front_say("gave_up", NULL, -1);
            return;
        }
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            if (asking == front_asking && !front_over) {
                front_ask();
            }
        });
    });
}

static void front_watch_the_window(void) {
    NSNotificationCenter *centre = NSNotificationCenter.defaultCenter;
    for (NSNotificationName name in @[NSApplicationDidBecomeActiveNotification, NSApplicationDidResignActiveNotification]) {
        [centre addObserverForName:name object:NSApp queue:nil usingBlock:^(NSNotification *note) {
            front_look(false);
        }];
    }
    for (NSNotificationName name in @[NSWindowDidBecomeKeyNotification, NSWindowDidResignKeyNotification,
                                      NSWindowDidChangeOcclusionStateNotification]) {
        [centre addObserverForName:name object:the_window queue:nil usingBlock:^(NSNotification *note) {
            front_look(false);
        }];
    }
    front_look(true);
    if (wake_front) {
        // Once the application runs: started in the background, or behind another that came
        // to the front meanwhile, it asks.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 4), dispatch_get_main_queue(), ^{
            front_ask();
        });
    }
}

#ifdef ISTESSO_MEASURING
// Called on the thread of the measurement's way in, after it handed keys to the input's thread.
static void awake_came_through_the_way_in(void *context) {
    awake_input();
}
#endif

static void usage(void) {
    fprintf(stderr,
            "usage: Istesso --address ADDRESS:PORT [--host NAME] [--input-address ADDRESS[:PORT]] [--size WxH] [--position X,Y]\n"
            "               [--whole-display] [--title TEXT] [--frame-log FILE] [--input-timings FILE] [--wake NAME,...]\n"
            "               [--command-key super] [--config DIR] [--on-top]\n"
#ifdef ISTESSO_MEASURING
            "               [--measure-keys FOLDER]   (a measuring build: keys written to FOLDER/measure-keys.sock go to the host;\n"
            "                                          --wake also knows keys-qos, for the thread that takes them)\n"
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
#ifdef ISTESSO_CHECKS
            } else if (strcmp(name, "--check-press-when-cut") == 0) {
                check_press_when_cut_ms = atof(value);
            } else if (strcmp(name, "--check-press-at") == 0 && check_presses < 8) {
                check_press_at_s[check_presses++] = atof(value);
            } else if (strcmp(name, "--check-menu-at") == 0) {
                check_menu_at_s = atof(value);
#endif
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
                    } else if (strcmp(each, "awake-spin") == 0 || strcmp(each, "awake-tick") == 0) {
                        int form = strcmp(each, "awake-spin") == 0 ? AWAKE_SPIN : AWAKE_TICK;
                        if (awake_form != AWAKE_OFF && awake_form != form) {
                            fprintf(stderr, "error: --wake takes awake-spin or awake-tick, not both\n");
                            return 2;
                        }
                        awake_form = form;
                    } else if (strcmp(each, "awake-session") == 0) {
                        awake_whole_session = true;
                    } else if (strcmp(each, "usage") == 0) {
                        wake_usage = true;
                    } else if (strcmp(each, "front") == 0) {
                        wake_front = true;
#ifdef ISTESSO_MEASURING
                    } else if (strcmp(each, "keys-qos") == 0) {
                        wake_keys_qos = true;
#endif
                    } else if (*each != '\0') {
#ifdef ISTESSO_MEASURING
                        fprintf(stderr, "error: --wake knows picture-qos, picture-realtime, input-qos, latency-critical, awake-spin, awake-tick, awake-session, usage, front and keys-qos, not %s\n", each);
#else
                        fprintf(stderr, "error: --wake knows picture-qos, picture-realtime, input-qos, latency-critical, awake-spin, awake-tick, awake-session, usage and front, not %s\n", each);
#endif
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
        if (awake_whole_session && awake_form == AWAKE_OFF) {
            fprintf(stderr, "error: --wake awake-session goes with awake-spin or awake-tick\n");
            return 2;
        }

        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        static IstessoApplication *application;
        application = [IstessoApplication new];
        NSApp.delegate = application;
        // The application's own menu, with its own Quit. It has no shortcut: Command-Q belongs
        // to the application of the host that the window shows, as every other key with
        // Command does (ADR 0018).
        NSMenu *bar = [NSMenu new];
        NSMenuItem *own = [NSMenuItem new];
        [bar addItem:own];
        NSMenu *menu = [NSMenu new];
        [menu addItemWithTitle:[@"Quit " stringByAppendingString:NSProcessInfo.processInfo.processName]
                        action:@selector(terminate:)
                 keyEquivalent:@""];
        own.submenu = menu;
        NSApp.mainMenu = bar;
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
        if (awake_form != AWAKE_OFF || wake_usage || wake_front) {
            // Without one of these switches nothing is watched.
            front_watch_the_window();
        }
        if (awake_form != AWAKE_OFF) {
            // Before anything can hand input on. Without the switch: no thread.
            awake_log = frame_log;
            if (!awake_start()) {
                fprintf(stderr, "error: cannot start the thread that keeps a core awake\n");
                return 1;
            }
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
            // With the switch that keeps a core awake, the keys that come this way count as
            // input going on, as the window's do. Without it nothing is told.
            measure_keys = istesso_measure_keys_open(input, measure_keys_dir, wake_keys_qos ? ISTESSO_INPUT_INTERACTIVE : 0,
                                                     awake_form != AWAKE_OFF ? awake_came_through_the_way_in : NULL, NULL);
            if (measure_keys == NULL) {
                fprintf(stderr, "error: the measurement's keys: %s\n", istesso_client_last_error());
                istesso_input_close(input);
                return 1;
            }
            if (frame_log != NULL) {
                // `keys_qos`: 0 not asked for, 1 in effect, 2 asked for and not in effect.
                IstessoMeasureKeysState stands = {0};
                istesso_measure_keys_state(measure_keys, &stands);
                fprintf(frame_log, "{\"kind\":\"measure_keys\",\"t_ns\":%llu,\"at\":\"open\",\"keys_qos\":%u}\n",
                        (unsigned long long)now_ns(), stands.interactive);
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
        // The window is there from the start, with a word about what it waits for. After
        // `use_channel`: that is where the layers the word is drawn in are made.
        show_the_window();
        if (input != NULL) {
            // There is no conversation for the input yet, from the first moment on and not
            // only once an attempt has failed: what is typed now does not arrive, a press
            // brings the mark up at once and is not passed on.
            input_went_away(now_ns());
        }
#ifdef ISTESSO_CHECKS
        // A press whenever the check's script says so (SIGUSR1).
        signal(SIGUSR1, SIG_IGN);
        static dispatch_source_t press_now;
        press_now = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGUSR1, 0, dispatch_get_main_queue());
        dispatch_source_set_event_handler(press_now, ^{
            check_press();
        });
        dispatch_resume(press_now);
        for (int i = 0; i < check_presses; i++) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(check_press_at_s[i] * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                check_press();
            });
        }
        if (check_menu_at_s >= 0) {
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(check_menu_at_s * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
                check_show_menu();
            });
        }
#endif
        [NSApp run];
        finish();
    }
    return 0;
}
