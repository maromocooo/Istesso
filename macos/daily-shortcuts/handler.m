#import <AppKit/AppKit.h>

// The signed resource contains the complete set of URLs and argv arrays made at build time.
// No part of an incoming URL is used as a command, a path or an argument.
static NSString *openShortcut(NSString *url, NSDictionary *config) {
    NSArray<NSString *> *args = config[@"routes"][url];
    if (args.count < 4) return @"This shortcut is not on the trial's app list.";
    NSString *name = [args[1] lowercaseString];
    if ([name hasSuffix:@".app"]) name = [name stringByDeletingPathExtension];
    if ([@[@"safari", @"notes", @"finder", @"preview", @"dia", @"desktop"] containsObject:name])
        return @"Open owner-only apps and the whole display from the owner's terminal.";

    NSURL *checkout = [NSURL fileURLWithPath:config[@"checkout"] isDirectory:YES];
    NSTask *task = [[NSTask alloc] init];
    NSMutableDictionary *environment = [NSProcessInfo.processInfo.environment mutableCopy];
    [environment removeObjectForKey:@"DAILY_OWNER"];
    [environment removeObjectForKey:@"DAILY_OPEN_HANDLER"];
    task.environment = environment;
    task.currentDirectoryURL = checkout;
    task.executableURL = [checkout URLByAppendingPathComponent:@"scripts/daily.sh"];
    task.arguments = args;
    task.standardInput = NSFileHandle.fileHandleWithNullDevice;
    task.standardOutput = NSFileHandle.fileHandleWithNullDevice;
    task.standardError = NSFileHandle.fileHandleWithNullDevice;
    if (![task launchAndReturnError:NULL]) return @"Could not start daily.sh in the trial checkout.";
    [task waitUntilExit];
    return task.terminationStatus == 0 ? nil :
        [NSString stringWithFormat:@"daily.sh could not open the app (exit %d).", task.terminationStatus];
}

// All URL failures go through the same serial queue.
static void logFailure(NSString *error) {
    static NSTimeInterval nextLog;
    NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
    if (now >= nextLog) {
        NSLog(@"%@", error);
        nextLog = now + 10.0;
    }
}

#ifdef SHORTCUT_TEST
// Headless entry point for the argv recorder. The shipped app has no command-line interface.
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc < 3) return 2;
        NSDictionary *config = [NSDictionary dictionaryWithContentsOfFile:@(argv[1])];
        int status = 0;
        for (int i = 2; i < argc; i++) {
            NSString *error = openShortcut(@(argv[i]), config);
            if (error) {
                logFailure(error);
                status = 1;
            }
        }
        return status;
    }
}
#else
@interface ShortcutDelegate : NSObject <NSApplicationDelegate>
@end

@implementation ShortcutDelegate {
    NSOperationQueue *_opens;
}
- (instancetype)init {
    if ((self = [super init])) {
        _opens = [[NSOperationQueue alloc] init];
        _opens.maxConcurrentOperationCount = 1;
    }
    return self;
}
- (void)application:(NSApplication *)app openURLs:(NSArray<NSURL *> *)urls {
    // Stay available while daily.sh's relay runs. Pick up rebuilt or removed routes on each event.
    NSURL *resource = [NSBundle.mainBundle URLForResource:@"Routes" withExtension:@"plist"];
    NSDictionary *config = resource ? [NSDictionary dictionaryWithContentsOfURL:resource] : nil;
    for (NSURL *url in urls) {
        [_opens addOperationWithBlock:^{
            NSString *error = openShortcut(url.absoluteString, config);
            if (error) logFailure(error);
        }];
    }
}
@end

int main(void) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        __attribute__((objc_precise_lifetime)) ShortcutDelegate *delegate = [[ShortcutDelegate alloc] init];
        app.delegate = delegate;
        [app run];
    }
    return 0;
}
#endif
