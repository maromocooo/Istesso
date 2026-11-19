// Install-time operations on the Mac. No application is launched.
#import <CoreServices/CoreServices.h>
#import <AppKit/AppKit.h>
#import <CoreText/CoreText.h>
#import <ImageIO/ImageIO.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdio.h>

// The host's image is untrusted. ImageIO only sees a PNG of at most 2 MiB whose header says at
// most 2048 x 2048 pixels; the header is read here, before ImageIO parses anything, and ImageIO is
// held to PNG for the rest of this process. Decode once and copy only its pixels into each bitmap.
static const NSUInteger maxPNGBytes = 2 << 20, maxPNGSide = 2048;

static uint32_t bigEndian(const unsigned char *p) {
    return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | (uint32_t)p[3];
}

static CGImageRef readPNG(NSString *path) {
    if (@available(macOS 14.2, *)) {
        if (CGImageSourceSetAllowableTypes((__bridge CFArrayRef)@[UTTypePNG.identifier]) != noErr) return NULL;
    }
    NSFileHandle *file = [NSFileHandle fileHandleForReadingAtPath:path];
    NSData *bytes = [file readDataUpToLength:maxPNGBytes + 1 error:NULL];
    // The signature, then the first chunk: its length (13), "IHDR", the width and the height.
    const unsigned char header[] = {137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 'I', 'H', 'D', 'R'};
    if (bytes.length < sizeof(header) + 8 || bytes.length > maxPNGBytes ||
        memcmp(bytes.bytes, header, sizeof(header))) return NULL;
    const unsigned char *raw = bytes.bytes;
    uint32_t width = bigEndian(raw + sizeof(header)), height = bigEndian(raw + sizeof(header) + 4);
    if (!width || !height || width > maxPNGSide || height > maxPNGSide) return NULL;
    NSDictionary *hint = @{(__bridge NSString *)kCGImageSourceTypeIdentifierHint: UTTypePNG.identifier};
    CGImageSourceRef source = CGImageSourceCreateWithData((__bridge CFDataRef)bytes, (__bridge CFDictionaryRef)hint);
    if (!source) return NULL;
    CGImageRef image = NULL;
    CFStringRef type = CGImageSourceGetType(source);
    // Also the guard where the allowed types cannot be set: the data's own type, before decoding.
    if (type && CFEqual(type, (__bridge CFStringRef)UTTypePNG.identifier) &&
        CGImageSourceGetStatus(source) == kCGImageStatusComplete) {
        NSDictionary *info = CFBridgingRelease(CGImageSourceCopyPropertiesAtIndex(source, 0, NULL));
        NSUInteger w = [info[(__bridge NSString *)kCGImagePropertyPixelWidth] unsignedIntegerValue];
        NSUInteger h = [info[(__bridge NSString *)kCGImagePropertyPixelHeight] unsignedIntegerValue];
        if (w == width && h == height) {
            NSDictionary *options = @{
                (__bridge NSString *)kCGImageSourceCreateThumbnailFromImageAlways: @YES,
                (__bridge NSString *)kCGImageSourceThumbnailMaxPixelSize: @1024,
                (__bridge NSString *)kCGImageSourceShouldCacheImmediately: @YES,
            };
            image = CGImageSourceCreateThumbnailAtIndex(source, 0, (__bridge CFDictionaryRef)options);
            if (image && CGImageSourceGetStatusAtIndex(source, 0) != kCGImageStatusComplete) {
                CGImageRelease(image);
                image = NULL;
            }
        }
    }
    CFRelease(source);
    return image;
}

static void drawBadge(CGContextRef ctx, size_t size, NSString *label, BOOL studio) {
    BOOL small = size <= 32;
    NSString *text = small ? [label substringWithRange:[label rangeOfComposedCharacterSequenceAtIndex:0]] : label;
    CGFloat height = round(size * (small ? 0.62 : 0.31));
    NSFont *font = [NSFont boldSystemFontOfSize:height * 0.73];
    NSAttributedString *string = [[NSAttributedString alloc] initWithString:text attributes:@{
        NSFontAttributeName: font, NSForegroundColorAttributeName: NSColor.whiteColor,
    }];
    CTLineRef line = CTLineCreateWithAttributedString((__bridge CFAttributedStringRef)string);
    CGRect ink = CTLineGetBoundsWithOptions(line, kCTLineBoundsUseGlyphPathBounds);
    CGFloat padding = MAX(1, round(height * 0.15));
    CGFloat scale = MIN(1, (size - 2 * padding) / MAX(1, ink.size.width));
    CGFloat width = MIN(size, MAX(height, ceil(ink.size.width * scale + 2 * padding)));
    CGRect box = CGRectMake(size - width, 0, width, height);
    CGPathRef shape = CGPathCreateWithRoundedRect(box, height * 0.22, height * 0.22, NULL);
    if (studio) CGContextSetRGBFillColor(ctx, 0.10, 0.25, 0.52, 1);
    else CGContextSetRGBFillColor(ctx, 0.57, 0.18, 0.04, 1);
    CGContextAddPath(ctx, shape);
    CGContextFillPath(ctx);
    CGPathRelease(shape);
    CGContextTranslateCTM(ctx, CGRectGetMidX(box), CGRectGetMidY(box));
    CGContextScaleCTM(ctx, scale, scale);
    CGContextSetTextPosition(ctx, -CGRectGetMidX(ink), -CGRectGetMidY(ink));
    CTLineDraw(line, ctx);
    CFRelease(line);
}

