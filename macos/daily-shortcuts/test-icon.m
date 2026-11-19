// Inspect the installed image through ImageIO and the viewer's loader. No NSApplication is needed.
#import "../gst-viewer/app-icon.h"
#import <ImageIO/ImageIO.h>

static BOOL unpack(NSURL *icon, NSURL *dir) {
    CGImageSourceRef source = CGImageSourceCreateWithURL((__bridge CFURLRef)icon, NULL);
    if (!source) return NO;
    BOOL ok = YES;
    for (size_t i = 0; i < CGImageSourceGetCount(source) && ok; i++) {
        CGImageRef image = CGImageSourceCreateImageAtIndex(source, i, NULL);
        if (!image) {
            ok = NO;
            break;
        }
        NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithCGImage:image];
        NSString *name = [NSString stringWithFormat:@"%zu.png", i];
        NSData *png = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        ok = [png writeToURL:[dir URLByAppendingPathComponent:name] atomically:NO];
        CGImageRelease(image);
    }
    CFRelease(source);
    return ok;
}

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc != 4) return 2;
        if (!strcmp(argv[1], "load")) return shortcutIcon(@(argv[2]), @(argv[3])) ? 0 : 1;
        return unpack([NSURL fileURLWithPath:@(argv[2])], [NSURL fileURLWithPath:@(argv[3])]) ? 0 : 1;
    }
}
