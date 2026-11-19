// Committed text for ADR 0007 ("確定した文字"), ADR 0020 route d.
// Trial viewer only, enabled with ISTESSO_TEXT_INPUT=1.
//
// A commit becomes input events, in order: text events within the limits (256 scalars, 1024 bytes
// of UTF-8, no control characters), and, for a newline (LF, CR or CR LF) or a tab inside it, the
// key Enter or Tab pressed and let go (they go as keys, not as text). Other control characters are
// left out.
#ifndef ISTESSO_TEXT_INPUT_H
#define ISTESSO_TEXT_INPUT_H

#import <Foundation/Foundation.h>
#include <stdio.h>
#include <string.h>

// The emitter sends one datagram and says whether it went. When it says NO, nothing after it is
// emitted (a commit never arrives with a gap), and text_input_packets returns NO.
typedef BOOL (^TextInputEmit)(const char *, size_t);

static inline BOOL text_input_key(TextInputEmit emit, unsigned code) {
    char key[64];
    int length = snprintf(key, sizeof key, "{\"kind\":\"key\",\"code\":%u,\"pressed\":true}", code);
    if (!emit(key, (size_t)length)) {
        return NO;
    }
    length = snprintf(key, sizeof key, "{\"kind\":\"key\",\"code\":%u,\"pressed\":false}", code);
    return emit(key, (size_t)length);
}

static inline BOOL text_input_packets(NSString *commit, TextInputEmit emit) {
    const char prefix[] = "{\"kind\":\"text\",\"text\":\"";
    NSMutableData *buffer = [NSMutableData dataWithLength:2048];
    char *packet = buffer.mutableBytes;
    memcpy(packet, prefix, sizeof prefix - 1);
    __block size_t used = sizeof prefix - 1;
    __block NSUInteger scalars = 0, bytes = 0;
    BOOL (^flush)(void) = ^BOOL {
        if (scalars == 0) {
            return YES;
        }
        memcpy(packet + used, "\"}", 2);
        BOOL went = emit(packet, used + 2);
        used = sizeof prefix - 1;
        scalars = bytes = 0;
        return went;
    };
    for (NSUInteger start = 0; start < commit.length;) {
        NSRange range = [commit rangeOfComposedCharacterSequenceAtIndex:start];
        start = NSMaxRange(range);
        NSString *piece = [commit substringWithRange:range];
        if ([piece isEqualToString:@"\n"] || [piece isEqualToString:@"\r"] || [piece isEqualToString:@"\r\n"]) {
            // CR LF is one newline (NSString may give it as two sequences).
            if ([piece isEqualToString:@"\r"] && start < commit.length && [commit characterAtIndex:start] == '\n') {
                start++;
            }
            if (!flush() || !text_input_key(emit, 28)) {
                return NO;
            }
            continue;
        }
        if ([piece isEqualToString:@"\t"]) {
            if (!flush() || !text_input_key(emit, 15)) {
                return NO;
            }
            continue;
        }
        // Other control characters are left out; what is around them stays.
        NSMutableString *clean = [NSMutableString string];
        for (NSUInteger i = 0; i < piece.length; i++) {
            unichar c = [piece characterAtIndex:i];
            if (c < 0x20 || (c >= 0x7f && c <= 0x9f)) {
                continue;
            }
            [clean appendFormat:@"%C", c];
        }
        // A single over-limit sequence cannot be sent intact.
        if (clean.length == 0 || clean.length > 2 * 256) {
            continue;
        }
        NSData *sequence = [clean dataUsingEncoding:NSUTF8StringEncoding];
        if (sequence.length == 0 || sequence.length > 1024) {
            continue;
        }
        const unsigned char *utf8 = sequence.bytes;
        NSUInteger count = 0;
        for (NSUInteger i = 0; i < sequence.length; i++) {
            count += (utf8[i] & 0xc0) != 0x80;
        }
        if (count > 256) {
            continue;
        }
        if ((scalars + count > 256 || bytes + sequence.length > 1024) && !flush()) {
            return NO;
        }
        scalars += count;
        bytes += sequence.length;
        // Escaping adds at most one byte per scalar to the 1024 payload bytes.
        for (NSUInteger i = 0; i < sequence.length; i++) {
            if (utf8[i] == '"' || utf8[i] == '\\') {
                packet[used++] = '\\';
            }
            packet[used++] = (char)utf8[i];
        }
    }
    return flush();
}

#endif