static CGImageRef renderSize(CGImageRef base, size_t size, NSString *label, BOOL studio) {
    CGColorSpaceRef color = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CGContextRef ctx = CGBitmapContextCreate(NULL, size, size, 8, 0, color, (CGBitmapInfo)kCGImageAlphaPremultipliedLast);
    CGColorSpaceRelease(color);
    if (!ctx) return NULL;
    CGFloat w = CGImageGetWidth(base), h = CGImageGetHeight(base);
    CGFloat factor = size / MAX(w, h);
    CGRect rect = CGRectMake((size - w * factor) / 2, (size - h * factor) / 2, w * factor, h * factor);
    CGContextSetInterpolationQuality(ctx, kCGInterpolationHigh);
    CGContextDrawImage(ctx, rect, base);
    drawBadge(ctx, size, label, studio);
    CGImageRef image = CGBitmapContextCreateImage(ctx);
    CGContextRelease(ctx);
    return image;
}

static BOOL compose(NSString *path, NSString *machine, NSString *label, NSURL *url) {
    if (!label.length) return NO;
    // "-" is the generic application icon. An image that cannot be used fails here, and the caller
    // draws the generic icon in a new process: ImageIO is held to PNG in this one.
    CGImageRef base = NULL;
    if ([path isEqualToString:@"-"]) {
        NSImage *generic = [NSWorkspace.sharedWorkspace iconForContentType:UTTypeApplicationBundle];
        NSRect bounds = NSMakeRect(0, 0, 1024, 1024);
        base = CGImageRetain([generic CGImageForProposedRect:&bounds context:nil hints:nil]);
    } else {
        base = readPNG(path);
    }
    if (!base) return NO;
    CGImageDestinationRef dest = CGImageDestinationCreateWithURL((__bridge CFURLRef)url,
        (__bridge CFStringRef)UTTypeICNS.identifier, 10, NULL);
    BOOL ok = dest != NULL;
    const int points[] = {16, 32, 128, 256, 512};
    for (size_t i = 0; i < sizeof(points) / sizeof(points[0]) && ok; i++) {
        for (int scale = 1; scale <= 2 && ok; scale++) {
            CGImageRef image = renderSize(base, points[i] * scale, label, [machine isEqualToString:@"mac"]);
            if (!image) {
                ok = NO;
                break;
            }
            NSDictionary *properties = @{
                (__bridge NSString *)kCGImagePropertyDPIWidth: @(72 * scale),
                (__bridge NSString *)kCGImagePropertyDPIHeight: @(72 * scale),
            };
            CGImageDestinationAddImage(dest, image, (__bridge CFDictionaryRef)properties);
            CGImageRelease(image);
        }
    }
    if (dest) {
        ok = ok && CGImageDestinationFinalize(dest);
        CFRelease(dest);
    }
    CGImageRelease(base);
    return ok;
}

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc == 6 && strcmp(argv[1], "compose") == 0) {
            return compose(@(argv[2]), @(argv[3]), @(argv[4]), [NSURL fileURLWithPath:@(argv[5])]) ? 0 : 1;
        }
        if (argc == 4 && strcmp(argv[1], "exchange") == 0) {
            if (renamex_np(argv[2], argv[3], RENAME_SWAP) == 0) return 0;
            perror("Could not exchange the trial directories");
            return 1;
        }
        if (argc == 3 && strcmp(argv[1], "register") == 0) {
            NSURL *bundle = [NSURL fileURLWithPath:@(argv[2]) isDirectory:YES];
            return LSRegisterURL((__bridge CFURLRef)bundle, true) == noErr ? 0 : 1;
        }
        return 2;
    }
}
