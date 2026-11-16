// Shows the video of a GStreamer pipeline in a window placed where asked.
//
// Step 3 of the M1 plan receives the stream with ready-made GStreamer elements. Their own
// window cannot be placed, and it always opens on the main display. For a measurement the
// relayed picture has to sit next to a local window on a chosen display, so this program opens
// the window itself and hands its view to the pipeline's video sink.
//
// Usage: gst-viewer X Y WIDTH HEIGHT TITLE PIPELINE
//   X Y            top left corner of the window, in points from the top left of the main display
//   WIDTH HEIGHT   size of the picture area, in points
//   PIPELINE       a gst-launch style description ending in a video sink, for example
//                  "udpsrc port=5004 caps=... ! rtph265depay ! h265parse ! avdec_h265 ! glimagesink sync=false"
//
// Only for measurements: the own client of step 4 replaces it. GStreamer is linked dynamically.

#import <Cocoa/Cocoa.h>
#include <gst/gst.h>
#include <gst/video/videooverlay.h>

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
        if (argc != 7) {
            fprintf(stderr, "usage: gst-viewer X Y WIDTH HEIGHT TITLE PIPELINE\n");
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
