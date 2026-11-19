// The application's other windows, each in a window of its own here (ADR 0027, a proposal).
// PROTOTYPE, trial only, and only with ISTESSO_WINDOWS=1 in the everyday form (ISTESSO_CHANNEL and
// ISTESSO_DAILY_RECONNECT); without it nothing here runs and the viewer is as before.
//
// Included once, by gst-viewer.m, after InputView and TextInputView.
//
//   - It says to the host, through the input socket, that it can show windows one by one
//     ({"kind":"windows","version":1}), and only while the relay says that the host's agent passes
//     window numbers on ("windows":1 in its state line): an agent that does not would let a
//     pointer meant for another window land on the first one. Said again when the relay's
//     connection or the picture's is made again.
//   - The host then lists its application's other windows (packet kind 6 on the first window's
//     connection, window-list.h). Each listed window gets a window here, with a connection of its
//     own to the port listed for it, a decoder of its own and a view of the same kind as the
//     first window's. They belong to this one process, so the Dock and Command-Tab show the
//     application once, and Command-` goes round its windows.
//   - Its pointer positions carry its number; keys carry none (they go where the host's keyboard
//     is). When it becomes the window typed into, {"kind":"focus","window":N} goes; its red button
//     asks the host to close that window ({"kind":"close","window":N}) and the window here goes
//     only when the host's list no longer has it.
//   - The first window's connection, decoder and window are not touched by any of this.
//
// The frame log (when there is one) gets {"kind":"window","id":N,"event":E,"t_ns":T} with E one of
// "listed", "connected", "first_picture", "cut", "over", "closed". Never a title.

#include "window-list.h"

static BOOL windows_on = NO;
static BOOL windows_hidden = NO;  // ISTESSO_NO_WINDOW: made, never shown (checks)
// What the relay said last: whether the host's agent passes window numbers on, and how often its
// connection was made again (a new connection is told again).
static BOOL relay_takes_windows = NO;
static long relay_windows_input = -1;

static void log_window(uint32_t number, const char *event) {
    if (frame_log != NULL) {
        fprintf(frame_log, "{\"kind\":\"window\",\"id\":%u,\"event\":\"%s\",\"t_ns\":%llu}\n", number, event,
                (unsigned long long)now_ns());
    }
}

static uint32_t key_window_number(void);

// Tells the host that this viewer shows windows one by one: only while the way there passes window
// numbers on. Idempotent on the host's side. Said again on a new connection, and with it which
// window here is typed into: the host may have lost it (a relay connected again passes on the
// pointer and the held keys only).
static void declare_windows(void) {
    if (windows_on && relay_takes_windows) {
        send_input("{\"kind\":\"windows\",\"version\":1}");
        char line[64];
        if (window_focus_line(NSApp.isActive, key_window_number(), line, sizeof line)) {
            send_input("%s", line);
        }
    }
}

// The relay's state line was read (on the main thread): `takes` its "windows", `input` how often
// its connection was made again.
static void relay_windows_state(BOOL takes, long input) {
    if (!windows_on) {
        return;
    }
    BOOL newly = takes && (!relay_takes_windows || input != relay_windows_input);
    relay_takes_windows = takes;
    relay_windows_input = input;
    if (newly) {
        declare_windows();
    }
}

// ---- One other window: its connection and decoder, on a thread of its own ----

typedef struct {
    VTDecompressionSessionRef session;
    CMVideoFormatDescriptionRef format;
    uint8_t *sets[3];
    size_t set_sizes[3];
    gboolean needs_key;
    guint64 asked_ns;
    gboolean srgb;
    int width, height;
} ExtraDecoder;

@class ExtraWindow;
static void extra_window_sized(ExtraWindow *extra, int width, int height);

