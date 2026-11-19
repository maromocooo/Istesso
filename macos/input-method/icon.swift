// Draws the input menu's icon for Istesso Dev Input: a black "I" on clear, 16 and 32 pixels, as a
// template image (the menu tints it). Usage: swift icon.swift <output .tiff>
import AppKit

let path = CommandLine.arguments[1]
var representations: [NSBitmapImageRep] = []
for size in [16, 32] {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let s = CGFloat(size)
    NSColor.black.setFill()
    NSRect(x: s * 0.40, y: s * 0.15, width: s * 0.20, height: s * 0.70).fill()
    NSRect(x: s * 0.25, y: s * 0.75, width: s * 0.50, height: s * 0.10).fill()
    NSRect(x: s * 0.25, y: s * 0.15, width: s * 0.50, height: s * 0.10).fill()
    NSGraphicsContext.restoreGraphicsState()
    rep.size = NSSize(width: 16, height: 16)
    representations.append(rep)
}
guard let data = NSBitmapImageRep.representationOfImageReps(in: representations, using: .tiff, properties: [:]) else { exit(1) }
try data.write(to: URL(fileURLWithPath: path))
