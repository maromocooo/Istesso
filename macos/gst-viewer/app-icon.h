// Open each component relative to its directory. A link cannot redirect the image read.
#import <AppKit/AppKit.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

static NSImage *shortcutIcon(NSString *home, NSString *path) {
    NSString *prefix = [home stringByAppendingString:@"/Applications/Istesso Dev/"];
    if (![path hasPrefix:prefix] || ![path.pathExtension isEqualToString:@"icns"]) return nil;
    NSArray<NSString *> *parts = [[path substringFromIndex:home.length + 1] componentsSeparatedByString:@"/"];
    int fd = open(home.fileSystemRepresentation, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    for (NSUInteger i = 0; fd >= 0 && i < parts.count; i++) {
        NSString *part = parts[i];
        int next = -1;
        if (part.length && ![part isEqualToString:@"."] && ![part isEqualToString:@".."]) {
            int flags = O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK;
            if (i + 1 < parts.count) flags |= O_DIRECTORY;
            next = openat(fd, part.fileSystemRepresentation, flags);
        }
        close(fd);
        fd = next;
    }
    if (fd < 0) return nil;
    NSFileHandle *file = [[NSFileHandle alloc] initWithFileDescriptor:fd closeOnDealloc:YES];
    struct stat st;
    if (fstat(fd, &st) || !S_ISREG(st.st_mode)) return nil;
    NSData *data = [file readDataToEndOfFileAndReturnError:NULL];
    return data.length ? [[NSImage alloc] initWithData:data] : nil;
}
