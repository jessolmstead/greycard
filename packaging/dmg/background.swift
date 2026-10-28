// The disk image's window background: an arrow from the app to
// Applications and a line saying to drag it there. Drawn rather than
// painted, so it can be changed here and made again:
//
//     swift packaging/dmg/background.swift packaging/dmg
//
// writes background.png (1x) and background@2x.png into the folder
// named, and layout.sh folds the two into background.tiff, the one
// file the image carries. The geometry matches layout.sh's window and
// icon positions; change one and change the other.

import AppKit

let width = 640.0
let height = 400.0
// Icon centers, in Finder's coordinates (y down), as layout.sh sets
// them.
let appCenter = (x: 170.0, y: 190.0)
let applicationsCenter = (x: 470.0, y: 190.0)
let iconSize = 128.0

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor {
    NSColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: alpha)
}

// Light's base surface (theme.slint), its words a step past
// text-secondary, and the selection's blue: a light ground, since
// Finder draws the icons' labels in black in a light appearance, and
// not so light that its white labels in a dark one disappear.
let ground = color(0xc4c4c4)
let ink = color(0x2a2a2a)
let accent = color(0x1f5fbf)

func draw(scale: CGFloat, to url: URL) {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: Int(width * scale), pixelsHigh: Int(height * scale),
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        .retagging(with: .sRGB)!
    rep.size = NSSize(width: width, height: height)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    ground.setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()

    // AppKit's y runs up; Finder's runs down.
    let y = height - appCenter.y
    let gap = iconSize / 2 + 22
    let start = appCenter.x + gap
    let end = applicationsCenter.x - gap
    let head = 18.0

    let shaft = NSBezierPath()
    shaft.move(to: NSPoint(x: start, y: y))
    shaft.line(to: NSPoint(x: end - head, y: y))
    shaft.lineWidth = 6
    shaft.lineCapStyle = .round
    accent.setStroke()
    shaft.stroke()

    let tip = NSBezierPath()
    tip.move(to: NSPoint(x: end, y: y))
    tip.line(to: NSPoint(x: end - head * 1.4, y: y + head))
    tip.line(to: NSPoint(x: end - head * 1.4, y: y - head))
    tip.close()
    accent.setFill()
    tip.fill()

    let words = "Drag greycard into Applications" as NSString
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 15, weight: .medium),
        .foregroundColor: ink,
        .paragraphStyle: style,
    ]
    words.draw(
        in: NSRect(x: 0, y: 62, width: width, height: 24),
        withAttributes: attributes)

    NSGraphicsContext.restoreGraphicsState()
    let png = rep.representation(using: .png, properties: [:])!
    try! png.write(to: url)
}

let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")
draw(scale: 1, to: out.appendingPathComponent("background.png"))
draw(scale: 2, to: out.appendingPathComponent("background@2x.png"))
