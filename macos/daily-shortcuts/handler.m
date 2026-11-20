#import <AppKit/AppKit.h>
#include <limits.h>
#include <pwd.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

// The environment daily.sh gets, made here as the owner shim makes its own: nothing of the one this
// handler was started with (a setting given to the whole login session, say) reaches the trial.
static NSMutableDictionary<NSString *, NSString *> *cleanEnvironment(NSString **home) {
    static NSDictionary<NSString *, NSString *> *made;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        char tmpdir[PATH_MAX];
        // The Darwin lookup can fall back to TMPDIR. This process's own environment is left as
        // it is (other threads read it): only the system's directory, the user's own, is taken.
        size_t len = confstr(_CS_DARWIN_USER_TEMP_DIR, tmpdir, sizeof tmpdir);
        struct passwd *account = getpwuid(getuid());
        struct stat place;
        if (!account || !account->pw_dir || !account->pw_dir[0] || !account->pw_name ||
            !account->pw_name[0] || len <= 1 || len > sizeof tmpdir ||
            (strncmp(tmpdir, "/var/folders/", 13) != 0 && strncmp(tmpdir, "/private/var/folders/", 21) != 0) ||
            lstat(tmpdir, &place) != 0 || !S_ISDIR(place.st_mode) || place.st_uid != getuid() ||
            (place.st_mode & (S_IWGRP | S_IWOTH)))
            return;
        NSString *user = @(account->pw_name);
        made = @{@"HOME" : @(account->pw_dir), @"USER" : user, @"LOGNAME" : user,
                 @"PATH" : @"/usr/bin:/bin:/usr/sbin:/sbin", @"TMPDIR" : @(tmpdir), @"LANG" : @"en_US.UTF-8"};
    });
    if (!made) return nil;
    *home = made[@"HOME"];
    return [made mutableCopy];
}

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
    // Never DAILY_OWNER or DAILY_OPEN_HANDLER: they are not in it.
    NSString *home = nil;
    NSMutableDictionary *environment = cleanEnvironment(&home);
    if (!environment) return @"Could not build the shortcut environment.";
    NSString *title = config[@"titles"][args[3]][args[1]];
    if (title && args.count == 4) {
        NSString *relative = [NSString stringWithFormat:@"Applications/Istesso Dev/%@.app/Contents/Resources/Shortcut.icns", title];
        environment[@"ISTESSO_APP_ICON"] = [home stringByAppendingPathComponent:relative];
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
