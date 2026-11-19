// Committed text for ADR 0007 ("確定した文字"), ADR 0020 route d.
// Trial viewer only, enabled with ISTESSO_TEXT_INPUT=1.
#ifndef ISTESSO_TEXT_INPUT_H
#define ISTESSO_TEXT_INPUT_H

#import <Foundation/Foundation.h>
#include <string.h>

// Emit complete datagrams. The callback consumes the bytes before returning.
static inline void text_input_packets(NSString *commit, void (^emit)(const char *, size_t)) {
    NSMutableCharacterSet *controls = [NSMutableCharacterSet characterSetWithRange:NSMakeRange(0, 32)];
    [controls addCharactersInRange:NSMakeRange(0x7f, 33)];
    NSString *text = [[commit componentsSeparatedByCharactersInSet:controls] componentsJoinedByString:@""];

    const char prefix[] = "{\"kind\":\"text\",\"text\":\"";
    char packet[2048];
    memcpy(packet, prefix, sizeof prefix - 1);
    size_t used = sizeof prefix - 1;
    NSUInteger scalars = 0, bytes = 0;
    for (NSUInteger start = 0; start < text.length;) {
        NSRange range = [text rangeOfComposedCharacterSequenceAtIndex:start];
        start = NSMaxRange(range);
        // A single over-limit sequence cannot be sent intact.
        if (range.length > 2 * 256) {
            continue;
        }
        NSData *sequence = [[text substringWithRange:range] dataUsingEncoding:NSUTF8StringEncoding];
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
        if (scalars + count > 256 || bytes + sequence.length > 1024) {
            memcpy(packet + used, "\"}", 2);
            emit(packet, used + 2);
            used = sizeof prefix - 1;
            scalars = bytes = 0;
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
    if (scalars > 0) {
        memcpy(packet + used, "\"}", 2);
        emit(packet, used + 2);
    }
}

#endif
