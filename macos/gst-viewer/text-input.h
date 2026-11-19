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

static inline BOOL text_input_control(unichar c) {
    return c < 0x20 || (c >= 0x7f && c <= 0x9f);
}

// The caller has room for the UTF-8 bytes plus one escape per scalar.
static inline size_t text_input_escape(char *out, const unsigned char *utf8, size_t length) {
    size_t used = 0;
    for (size_t i = 0; i < length; i++) {
        if (utf8[i] == '"' || utf8[i] == '\\') {
            out[used++] = '\\';
        }
        out[used++] = (char)utf8[i];
    }
    return used;
}

// One whole marked string or nil if it cannot travel. Cursor offsets count unescaped,
// printable UTF-8 bytes. An endpoint inside a surrogate pair stays before that scalar.
static inline NSData *text_input_preedit(NSString *marked, NSRange selection, BOOL *hasText) {
    if (marked.length > 2 * 256) {
        return nil;
    }
    NSData *data = [marked dataUsingEncoding:NSUTF8StringEncoding];
    if (!data || data.length > 1024) {
        return nil;
    }
    NSUInteger start = MIN(selection.location, marked.length);
    NSUInteger end = start + MIN(selection.length, marked.length - start);
    char packet[2048] = "{\"kind\":\"preedit\",\"text\":\"";
    size_t used = strlen(packet), bytes = 0, beginByte = 0, endByte = 0;
    const unsigned char *utf8 = data.bytes;
    NSUInteger offset = 0, scalars = 0;
    for (NSUInteger i = 0; i < data.length;) {
        NSUInteger next = i + 1;
        while (next < data.length && (utf8[next] & 0xc0) == 0x80) {
            next++;
        }
        if (++scalars > 256) {
            return nil;
        }
        NSUInteger length = next - i;
        if (!text_input_control([marked characterAtIndex:offset])) {
            used += text_input_escape(packet + used, utf8 + i, length);
            bytes += length;
        }
        offset += length == 4 ? 2 : 1;
        if (offset <= start) beginByte = bytes;
        if (offset <= end) endByte = bytes;
        i = next;
    }
    used += (size_t)snprintf(packet + used, sizeof packet - used, "\",\"cursor\":[%zu,%zu]}", beginByte, endByte);
    *hasText = bytes != 0;
    return [NSData dataWithBytes:packet length:used];
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
            if (text_input_control(c)) {
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
        used += text_input_escape(packet + used, utf8, sequence.length);
    }
    return flush();
}

#endif
