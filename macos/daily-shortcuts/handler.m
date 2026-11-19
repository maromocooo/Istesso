#import <AppKit/AppKit.h>

// The signed resource contains the complete set of URLs and argv arrays made at build time.
// No part of an incoming URL is used as a command, a path or an argument.
static NSString *openShortcut(NSString *url, NSDictionary *config) {
    NSArray<NSString *> *args = config[@"routes"][url];
    if (!args) return @"This shortcut is not on the trial's app list.";

    NSURL *checkout = [NSURL fileURLWithPath:config[@"checkout"] isDirectory:YES];
    NSTask *task = [[NSTask alloc] init];
    NSMutableDictionary *environment = [NSProcessInfo.processInfo.environment mutableCopy];
    if ([args[3] isEqualToString:@"mac"]) {
        environment[@"DAILY_OWNER"] = @"1";
        environment[@"DAILY_OPEN_HANDLER"] = @"1";
    } else {
        [environment removeObjectForKey:@"DAILY_OWNER"];
        [environment removeObjectForKey:@"DAILY_OPEN_HANDLER"];
    }
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
        [NSString stringWithFormat:@"daily.sh could not open %@ (exit %d).", args[1], task.terminationStatus];
}

#ifdef SHORTCUT_TEST
// Headless entry point for the argv recorder. The shipped app has no command-line interface.
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 3) return 2;
        NSDictionary *config = [NSDictionary dictionaryWithContentsOfFile:@(argv[1])];
        NSString *error = openShortcut(@(argv[2]), config);
        if (error) fprintf(stderr, "%s\n", error.UTF8String);
        return error != nil;
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
            if (error) {
                [[NSOperationQueue mainQueue] addOperationWithBlock:^{
                    NSAlert *alert = [[NSAlert alloc] init];
                    alert.messageText = @"Istesso Dev Open";
                    alert.informativeText = error;
                    [app activateIgnoringOtherApps:YES];
                    [alert runModal];
                }];
            }
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
