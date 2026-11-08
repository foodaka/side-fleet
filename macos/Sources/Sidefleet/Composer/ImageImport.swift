import ImageIO
import AppKit
import SidefleetKit
import SwiftUI
import UniformTypeIdentifiers

/// Images arriving from the clipboard, a drop or the file panel, made into something the control plane
/// accepts: PNG, JPEG, GIF or WebP. A screenshot on the clipboard is TIFF and an iPhone photo is HEIC.
enum ImageImport {
    static func fromPasteboard(_ pb: NSPasteboard) -> [AttachedImage] {
        let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !urls.isEmpty { return fromFiles(urls) }
        // Text wins when there's both: a spreadsheet's cells also come with a picture of themselves.
        if pb.string(forType: .string) != nil { return [] }
        for type in [NSPasteboard.PasteboardType.png, .tiff] {
            if let data = pb.data(forType: type), let image = normalize(data, name: "Pasted image") { return [image] }
        }
        return []
    }

    static func fromFiles(_ urls: [URL]) -> [AttachedImage] {
        urls.compactMap { url in
            guard let data = try? Data(contentsOf: url) else { return nil }
            // Not an image: keep it, so the limits can say what's wrong with it.
            return normalize(data, name: url.lastPathComponent) ?? AttachedImage(name: url.lastPathComponent, mediaType: "application/octet-stream", data: Data())
        }
    }

    static func normalize(_ data: Data, name: String) -> AttachedImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil), let raw = CGImageSourceGetType(source), let type = UTType(raw as String) else { return nil }
        switch type {
        case .png: return AttachedImage(name: name, mediaType: "image/png", data: data)
        case .jpeg: return AttachedImage(name: name, mediaType: "image/jpeg", data: data)
        case .gif: return AttachedImage(name: name, mediaType: "image/gif", data: data)
        case .webP: return AttachedImage(name: name, mediaType: "image/webp", data: data)
        default:
            guard type.conforms(to: .image), type != .svg, let rep = NSBitmapImageRep(data: data) else { return nil }
            if type == .heic || type == .heif, let jpeg = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.9]) {
                return AttachedImage(name: name, mediaType: "image/jpeg", data: jpeg)
            }
            guard let png = rep.representation(using: .png, properties: [:]) else { return nil }
            return AttachedImage(name: name, mediaType: "image/png", data: png)
        }
    }

    @MainActor
    static func pickFiles() -> [AttachedImage] {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.png, .jpeg, .gif, .webP, .heic, .tiff]
        panel.prompt = "Attach"
        return panel.runModal() == .OK ? fromFiles(panel.urls) : []
    }
}

/// Adds images to a message and says why any were left out.
@MainActor
func attach(_ new: [AttachedImage], to items: inout [AttachedImage], toasts: ToastCenter) {
    let skipped = Attachments.add(new, to: &items)
    guard !skipped.isEmpty else { return }
    toasts.show(skipped.count == 1 ? "Could not attach it" : "Could not attach \(skipped.count) files", detail: skipped.joined(separator: ". "), isError: true)
}

/// Thumbnails of what's attached, each with a way to take it off.
struct AttachmentStrip: View {
    @Binding var images: [AttachedImage]

    var body: some View {
        if !images.isEmpty {
            ScrollView(.horizontal) {
                HStack(spacing: 8) {
                    ForEach(images) { image in
                        ZStack(alignment: .topTrailing) {
                            Group {
                                if let ns = NSImage(data: image.data) {
                                    Image(nsImage: ns).resizable().aspectRatio(contentMode: .fill)
                                } else {
                                    SF.fill
                                }
                            }
                            .frame(width: 56, height: 56)
                            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(SF.border))
                            .help(image.name)
                            Button {
                                images.removeAll { $0.id == image.id }
                            } label: {
                                Image(systemName: "xmark")
                                    .font(.system(size: 8, weight: .bold))
                                    .foregroundStyle(.white)
                                    .frame(width: 16, height: 16)
                                    .background(.black.opacity(0.6), in: Circle())
                            }
                            .buttonStyle(.plain)
                            .offset(x: 4, y: -4)
                            .accessibilityLabel("Remove \(image.name)")
                        }
                    }
                }
                .padding(.top, 6)
                .padding(.trailing, 6)
            }
            .scrollIndicators(.never)
        }
    }
}

/// A screen that takes dropped files: an outline says so, and they attach to the message.
struct DropTarget: ViewModifier {
    var enabled = true
    var onImages: ([AttachedImage]) -> Void
    @State private var dragging = false

    func body(content: Content) -> some View {
        content
            .overlay {
                if dragging {
                    RoundedRectangle(cornerRadius: 18, style: .continuous)
                        .strokeBorder(SF.ink.opacity(0.4), style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
                        .background(SF.card.opacity(0.9), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                        .overlay(Text("Drop images to attach them").font(SF.font(14)).foregroundStyle(SF.muted))
                        .padding(12)
                        .allowsHitTesting(false)
                        .transition(.opacity)
                }
            }
            .onDrop(of: [.fileURL], isTargeted: Binding(get: { dragging }, set: { dragging = $0 && enabled })) { providers in
                guard enabled else { return false }
                Task { @MainActor in
                    var urls: [URL] = []
                    for p in providers {
                        if let u = await p.loadFileURL() { urls.append(u) }
                    }
                    onImages(ImageImport.fromFiles(urls))
                }
                return true
            }
    }
}

extension NSItemProvider {
    fileprivate func loadFileURL() async -> URL? {
        await withCheckedContinuation { cont in
            _ = loadObject(ofClass: URL.self) { url, _ in cont.resume(returning: url) }
        }
    }
}
