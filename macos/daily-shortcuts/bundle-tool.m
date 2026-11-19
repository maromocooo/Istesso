// Install-time operations on the Mac. Neither operation starts an application.
#import <CoreServices/CoreServices.h>
#import <Foundation/Foundation.h>
#include <stdio.h>

int main(int argc, char **argv) {
    @autoreleasepool {
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
