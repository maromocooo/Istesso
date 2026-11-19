// Windowless tests for text-input.h. Link with Foundation.
#include "text-input.h"
#include <stdio.h>

static int failures;

// Expected pieces contain the exact UTF-8 payload as it should appear inside the JSON string.
static void expect(const char *name, NSString *input, NSArray<NSString *> *pieces) {
    __block NSUInteger index = 0;
    __block BOOL matches = YES;
    text_input_packets(input, ^(const char *packet, size_t length) {
        if (index < pieces.count) {
            NSString *json = [NSString stringWithFormat:@"{\"kind\":\"text\",\"text\":\"%@\"}", pieces[index]];
            NSData *expected = [json dataUsingEncoding:NSUTF8StringEncoding];
            matches &= length == expected.length && memcmp(packet, expected.bytes, length) == 0;
        }
        index++;
    });
    if (!matches || index != pieces.count) {
        fprintf(stderr, "FAILED: %s\n", name);
        failures++;
    }
}

static NSString *repeat(NSString *unit, NSUInteger count) {
    return [@"" stringByPaddingToLength:unit.length * count withString:unit startingAtIndex:0];
}

int main(void) {
    @autoreleasepool {
        expect("empty commit", @"", @[]);
        NSMutableString *controls = [NSMutableString string];
        for (unsigned i = 0; i < 160; i++) {
            if (i < 32 || i >= 127) {
                [controls appendFormat:@"%C", (unichar)i];
            }
        }
        expect("only controls", controls, @[]);
        expect("controls and adjacent printable scalars", [NSString stringWithFormat:@" ~%@\u00a0文", controls], @[@" ~\u00a0文"]);
        expect("UTF-8 and JSON escapes", @"日本\"\\/語", @[@"日本\\\"\\\\/語"]);
        NSString *full = repeat(@"😀", 256);
        expect("both limits exactly", full, @[full]);
        expect("one more scalar", [full stringByAppendingString:@"字"], @[full, @"字"]);
        expect("escaped ASCII at the scalar limit", repeat(@"\\", 257), @[repeat(@"\\\\", 256), @"\\\\"]);

        // Filtering the newline brings the combining mark next to its base again.
        NSString *prefix = repeat(@"字", 255);
        expect("combining sequence at a packet boundary", [prefix stringByAppendingString:@"か\n\u3099"], @[prefix, @"か\u3099"]);
        NSString *emoji = @"👩‍💻";
        expect("joined emoji at a packet boundary", [prefix stringByAppendingString:emoji], @[prefix, emoji]);
        NSString *tooLong = [@"か" stringByAppendingString:repeat(@"\u3099", 256)];
        expect("one unsendable sequence between valid ones", [NSString stringWithFormat:@"前%@後", tooLong], @[@"前後"]);
    }
    if (failures == 0) {
        puts("text input: all tests passed");
    }
    return failures != 0;
}
