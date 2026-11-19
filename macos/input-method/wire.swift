// The local link between the Mac host and Istesso Dev Input (trial only; see build.sh).
//
// The host owns the socket: a folder of its own (0700) in this user's temporary folder, the socket
// in it 0600. The input method connects to it. Each side checks the other before reading a byte:
// the same user, and code signed by the same leaf certificate as itself with an identifier from a
// short list. An ad hoc signed side (no certificate) is refused: fail closed.
//
// Messages are one JSON object per line, at most 8 KiB. Text (composing or committed) is never
// logged by either side: lengths only.
import Foundation
import Security
import CryptoKit

enum Wire {
    static let inputMethodIdentifier = "com.maromoco.istesso-dev.inputmethod"
    /// The programs the input method takes text from.
    static let hostIdentifiers = ["com.maromoco.istesso-dev.ime-probe", "com.maromoco.istesso-dev.daily-host"]
    static let maxLine = 8192
    /// The same limits as committed text on the control channel (ADR 0007).
    static let maxCharacters = 256
    static let maxBytes = 1024

    /// This user's private temporary folder (the system makes it 0700 for this user alone).
    static func userTemporaryFolder() -> String? {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        guard confstr(_CS_DARWIN_USER_TEMP_DIR, &buffer, buffer.count) > 0 else { return nil }
        return String(cString: buffer)
    }

    static func defaultSocketPath() -> String? {
        guard let base = userTemporaryFolder() else { return nil }
        return (base as NSString).appendingPathComponent("istesso-dev-input/socket")
    }

    /// Text that may be shown or committed: not empty unless allowed, within the limits, no
    /// control characters.
    static func acceptable(_ text: String, allowEmpty: Bool) -> Bool {
        if text.isEmpty { return allowEmpty }
        if text.count > maxCharacters || text.utf8.count > maxBytes { return false }
        return !text.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) }
    }

    /// A UTF-8 byte offset (ADR 0007 counts the selected range that way) as a UTF-16 offset, or nil
    /// when it is not on a character boundary or past the end.
    static func utf16Offset(_ text: String, utf8 offset: Int) -> Int? {
        let utf8 = text.utf8
        guard offset >= 0, offset <= utf8.count else { return nil }
        let index = utf8.index(utf8.startIndex, offsetBy: offset)
        guard let exact = index.samePosition(in: text.unicodeScalars) else { return nil }
        return text.utf16.distance(from: text.utf16.startIndex, to: exact.samePosition(in: text.utf16)!)
    }

    // MARK: - Lines

    static func encode(_ object: [String: Any]) -> Data? {
        guard var data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return nil }
        data.append(0x0A)
        return data.count <= maxLine ? data : nil
    }

    static func writeAll(_ fd: Int32, _ data: Data) -> Bool {
        data.withUnsafeBytes { raw -> Bool in
            var sent = 0
            while sent < raw.count {
                let n = write(fd, raw.baseAddress! + sent, raw.count - sent)
                if n < 0 { if errno == EINTR { continue }; return false }
                sent += n
            }
            return true
        }
    }

    /// Splits what arrived into lines; nil when a line is longer than allowed (the caller closes).
    final class LineReader {
        private var pending = Data()
        func feed(_ bytes: Data) -> [[String: Any]]? {
            pending.append(bytes)
            var out: [[String: Any]] = []
            while let newline = pending.firstIndex(of: 0x0A) {
                let line = pending[pending.startIndex..<newline]
                pending.removeSubrange(pending.startIndex...newline)
                if let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any] { out.append(object) }
            }
            return pending.count > Wire.maxLine ? nil : out
        }
    }

    // MARK: - Who is on the other end

    /// The SHA-1 of the leaf certificate this program is signed with; nil when it is signed ad hoc.
    static func ownLeafHash() -> String? {
        var me: SecCode?
        var staticMe: SecStaticCode?
        var info: CFDictionary?
        guard SecCodeCopySelf([], &me) == errSecSuccess, let me,
              SecCodeCopyStaticCode(me, [], &staticMe) == errSecSuccess, let staticMe,
              SecCodeCopySigningInformation(staticMe, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
              let dict = info as? [String: Any],
              let certificates = dict[kSecCodeInfoCertificates as String] as? [SecCertificate],
              let leaf = certificates.first else { return nil }
        let digest = Insecure.SHA1.hash(data: SecCertificateCopyData(leaf) as Data)
        return digest.map { String(format: "%02X", $0) }.joined()
    }

    enum PeerResult: Equatable { case ok(String), refused(String) }

    /// Checks the process on the other end of a connected Unix socket: the same user, and signed
    /// by the same leaf certificate as this program with one of the given identifiers. Uses the
    /// peer's audit token, not its process number, so a process that exits and is replaced is not
    /// taken for it.
    static func checkPeer(_ fd: Int32, identifiers: [String]) -> PeerResult {
        var uid: uid_t = 0
        var gid: gid_t = 0
        guard getpeereid(fd, &uid, &gid) == 0 else { return .refused("no peer user") }
        guard uid == getuid() else { return .refused("peer is another user") }
        guard let leaf = ownLeafHash() else { return .refused("this program is not signed with a certificate") }
        var token = audit_token_t()
        var length = socklen_t(MemoryLayout<audit_token_t>.size)
        guard getsockopt(fd, 0 /* SOL_LOCAL */, 0x006 /* LOCAL_PEERTOKEN */, &token, &length) == 0 else {
            return .refused("no peer audit token")
        }
        let tokenData = withUnsafeBytes(of: &token) { Data($0) }
        var code: SecCode?
        guard SecCodeCopyGuestWithAttributes(nil, [kSecGuestAttributeAudit: tokenData] as CFDictionary, [], &code) == errSecSuccess,
              let code else { return .refused("peer code not found") }
        let names = identifiers.map { "identifier \"\($0)\"" }.joined(separator: " or ")
        var requirement: SecRequirement?
        guard SecRequirementCreateWithString("(\(names)) and certificate leaf = H\"\(leaf)\"" as CFString, [], &requirement) == errSecSuccess,
              let requirement else { return .refused("requirement not made") }
        let status = SecCodeCheckValidity(code, [], requirement)
        guard status == errSecSuccess else { return .refused("peer signature does not match (\(status))") }
        var staticCode: SecStaticCode?
        var info: CFDictionary?
        if SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode {
            _ = SecCodeCopySigningInformation(staticCode, [], &info)
        }
        let identifier = (info as? [String: Any])?[kSecCodeInfoIdentifier as String] as? String ?? "?"
        return .ok(identifier)
    }

    /// The socket's folder and file belong to this user, are not links, and are 0700 / 0600.
    static func socketIsPrivate(_ path: String) -> Bool {
        var folder = stat()
        var file = stat()
        let parent = (path as NSString).deletingLastPathComponent
        guard lstat(parent, &folder) == 0, lstat(path, &file) == 0 else { return false }
        return (folder.st_mode & S_IFMT) == S_IFDIR && folder.st_uid == getuid() && (folder.st_mode & 0o077) == 0
            && (file.st_mode & S_IFMT) == S_IFSOCK && file.st_uid == getuid() && (file.st_mode & 0o077) == 0
    }

    static func socketAddress(_ path: String) -> sockaddr_un? {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else { return nil }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            for (i, b) in bytes.enumerated() { raw[i] = b }
        }
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        return address
    }

    /// Connects to the host's socket after checking the file; nil when it is absent or not private.
    static func connect(_ path: String) -> Int32? {
        guard socketIsPrivate(path), var address = socketAddress(path) else { return nil }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        let result = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        if result != 0 { close(fd); return nil }
        return fd
    }
}