// VideoToolbox has decoded a picture of another window: to that window's layer, as the first
// window's (on_picture), without copying it.
static void extra_on_picture(void *context, void *frame, OSStatus status, VTDecodeInfoFlags flags, CVImageBufferRef picture,
                             CMTime time, CMTime duration) {
    (void)frame, (void)flags, (void)time, (void)duration;
    if (status != noErr || picture == NULL) {
        return;
    }
    CALayer *layer = (__bridge CALayer *)context;
    IOSurfaceRef surface = CVPixelBufferGetIOSurface(picture);
    if (surface != NULL) {
        static CFDataRef profile = NULL;
        if (profile == NULL) {
            CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            profile = CGColorSpaceCopyICCData(space);
            CGColorSpaceRelease(space);
        }
        CVBufferSetAttachment(picture, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_sRGB,
                              kCVAttachmentMode_ShouldPropagate);
        if (profile != NULL) {
            IOSurfaceSetValue(surface, kIOSurfaceColorSpace, profile);
        }
    }
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    layer.contents = (__bridge id)surface;
    [CATransaction commit];
    [CATransaction flush];
}

static void extra_decoder_end(ExtraDecoder *d) {
    if (d->session != NULL) {
        VTDecompressionSessionInvalidate(d->session);
        CFRelease(d->session);
        d->session = NULL;
    }
    if (d->format != NULL) {
        CFRelease(d->format);
        d->format = NULL;
    }
    for (int i = 0; i < 3; i++) {
        free(d->sets[i]);
        d->sets[i] = NULL;
        d->set_sizes[i] = 0;
    }
}

static gboolean extra_decoder_configure(ExtraDecoder *d, const NalUnit *sets[3], gboolean all_colour, CALayer *layer) {
    gboolean same = d->session != NULL;
    for (int i = 0; i < 3 && same; i++) {
        same = d->set_sizes[i] == sets[i]->size && memcmp(d->sets[i], sets[i]->bytes, sets[i]->size) == 0;
    }
    if (same) {
        return TRUE;
    }
    extra_decoder_end(d);
    const uint8_t *pointers[3];
    for (int i = 0; i < 3; i++) {
        d->sets[i] = malloc(sets[i]->size);
        memcpy(d->sets[i], sets[i]->bytes, sets[i]->size);
        d->set_sizes[i] = sets[i]->size;
        pointers[i] = d->sets[i];
    }
    if (CMVideoFormatDescriptionCreateFromHEVCParameterSets(kCFAllocatorDefault, 3, pointers, d->set_sizes, 4, NULL, &d->format) != noErr) {
        return FALSE;
    }
    CMVideoDimensions size = CMVideoFormatDescriptionGetDimensions(d->format);
    d->width = size.width;
    d->height = size.height;
    NSDictionary *decoder = @{(__bridge NSString *)kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder : @YES};
    NSDictionary *pictures = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey :
            @(all_colour ? kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange : kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{}
    };
    VTDecompressionOutputCallbackRecord callback = {extra_on_picture, (__bridge void *)layer};
    if (VTDecompressionSessionCreate(kCFAllocatorDefault, d->format, (__bridge CFDictionaryRef)decoder, (__bridge CFDictionaryRef)pictures,
                                     &callback, &d->session) != noErr) {
        d->session = NULL;
        return FALSE;
    }
    VTSessionSetProperty(d->session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue);
    return TRUE;
}

static void extra_ask_key(ExtraDecoder *d, IstessoMediaWatch *watch) {
    guint64 t = now_ns();
    if (d->asked_ns != 0 && t - d->asked_ns < 500 * 1000 * 1000ULL) {
        return;
    }
    d->asked_ns = t;
    istesso_media_watch_ask_key_frame(watch);
}

