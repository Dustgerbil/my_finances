import Foundation
import UIKit
import UniformTypeIdentifiers
import Tauri

private struct ReadOptions: Decodable { let destination: String }

final class FinanceSyncPlugin: Plugin, UIDocumentPickerDelegate {
    private let bookmarkKey = "financeSyncFolderBookmark"
    private var pending: Invoke?
    private let queue = DispatchQueue(label: "com.stuartg.my-finances.sync")

    @objc public func connect(_ invoke: Invoke) {
        DispatchQueue.main.async {
            guard self.pending == nil else {
                invoke.reject("A folder picker is already open.")
                return
            }
            guard let controller = self.manager.viewController else {
                invoke.reject("The app is not ready to select a folder.")
                return
            }
            self.pending = invoke
            let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
            picker.allowsMultipleSelection = false
            picker.delegate = self
            controller.present(picker, animated: true)
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        pending?.reject("Folder selection cancelled. Your previous connection is unchanged.")
        pending = nil
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        defer { pending = nil }
        guard let folder = urls.first, folder.startAccessingSecurityScopedResource() else {
            pending?.reject("Folder access was not granted. Please select the mobile folder in iCloud Drive.")
            return
        }
        defer { folder.stopAccessingSecurityScopedResource() }
        do {
            // iOS bookmarks inherit the document picker's security scope; the
            // macOS-only .withSecurityScope option must not be used here.
            let bookmark = try folder.bookmarkData(options: .minimalBookmark,
                includingResourceValuesForKeys: nil, relativeTo: nil)
            UserDefaults.standard.set(bookmark, forKey: bookmarkKey)
            pending?.resolve(["connected": true])
        } catch {
            pending?.reject("Could not remember folder access. Please reconnect.")
        }
    }

    @objc public func readSnapshot(_ invoke: Invoke) throws {
        let options = try invoke.parseArgs(ReadOptions.self)
        queue.async {
            guard let bookmark = UserDefaults.standard.data(forKey: self.bookmarkKey) else {
                invoke.reject("Connect the mobile folder in iCloud Drive to start automatic sync.")
                return
            }
            do {
                var stale = false
                let folder = try URL(resolvingBookmarkData: bookmark, options: [],
                    relativeTo: nil, bookmarkDataIsStale: &stale)
                guard folder.startAccessingSecurityScopedResource() else {
                    invoke.reject("Folder access has expired. Reconnect to iCloud Drive.")
                    return
                }
                defer { folder.stopAccessingSecurityScopedResource() }
                if stale {
                    let renewed = try folder.bookmarkData(options: .minimalBookmark,
                        includingResourceValuesForKeys: nil, relativeTo: nil)
                    UserDefaults.standard.set(renewed, forKey: self.bookmarkKey)
                }
                let source = folder.appendingPathComponent("read-only.db")
                if (try? source.resourceValues(forKeys: [.isUbiquitousItemKey]).isUbiquitousItem) == true {
                    try FileManager.default.startDownloadingUbiquitousItem(at: source)
                }
                var coordinationError: NSError?
                var copyError: Error?
                let coordinator = NSFileCoordinator(filePresenter: nil)
                coordinator.coordinate(readingItemAt: source, options: [], error: &coordinationError) { url in
                    do {
                        // Bound input before allocating or copying it. Rust also
                        // checks the size and validates SQLite before promotion.
                        let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                        guard size > 0 && size <= 134_217_728 else {
                            throw NSError(domain: "FinanceSync", code: 1)
                        }
                        let destination = URL(fileURLWithPath: options.destination)
                        if FileManager.default.fileExists(atPath: destination.path) {
                            try FileManager.default.removeItem(at: destination)
                        }
                        try FileManager.default.copyItem(at: url, to: destination)
                        try FileManager.default.setAttributes(
                            [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication],
                            ofItemAtPath: destination.path)
                    } catch { copyError = error }
                }
                if coordinationError != nil || copyError != nil {
                    invoke.reject("Snapshot unavailable. Check iCloud connectivity and that the Mac has published read-only.db. Your cached data is kept.")
                } else {
                    invoke.resolve(["copied": true])
                }
            } catch {
                invoke.reject("Could not read the linked iCloud folder. Check connectivity or reconnect. Your cached data is kept.")
            }
        }
    }
}

@_cdecl("init_plugin_finance_sync")
func initPlugin() -> Plugin { FinanceSyncPlugin() }
