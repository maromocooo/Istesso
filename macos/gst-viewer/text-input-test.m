// Windowless tests for text-input.h. Link with Foundation.
#include "text-input.h"
#include <stdio.h>

static int failures;

// Expected pieces contain the exact UTF-8 payload as it should appear inside the JSON string;
// "KEY:n" stands for key n pressed and let go (two datagrams).
static void expect(const char *name, NSString *input, NSArray<NSString *> *pieces) {
    NSMutableArray<NSString *> *wanted = [NSMutableArray array];
    for (NSString *piece in pieces) {
        if ([piece hasPrefix:@"KEY:"]) {
            NSString *code = [piece substringFromIndex:4];
            [wanted addObject:[NSString stringWithFormat:@"{\"kind\":\"key\",\"code\":%@,\"pressed\":true}", code]];
            [wanted addObject:[NSString stringWithFormat:@"{\"kind\":\"key\",\"code\":%@,\"pressed\":false}", code]];
        } else {
            [wanted addObject:[NSString stringWithFormat:@"{\"kind\":\"text\",\"text\":\"%@\"}", piece]];
        }
    }
    __block NSUInteger index = 0;
    __block BOOL matches = YES;
    text_input_packets(input, ^BOOL(const char *packet, size_t length) {
        if (index < wanted.count) {
            NSData *expected = [wanted[index] dataUsingEncoding:NSUTF8StringEncoding];
            matches &= length == expected.length && memcmp(packet, expected.bytes, length) == 0;
        }
        index++;
        return YES;
    });
    pieces = wanted;
    if (!matches || index != pieces.count) {
        fprintf(stderr, "FAILED: %s\n", name);
        failures++;
    }
}

static NSString *repeat(NSString *unit, NSUInteger count) {
    return [@"" stringByPaddingToLength:unit.length * count withString:unit startingAtIndex:0];
}

// Compare the actual wire bytes without printing text, including on failure.
static void expect_preedit(const char *name, NSString *marked, NSRange selection,
                           NSString *escaped, NSUInteger begin, NSUInteger end) {
    BOOL hasText = NO;
    NSData *got = text_input_preedit(marked, selection, &hasText);
    NSString *json = [NSString stringWithFormat:@"{\"kind\":\"preedit\",\"text\":\"%@\",\"cursor\":[%lu,%lu]}",
                      escaped, (unsigned long)begin, (unsigned long)end];
    if (escaped ? (![got isEqualToData:[json dataUsingEncoding:NSUTF8StringEncoding]] || hasText != (escaped.length != 0))
                : got != nil) {
        fprintf(stderr, "FAILED: %s\n", name);
        failures++;
    }
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
        // Newline and tab among them go as the keys Enter and Tab; the rest is left out.
        expect("only controls", controls, @[@"KEY:15", @"KEY:28", @"KEY:28"]);
        expect("newlines and a tab", @"一\r\n二\n三\t四", @[@"一", @"KEY:28", @"二", @"KEY:28", @"三", @"KEY:15", @"四"]);
        expect("UTF-8 and JSON escapes", @"日本\"\\/語", @[@"日本\\\"\\\\/語"]);
        NSString *full = repeat(@"😀", 256);
        expect_preedit("clear", @"", NSMakeRange(0, 0), @"", 0, 0);
        expect_preedit("Japanese selection", @"a日本語", NSMakeRange(1, 2), @"a日本語", 1, 7);
        expect_preedit("surrogate pair", @"日😀本", NSMakeRange(1, 2), @"日😀本", 3, 7);
        expect_preedit("inside a surrogate pair", @"日😀本", NSMakeRange(2, 0), @"日😀本", 3, 3);
        expect_preedit("combining mark", @"か\u3099", NSMakeRange(1, 1), @"か\u3099", 3, 6);
        expect_preedit("controls and escaping", [NSString stringWithFormat:@"\t日\n\"\\%C本", (unichar)0x85], NSMakeRange(2, 4), @"日\\\"\\\\本", 3, 5);
        expect_preedit("filtered mark", @"\r\n\t", NSMakeRange(0, 3), @"", 0, 0);
        expect_preedit("clamped selection", @"字", NSMakeRange(NSNotFound, 10), @"字", 3, 3);
        expect_preedit("full preedit", full, NSMakeRange(0, full.length), full, 0, 1024);
        expect_preedit("too many scalars", repeat(@"a", 257), NSMakeRange(0, 0), nil, 0, 0);
        expect_preedit("too many bytes", [full stringByAppendingString:@"a"], NSMakeRange(0, 0), nil, 0, 0);
        expect("both limits exactly", full, @[full]);
        expect("one more scalar", [full stringByAppendingString:@"字"], @[full, @"字"]);
        expect("escaped ASCII at the scalar limit", repeat(@"\\", 257), @[repeat(@"\\\\", 256), @"\\\\"]);

        NSString *prefix = repeat(@"字", 255);
        expect("combining sequence at a packet boundary", [prefix stringByAppendingString:@"か\u3099"], @[prefix, @"か\u3099"]);
        NSString *emoji = @"👩‍💻";
        expect("joined emoji at a packet boundary", [prefix stringByAppendingString:emoji], @[prefix, emoji]);
        NSString *tooLong = [@"か" stringByAppendingString:repeat(@"\u3099", 256)];
        expect("one unsendable sequence between valid ones", [NSString stringWithFormat:@"前%@後", tooLong], @[@"前後"]);
        // An emitter that fails stops everything after it: no gap.
        __block int sent = 0;
        BOOL all = text_input_packets([NSString stringWithFormat:@"%@字字", repeat(@"字", 256)], ^BOOL(const char *p, size_t l) {
            (void)p, (void)l;
            return ++sent < 1;
        });
        if (all || sent != 1) {
            fprintf(stderr, "FAILED: a failed send stops the rest\n");
            failures++;
        }
    }
    if (failures == 0) {
        puts("text input: all tests passed");
    }
    return failures != 0;
}