// One whole frame of another window: decoded here and now. TRUE when a picture was decoded.
static gboolean extra_decode(ExtraDecoder *d, IstessoMediaWatch *watch, const IstessoMediaFrame *frame, gboolean all_colour,
                             CALayer *layer) {
    NalUnit units[128];
    size_t count = split_access_unit(frame->data, frame->len, units, sizeof units / sizeof units[0]);
    const NalUnit *sets[3] = {NULL, NULL, NULL};
    size_t total = 0;
    for (size_t i = 0; i < count; i++) {
        if (units[i].type >= 32 && units[i].type <= 34) {
            sets[units[i].type - 32] = &units[i];
        } else if (units[i].type != 35) {
            total += 4 + units[i].size;
        }
    }
    if (frame->key_frame && sets[0] != NULL && sets[1] != NULL && sets[2] != NULL && !extra_decoder_configure(d, sets, all_colour, layer)) {
        d->needs_key = TRUE;
        extra_ask_key(d, watch);
        return FALSE;
    }
    if (frame->discontinuity && !frame->key_frame) {
        d->needs_key = TRUE;
    }
    if (d->session == NULL || total == 0 || (d->needs_key && !frame->key_frame)) {
        d->needs_key = TRUE;
        extra_ask_key(d, watch);
        return FALSE;
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
        status = CMSampleBufferCreateReady(kCFAllocatorDefault, block, d->format, 1, 0, NULL, 1, &total, &sample);
    }
    if (status == noErr) {
        status = VTDecompressionSessionDecodeFrame(d->session, sample, 0, NULL, NULL);
    }
    d->needs_key = status != noErr;
    if (d->needs_key) {
        extra_ask_key(d, watch);
    } else if (frame->key_frame) {
        d->asked_ns = 0;
    }
    if (sample != NULL) {
        CFRelease(sample);
    }
    if (block != NULL) {
        CFRelease(block);
    }
    return status == noErr;
}

// ---- The window ----

@interface ExtraWindow : NSObject <NSWindowDelegate>
@property(nonatomic) uint32_t number;
@property(nonatomic) uint16_t port;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, strong) InputView *view;
@property(nonatomic, strong) CALayer *layer;
@property(nonatomic) BOOL closeAsked;
@property(nonatomic, strong) id focusWatch;
// The host's keyboard was in it, as the host's list said last.
@property(nonatomic) BOOL hostKey;
@end

static NSMutableDictionary<NSNumber *, ExtraWindow *> *extra_windows = nil;
static void *extra_thread(void *raw);

// The number of this application's window typed into: 1 the first, 2 and up another, 0 none.
static uint32_t key_window_number(void) {
    NSWindow *key = NSApp.keyWindow;
    if (key == nil) {
        return 0;
    }
    if (key == picture_window) {
        return 1;
    }
    for (ExtraWindow *extra in extra_windows.allValues) {
        if (extra.window == key) {
            return extra.number;
        }
    }
    return 0;
}

@implementation ExtraWindow {
    atomic_bool _stop;
    char _address[300];
}

- (instancetype)initWithListed:(ListedWindow *)listed index:(unsigned)index {
    if ((self = [super init]) == nil) {
        return nil;
    }
    _number = listed.number;
    _port = listed.port;
    atomic_init(&_stop, false);
    if (!window_address(channel_address, listed.port, _address, sizeof _address)) {
        return nil;
    }
    // As large as its picture, at the first window's points to a pixel, made smaller as a whole to
    // fit the screen (window-size.h).
    NSScreen *screen = picture_window.screen ?: NSScreen.mainScreen;
    double each = points_per_pixel > 0 ? points_per_pixel : 1.0 / (screen.backingScaleFactor > 0 ? screen.backingScaleFactor : 2);
    NSRect room = screen.visibleFrame;
    double w, h;
    window_size_for_picture((unsigned)listed.width, (unsigned)listed.height, each, room.size.width, room.size.height - 28, &w, &h);
    NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, fmax(w, 64), fmax(h, 64))
                                                   styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable
                                                     backing:NSBackingStoreBuffered
                                                       defer:NO];
    window.releasedWhenClosed = NO;
    window.title = listed.title.length > 0 ? listed.title : (picture_window.title ?: @"");
    window.delegate = self;
    Class viewClass = [cursor_view isKindOfClass:InputView.class] ? cursor_view.class : InputView.class;
    InputView *view = [[viewClass alloc] initWithFrame:NSMakeRect(0, 0, fmax(w, 64), fmax(h, 64))];
    view.hostWindow = listed.number;
    view.wantsLayer = YES;
    window.contentView = view;
    CALayer *layer = view.layer;
    layer.contentsGravity = kCAGravityResize;
    layer.backgroundColor = CGColorGetConstantColor(kCGColorBlack);
    _window = window;
    _view = view;
    _layer = layer;
    [window makeFirstResponder:view];
    NSRect first = picture_window != nil ? picture_window.frame : NSMakeRect(NSMinX(room), NSMaxY(room) - 400, 400, 400);
    [window setFrameTopLeftPoint:window_top_left(first, window.frame.size, index, room)];
    uint32_t number = listed.number;
    _focusWatch = [NSNotificationCenter.defaultCenter addObserverForName:NSWindowDidBecomeKeyNotification
                                                    object:window
                                                     queue:NSOperationQueue.mainQueue
                                                usingBlock:^(NSNotification *note) {
                                                    (void)note;
                                                    send_input("{\"kind\":\"focus\",\"window\":%u}", number);
                                                }];
    return self;
}

