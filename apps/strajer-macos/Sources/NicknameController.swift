import AppKit
import Combine
import Foundation

@MainActor
protocol NicknameControllerDelegate: AnyObject {
    func nicknameControllerDidChange(_ controller: NicknameController)
}

@MainActor
final class NicknameController: ObservableObject {
    static let maximumNicknameBytes = 15

    @Published private(set) var nickname: String?
    weak var delegate: NicknameControllerDelegate?

    private static let defaultsKey = "StrajerNickname"
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults

        if let storedNickname = defaults.string(forKey: Self.defaultsKey),
           Self.isValid(storedNickname) {
            nickname = storedNickname
        } else {
            nickname = nil
            defaults.removeObject(forKey: Self.defaultsKey)
        }
    }

    var menuDescription: String {
        nickname ?? "Not set"
    }

    func capture(_ value: String) {
        guard Self.isValid(value) else {
            return
        }

        save(value)
    }

    func promptForNickname() {
        let alert = NSAlert()
        alert.messageText = "Strajer Nickname"
        alert.informativeText = "Use 1 to 15 UTF-8 bytes without control characters."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "Save")
        alert.addButton(withTitle: "Cancel")

        let textField = NSTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 24))
        textField.stringValue = nickname ?? ""
        textField.placeholderString = "Nickname"
        alert.accessoryView = textField

        NSApplication.shared.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else {
            return
        }

        let value = textField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard Self.isValid(value) else {
            showValidationError()
            return
        }

        save(value)
    }

    static func isValid(_ value: String) -> Bool {
        !value.isEmpty
            && value.lengthOfBytes(using: .utf8) <= maximumNicknameBytes
            && !value.contains("\0")
            && !value.unicodeScalars.contains(where: isControlCharacter)
    }

    private func save(_ value: String) {
        guard nickname != value else {
            return
        }

        defaults.set(value, forKey: Self.defaultsKey)
        nickname = value
        delegate?.nicknameControllerDidChange(self)
    }

    private func showValidationError() {
        let alert = NSAlert()
        alert.messageText = "Invalid nickname"
        alert.informativeText = "Nickname must contain 1 to 15 UTF-8 bytes and no control characters."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    private static func isControlCharacter(_ scalar: UnicodeScalar) -> Bool {
        CharacterSet.controlCharacters.contains(scalar)
    }
}
