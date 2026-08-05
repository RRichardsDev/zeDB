// Renders the zeDB DMG background at @2x (1320x800) using the app's
// placeholder palette. Icon centers in the 660x400 window: app at (165,200),
// Applications at (495,200), icon size 128.
import CoreGraphics
import CoreText
import ImageIO
import Foundation
import UniformTypeIdentifiers

let scale: CGFloat = 2
let width = Int(660 * scale)
let height = Int(400 * scale)

let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(
    data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
    space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func rgb(_ hex: UInt32, alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
}

// Background: BG with a subtle vertical lift toward BG_SIDEBAR.
ctx.setFillColor(rgb(0x1e2227))
ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
let gradient = CGGradient(
    colorsSpace: colorSpace,
    colors: [rgb(0x23272e, alpha: 0.9), rgb(0x1e2227, alpha: 0)] as CFArray,
    locations: [0, 1])!
ctx.drawLinearGradient(
    gradient, start: CGPoint(x: 0, y: CGFloat(height)), end: CGPoint(x: 0, y: 0), options: [])

// Arrow between the two icon positions (window coords y=200 -> centered).
// Icon size is 128, so leave breathing room: shaft from x=250 to x=404.
let y = CGFloat(height) / 2
let shaftStart = 250 * scale
let shaftEnd = 404 * scale
ctx.setStrokeColor(rgb(0x6b7380))
ctx.setLineWidth(5 * scale / 2)
ctx.setLineCap(.round)
ctx.move(to: CGPoint(x: shaftStart, y: y))
ctx.addLine(to: CGPoint(x: shaftEnd, y: y))
ctx.strokePath()
// Arrow head.
let head = 14 * scale
ctx.move(to: CGPoint(x: shaftEnd - head, y: y + head * 0.75))
ctx.addLine(to: CGPoint(x: shaftEnd, y: y))
ctx.addLine(to: CGPoint(x: shaftEnd - head, y: y - head * 0.75))
ctx.strokePath()

// Subtle lighter plates behind the two Finder label zones so black
// light-mode label text stays legible on the dark surface. Labels sit
// under the 128px icons centered at x=165 and x=495 (window coords).
for centerX: CGFloat in [165, 495] {
    let plate = CGRect(
        x: (centerX - 70) * scale, y: CGFloat(height) - (295 * scale),
        width: 140 * scale, height: 26 * scale)
    let path = CGPath(roundedRect: plate, cornerWidth: 6 * scale, cornerHeight: 6 * scale, transform: nil)
    ctx.addPath(path)
    ctx.setFillColor(rgb(0xaab2bd, alpha: 0.16))
    ctx.fillPath()
}

// Caption under the arrow, quiet.
let caption = "Drag zeDB to Applications" as CFString
let font = CTFontCreateWithName("Helvetica Neue" as CFString, 13 * scale, nil)
let attrs: [CFString: Any] = [
    kCTFontAttributeName: font,
    kCTForegroundColorAttributeName: rgb(0x6b7380, alpha: 0.85),
]
let attrString = CFAttributedStringCreate(nil, caption, attrs as CFDictionary)!
let line = CTLineCreateWithAttributedString(attrString)
let bounds = CTLineGetBoundsWithOptions(line, [])
ctx.textPosition = CGPoint(x: (CGFloat(width) - bounds.width) / 2, y: 56 * scale)
CTLineDraw(line, ctx)

let image = ctx.makeImage()!
let out = CommandLine.arguments[1]
let dest = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: out) as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, image, nil)
CGImageDestinationFinalize(dest)
print("wrote \(out)")