// The red button: the host is asked to close the application's window; this one goes when the
// host's list no longer has it (ADR 0027, question 窓3, (c)).
- (BOOL)windowShouldClose:(NSWindow *)sender {
    (void)sender;
    if (!self.closeAsked) {
        self.closeAsked = YES;
        send_input("{\"kind\":\"close\",\"window\":%u}", self.number);
        // Asked again on the next press after two seconds (a request lost on the way).
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            self.closeAsked = NO;
        });
    }
    return NO;
}

- (void)start {
    if (!windows_hidden) {
        [self.window orderFront:nil];
    }
    pthread_t thread;
    if (pthread_create(&thread, NULL, extra_thread, (__bridge_retained void *)self) == 0) {
        pthread_detach(thread);
    }
}

// Ends its connection (from its own thread) and takes the window away.
- (void)end {
    atomic_store(&_stop, true);
    if (self.focusWatch != nil) {
        [NSNotificationCenter.defaultCenter removeObserver:self.focusWatch];
        self.focusWatch = nil;
    }
    self.window.delegate = nil;
    [self.window orderOut:nil];
    [self.window close];
    log_window(self.number, "closed");
}

- (BOOL)stopped {
    return atomic_load(&_stop);
}

- (const char *)address {
    return _address;
}

static void *extra_thread(void *raw) {
    ExtraWindow *extra = (__bridge_transfer ExtraWindow *)raw;
    IstessoMediaWatch *watch = istesso_media_watch_open(NULL, [extra address], getenv("ISTESSO_CHANNEL_PEER"));
    if (watch == NULL) {
        fprintf(stderr, "note: the picture of another window cannot be asked for: %s\n", istesso_media_last_error());
        return NULL;
    }
    // Not told to follow the host's restarts: another window's stream that the host ends is over
    // (its window is gone, or it has another size and so another stream, on another port).
    ExtraDecoder decoder = {0};
    decoder.needs_key = TRUE;
    IstessoMediaStreamInfo info = {0};
    gboolean drawn = FALSE;
    int told_width = 0, told_height = 0;
    while (![extra stopped]) {
        IstessoMediaFrame frame;
        int32_t got = istesso_media_watch_next(watch, 250, &frame);
        if (got == 1) {
            if (extra_decode(&decoder, watch, &frame, info.chroma == 444, extra.layer)) {
                if (!drawn) {
                    drawn = TRUE;
                    log_window(extra.number, "first_picture");
                }
                int width = decoder.width, height = decoder.height;
                if (width != told_width || height != told_height) {
                    told_width = width;
                    told_height = height;
                    dispatch_async(dispatch_get_main_queue(), ^{
                        extra_window_sized(extra, width, height);
                    });
                }
            }
        } else if (got == 4) {
            istesso_media_watch_info(watch, &info);
            decoder.needs_key = TRUE;
            decoder.asked_ns = 0;
            log_window(extra.number, "connected");
        } else if (got == 3) {
            log_window(extra.number, "cut");
        } else if (got < 0) {
            log_window(extra.number, "over");
            // Its window goes here too, unless the host's list has already taken it away.
            dispatch_async(dispatch_get_main_queue(), ^{
                NSNumber *key = @(extra.number);
                if (extra_windows[key] == extra) {
                    [extra end];
                    [extra_windows removeObjectForKey:key];
                }
            });
            break;
        }
    }
    istesso_media_watch_close(watch);
    extra_decoder_end(&decoder);
    return NULL;
}
@end

static void extra_window_sized(ExtraWindow *extra, int width, int height) {
    extra.view.ownPictureWidth = width;
    extra.view.ownPictureHeight = height;
}

// What the host says about the text field with the keyboard (kind 5) reaches the other windows'
// views too: above all a password field, which turns the Mac's secure input on there as well. Its
// place is in the first window's picture, so the other windows take the state without the place.
// The Mac's secure input is one switch that each view sets for itself: the window typed into is
// told last, so that its answer is the one that stays. NO (nothing done) without other windows.
static BOOL windows_text_field(IstessoMediaTextInput field) {
    if (!windows_on || extra_windows.count == 0) {
        return NO;
    }
    IstessoMediaTextInput without_place = field;
    without_place.x = 0;
    without_place.y = 0;
    without_place.width = 0;
    without_place.height = 0;
    NSMutableArray<NSView *> *views = [NSMutableArray array];
    if (cursor_view != nil) {
        [views addObject:cursor_view];
    }
    for (ExtraWindow *extra in extra_windows.allValues) {
        [views addObject:extra.view];
    }
    [views sortUsingComparator:^NSComparisonResult(NSView *a, NSView *b) {
        return (NSComparisonResult)((int)a.window.isKeyWindow - (int)b.window.isKeyWindow);
    }];
    for (NSView *view in views) {
        if ([view respondsToSelector:@selector(setTextInputField:)]) {
            [(id)view setTextInputField:view == cursor_view ? field : without_place];
        }
    }
    return YES;
}

// The first window closes (its red button: the form of today, the program ends with it): the other
// windows here go with it; the application's windows stay on the host.
static void windows_follow_first(NSWindow *first) {
    if (!windows_on) {
        return;
    }
    [NSNotificationCenter.defaultCenter addObserverForName:NSWindowWillCloseNotification
                                                    object:first
                                                     queue:NSOperationQueue.mainQueue
                                                usingBlock:^(NSNotification *note) {
                                                    (void)note;
                                                    for (ExtraWindow *extra in extra_windows.allValues) {
                                                        [extra end];
                                                    }
                                                    [extra_windows removeAllObjects];
                                                }];
}

// The host's list of its application's other windows (on the main thread).
static void windows_listed(NSData *json) {
    if (!windows_on) {
        return;
    }
    NSArray<ListedWindow *> *listed = windows_from_list(json);
    if (listed == nil) {
        fprintf(stderr, "note: the host's list of windows could not be read\n");
        return;
    }
    if (extra_windows == nil) {
        extra_windows = [NSMutableDictionary dictionary];
    }
    NSMutableDictionary<NSNumber *, NSNumber *> *shown = [NSMutableDictionary dictionary];
    for (NSNumber *number in extra_windows) {
        shown[number] = @(extra_windows[number].port);
    }
    WindowChanges changes = window_changes(shown, listed);
    for (NSNumber *number in changes.close) {
        [extra_windows[number] end];
        [extra_windows removeObjectForKey:number];
    }
    for (ListedWindow *w in changes.open) {
        log_window(w.number, "listed");
        ExtraWindow *extra = [[ExtraWindow alloc] initWithListed:w index:(unsigned)extra_windows.count];
        if (extra == nil) {
            fprintf(stderr, "note: another window's picture has no address to be asked for at\n");
            continue;
        }
        extra_windows[@(w.number)] = extra;
        [extra start];
    }
    for (ListedWindow *w in listed) {
        ExtraWindow *extra = extra_windows[@(w.number)];
        if (extra == nil) {
            continue;
        }
        if (w.title.length > 0 && ![extra.window.title isEqualToString:w.title]) {
            extra.window.title = w.title;
        }
        // The host's keyboard went into this window (a new window of Command-N, a dialog): it
        // becomes the window typed into here too, but only while this application is the one in
        // front here (ADR 0027, question 窓7, (a)); otherwise nothing moves.
        if (w.key && !extra.hostKey && NSApp.isActive && !windows_hidden) {
            [extra.window makeKeyAndOrderFront:nil];
        }
        extra.hostKey = w.key;
    }
    fprintf(stderr, "windows: %lu besides the first\n", (unsigned long)extra_windows.count);
}
